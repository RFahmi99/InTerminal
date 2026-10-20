//! Locating, downloading and verifying the GGUF model used for command
//! generation. The default model is downloaded only after the user agrees,
//! and is checked against a pinned SHA-256 before it is ever loaded.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, CmdResult, ErrorCode};

pub struct ModelSpec {
    pub name: &'static str,
    pub file_name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    pub license: &'static str,
}

/// Pinned to an immutable Hugging Face commit.
pub const DEFAULT_MODEL: ModelSpec = ModelSpec {
    name: "Qwen2.5-Coder 0.5B Instruct (4-bit)",
    file_name: "qwen2.5-coder-0.5b-instruct-q4_k_m.gguf",
    url: "https://huggingface.co/Qwen/Qwen2.5-Coder-0.5B-Instruct-GGUF/resolve/ebb2015119c907b064c512bf053e945850b5875f/qwen2.5-coder-0.5b-instruct-q4_k_m.gguf",
    sha256: "1d9614638d18024d0fbb36575a15f1302a3adf044df10345688ec4f6e1c4ff32",
    bytes: 491_400_064,
    license: "Apache-2.0",
};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
}

pub fn default_model_path(models_dir: &Path) -> PathBuf {
    models_dir.join(DEFAULT_MODEL.file_name)
}

/// Checks that `path` is a readable GGUF file, so a wrong pick fails here
/// with a clear message instead of deep inside llama.cpp.
pub fn validate_gguf(path: &Path) -> CmdResult<()> {
    let mut magic = [0u8; 4];
    File::open(path).and_then(|mut f| f.read_exact(&mut magic)).map_err(|e| AppError::io("read model file", path, &e))?;
    if &magic != b"GGUF" {
        return Err(AppError::new(
            ErrorCode::InvalidInput,
            format!("\"{}\" isn't a GGUF model file. Choose a .gguf file made for llama.cpp.", path.display()),
        ));
    }
    Ok(())
}

/// HTTPS through Windows' own TLS stack (SChannel) and certificate store.
/// That respects enterprise root certificates and TLS-inspecting proxies, and
/// avoids compiling a C crypto library (which Smart App Control can block).
fn https_agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .build(),
    )
}

/// Downloads the default model into `models_dir`, reporting progress at most
/// a few times per second. The file only appears under its final name once
/// the hash matches, so an interrupted download is never mistaken for a model.
///
/// Each `attempt` writes its own partial file: a read on a dead connection
/// can block indefinitely, so cancelling abandons the attempt rather than
/// waiting for it, and a retry must not share its file.
pub fn download_default(
    models_dir: &Path,
    attempt: u64,
    is_cancelled: impl Fn() -> bool,
    mut on_progress: impl FnMut(DownloadProgress),
) -> CmdResult<PathBuf> {
    std::fs::create_dir_all(models_dir).map_err(|e| AppError::io("create folder", models_dir, &e))?;
    let target = default_model_path(models_dir);
    let partial = target.with_extension(format!("gguf.partial-{attempt}"));

    let failed = |what: String| AppError::new(ErrorCode::AiFailed, format!("Downloading the model failed: {what}"));
    let response = https_agent().get(DEFAULT_MODEL.url).call().map_err(|e| failed(e.to_string()))?;
    let total =
        response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MODEL.bytes);
    let mut reader = response.into_body().into_reader();
    let mut file = File::create(&partial).map_err(|e| AppError::io("create", &partial, &e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut downloaded = 0u64;
    let mut last_report = Instant::now();

    let result = (|| {
        loop {
            if is_cancelled() {
                return Err(AppError::new(ErrorCode::Cancelled, "The model download was cancelled."));
            }
            let n = reader.read(&mut buffer).map_err(|e| failed(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n]).map_err(|e| AppError::io("write", &partial, &e))?;
            hasher.update(&buffer[..n]);
            downloaded += n as u64;
            if last_report.elapsed() > Duration::from_millis(250) {
                on_progress(DownloadProgress { downloaded, total });
                last_report = Instant::now();
            }
        }
        file.sync_all().map_err(|e| AppError::io("save", &partial, &e))?;
        let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if actual != DEFAULT_MODEL.sha256 {
            return Err(failed(format!(
                "the file's checksum doesn't match (expected {}, got {actual}). It may be corrupted; try again.",
                DEFAULT_MODEL.sha256
            )));
        }
        Ok(())
    })();

    drop(file);
    let result = result.and_then(|()| std::fs::rename(&partial, &target).map_err(|e| AppError::io("save", &target, &e)));
    if let Err(e) = result {
        // Don't leave ~500 MB of partial download behind.
        if let Err(remove_error) = std::fs::remove_file(&partial) {
            log::warn!("Could not remove partial download {}: {remove_error}", partial.display());
        }
        return Err(e);
    }
    on_progress(DownloadProgress { downloaded, total });
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_gguf_checks_magic_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("m.gguf");
        std::fs::write(&good, b"GGUF\x03\x00\x00\x00").unwrap();
        assert!(validate_gguf(&good).is_ok());
        let bad = dir.path().join("m.bin");
        std::fs::write(&bad, b"PK\x03\x04").unwrap();
        assert_eq!(validate_gguf(&bad).unwrap_err().code, ErrorCode::InvalidInput);
        assert_eq!(validate_gguf(&dir.path().join("missing.gguf")).unwrap_err().code, ErrorCode::NotFound);
    }
}
