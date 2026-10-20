//! Static (model2vec) sentence embeddings: a sentence vector is the mean of
//! per-token vectors looked up in a table, so encoding a query takes
//! microseconds and needs no neural-network runtime.
//!
//! The bundled model is `minishlab/potion-base-8M`, which uses the uncased
//! BERT WordPiece tokenizer. This module reimplements that tokenizer rather
//! than pulling in the `tokenizers` crate and its C dependencies.

use std::collections::HashMap;
use std::path::Path;

/// Rows are stored as i8 with one f32 scale per row: a quarter of the f32
/// memory with no measurable effect on nearest-neighbour ranking.
pub struct StaticEmbedder {
    vocab: HashMap<String, u32>,
    unk_id: Option<u32>,
    continuing_prefix: String,
    max_chars_per_word: usize,
    dim: usize,
    weights: Vec<i8>,
    scales: Vec<f32>,
}

impl StaticEmbedder {
    /// Loads `tokenizer.json` and `model.safetensors` from `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let tokenizer_path = dir.join("tokenizer.json");
        let tokenizer_text = std::fs::read_to_string(&tokenizer_path)
            .map_err(|e| format!("reading {}: {e}", tokenizer_path.display()))?;
        let tokenizer: serde_json::Value = serde_json::from_str(&tokenizer_text)
            .map_err(|e| format!("parsing {}: {e}", tokenizer_path.display()))?;
        let model = &tokenizer["model"];
        if model["type"] != "WordPiece" {
            return Err(format!("{} is not a WordPiece tokenizer", tokenizer_path.display()));
        }
        let vocab: HashMap<String, u32> = model["vocab"]
            .as_object()
            .ok_or_else(|| format!("{} has no vocabulary", tokenizer_path.display()))?
            .iter()
            .filter_map(|(token, id)| Some((token.clone(), u32::try_from(id.as_u64()?).ok()?)))
            .collect();
        let unk_id = model["unk_token"].as_str().and_then(|unk| vocab.get(unk).copied());
        let continuing_prefix = model["continuing_subword_prefix"].as_str().unwrap_or("##").to_string();
        let max_chars_per_word = model["max_input_chars_per_word"].as_u64().unwrap_or(100) as usize;

        let model_path = dir.join("model.safetensors");
        let bytes = std::fs::read(&model_path).map_err(|e| format!("reading {}: {e}", model_path.display()))?;
        let (rows, dim, floats) = read_embedding_tensor(&bytes).map_err(|e| format!("{}: {e}", model_path.display()))?;
        if rows < vocab.len() {
            return Err(format!("{} has {rows} rows but the vocabulary has {} tokens", model_path.display(), vocab.len()));
        }
        let (weights, scales) = quantize_rows(floats, dim);
        Ok(Self { vocab, unk_id, continuing_prefix, max_chars_per_word, dim, weights, scales })
    }

    /// Returns the L2-normalised sentence vector, or `None` when no token of
    /// `text` is in the vocabulary.
    pub fn embed(&self, text: &str) -> Option<Vec<f32>> {
        let ids: Vec<u32> = self.tokenize(text).into_iter().filter(|id| Some(*id) != self.unk_id).collect();
        if ids.is_empty() {
            return None;
        }
        let mut sum = vec![0f32; self.dim];
        for id in &ids {
            let row = *id as usize;
            let scale = self.scales[row];
            let start = row * self.dim;
            for (acc, q) in sum.iter_mut().zip(&self.weights[start..start + self.dim]) {
                *acc += f32::from(*q) * scale;
            }
        }
        let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            return None;
        }
        sum.iter_mut().for_each(|x| *x /= norm);
        Some(sum)
    }

    fn tokenize(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        for word in pre_tokenize(&normalize(text)) {
            self.word_piece(&word, &mut ids);
        }
        ids
    }

    /// Greedy longest-match-first WordPiece. A word with any unmatchable
    /// piece becomes a single unknown token, as in BERT.
    fn word_piece(&self, word: &str, ids: &mut Vec<u32>) {
        let chars: Vec<char> = word.chars().collect();
        if chars.len() > self.max_chars_per_word {
            ids.extend(self.unk_id);
            return;
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let mut end = chars.len();
            let mut found = None;
            while start < end {
                let piece: String = chars[start..end].iter().collect();
                let candidate = if start > 0 { format!("{}{piece}", self.continuing_prefix) } else { piece };
                if let Some(id) = self.vocab.get(&candidate) {
                    found = Some(*id);
                    break;
                }
                end -= 1;
            }
            match found {
                Some(id) => {
                    pieces.push(id);
                    start = end;
                }
                None => {
                    ids.extend(self.unk_id);
                    return;
                }
            }
        }
        ids.extend(pieces);
    }
}

/// An L2-normalised sentence vector stored like the model's rows, as i8 with
/// one scale: the search index keeps thousands of these resident.
pub struct CompactVector {
    scale: f32,
    values: Box<[i8]>,
}

impl CompactVector {
    pub fn new(vector: &[f32]) -> Self {
        let (scale, values) = quantize_row(vector);
        Self { scale, values: values.into_boxed_slice() }
    }

    /// Cosine similarity with an L2-normalised `query`.
    pub fn cosine(&self, query: &[f32]) -> f32 {
        self.scale * query.iter().zip(&*self.values).map(|(q, v)| q * f32::from(*v)).sum::<f32>()
    }
}

/// Parses the single `embeddings` F32 tensor out of a safetensors file:
/// an 8-byte little-endian header length, a JSON header, then raw data.
fn read_embedding_tensor(bytes: &[u8]) -> Result<(usize, usize, Vec<f32>), String> {
    let header_len = bytes
        .get(..8)
        .map(|b| u64::from_le_bytes(b.try_into().expect("slice of 8 bytes")) as usize)
        .ok_or("file is too short for a safetensors header")?;
    let header_bytes = bytes.get(8..8 + header_len).ok_or("safetensors header is truncated")?;
    let header: serde_json::Value = serde_json::from_slice(header_bytes).map_err(|e| format!("invalid safetensors header: {e}"))?;
    let tensor = &header["embeddings"];
    if tensor["dtype"] != "F32" {
        return Err(format!("expected F32 embeddings, found {}", tensor["dtype"]));
    }
    let shape: Vec<usize> = tensor["shape"]
        .as_array()
        .map(|dims| dims.iter().filter_map(|d| d.as_u64().map(|d| d as usize)).collect())
        .unwrap_or_default();
    let [rows, dim] = shape[..] else {
        return Err(format!("expected a 2-D embeddings tensor, found shape {shape:?}"));
    };
    let offsets: Vec<usize> = tensor["data_offsets"]
        .as_array()
        .map(|o| o.iter().filter_map(|v| v.as_u64().map(|v| v as usize)).collect())
        .unwrap_or_default();
    let [begin, end] = offsets[..] else {
        return Err("embeddings tensor has no data offsets".into());
    };
    let data_start = 8 + header_len;
    let data = bytes.get(data_start + begin..data_start + end).ok_or("embeddings data is truncated")?;
    if data.len() != rows * dim * 4 {
        return Err(format!("embeddings data is {} bytes, expected {}", data.len(), rows * dim * 4));
    }
    let floats = data.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().expect("chunk of 4 bytes"))).collect();
    Ok((rows, dim, floats))
}

fn quantize_rows(floats: Vec<f32>, dim: usize) -> (Vec<i8>, Vec<f32>) {
    let mut weights = Vec::with_capacity(floats.len());
    let mut scales = Vec::with_capacity(floats.len() / dim);
    for row in floats.chunks_exact(dim) {
        let (scale, values) = quantize_row(row);
        scales.push(scale);
        weights.extend(values);
    }
    (weights, scales)
}

/// Scales `row` so its largest magnitude maps to 127.
fn quantize_row(row: &[f32]) -> (f32, Vec<i8>) {
    let max_abs = row.iter().fold(0f32, |m, x| m.max(x.abs()));
    let scale = if max_abs > 0.0 { max_abs / 127.0 } else { 1.0 };
    (scale, row.iter().map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8).collect())
}

/// BERT normalisation: drop control characters, map whitespace to spaces,
/// isolate CJK ideographs, lowercase and strip accents.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\0' || ch == '\u{FFFD}' || (ch.is_control() && !ch.is_whitespace()) {
            continue;
        }
        if ch.is_whitespace() {
            out.push(' ');
        } else if is_cjk(ch) {
            out.push(' ');
            out.push(ch);
            out.push(' ');
        } else {
            for lower in ch.to_lowercase() {
                out.push(strip_accent(lower));
            }
        }
    }
    out
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2B73F
        | 0x2B740..=0x2B81F | 0x2B820..=0x2CEAF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F)
}

/// Folds the accented Latin letters people realistically type into a search
/// box. Full NFD decomposition would need Unicode tables this app otherwise
/// has no use for.
fn strip_accent(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ő' => 'o',
        'ř' => 'r',
        'ś' | 'š' | 'ş' => 's',
        'ť' | 'ţ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation() || (!ch.is_ascii() && !ch.is_alphanumeric() && !ch.is_whitespace())
}

/// BERT pre-tokenisation: split on whitespace and make every punctuation
/// character its own token.
fn pre_tokenize(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for chunk in text.split(' ').filter(|c| !c.is_empty()) {
        let mut current = String::new();
        for ch in chunk.chars() {
            if is_punctuation(ch) {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                words.push(ch.to_string());
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() {
            words.push(current);
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_embedder() -> StaticEmbedder {
        let tokens = ["[UNK]", "zip", "file", "##s", "un", "##zip", "extract", "list", "process", "-"];
        let vocab: HashMap<String, u32> = tokens.iter().enumerate().map(|(i, t)| (t.to_string(), i as u32)).collect();
        let dim = 4;
        // Each token gets a distinct direction; "unzip" pieces share the zip axis.
        let rows: [[f32; 4]; 10] = [
            [0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.1, 0.0, 0.0],
            [0.5, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.8, 0.0, 0.2, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.1, 1.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
        let (weights, scales) = quantize_rows(rows.iter().flatten().copied().collect(), dim);
        StaticEmbedder {
            vocab,
            unk_id: Some(0),
            continuing_prefix: "##".into(),
            max_chars_per_word: 100,
            dim,
            weights,
            scales,
        }
    }

    #[test]
    fn normalizes_like_bert_uncased() {
        assert_eq!(normalize("Café\tZIP"), "cafe zip");
        assert_eq!(pre_tokenize("kill-port 3000!"), vec!["kill", "-", "port", "3000", "!"]);
    }

    #[test]
    fn word_piece_uses_longest_match_and_continuation_prefix() {
        let e = tiny_embedder();
        assert_eq!(e.tokenize("unzip files"), vec![4, 5, 2, 3]);
        // An unmatchable piece turns the whole word into [UNK].
        assert_eq!(e.tokenize("zipq"), vec![0]);
    }

    #[test]
    fn similar_text_scores_higher() {
        let e = tiny_embedder();
        let query = e.embed("unzip").unwrap();
        let zip = e.embed("extract zip file").unwrap();
        let ps = e.embed("list process").unwrap();
        assert!(CompactVector::new(&zip).cosine(&query) > CompactVector::new(&ps).cosine(&query));
        assert!(e.embed("qqq").is_none());
    }

    #[test]
    fn compact_vector_cosine_matches_f32() {
        let e = tiny_embedder();
        let query = e.embed("unzip").unwrap();
        let doc = e.embed("extract zip file").unwrap();
        let exact: f32 = query.iter().zip(&doc).map(|(a, b)| a * b).sum();
        assert!((CompactVector::new(&doc).cosine(&query) - exact).abs() < 0.01);
    }

    #[test]
    fn quantization_preserves_values_closely() {
        let row = vec![0.5f32, -1.0, 0.25, 0.0];
        let (weights, scales) = quantize_rows(row.clone(), 4);
        for (q, original) in weights.iter().zip(&row) {
            assert!((f32::from(*q) * scales[0] - original).abs() < 0.01);
        }
    }

    #[test]
    fn rejects_malformed_safetensors() {
        assert!(read_embedding_tensor(&[1, 2, 3]).is_err());
        let header = br#"{"embeddings":{"dtype":"F16","shape":[1,1],"data_offsets":[0,2]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(&[0, 0]);
        assert!(read_embedding_tensor(&bytes).unwrap_err().contains("F32"));
    }
}
