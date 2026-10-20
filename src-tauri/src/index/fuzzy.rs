//! Typo-tolerant name matching for "did you mean" suggestions.

use std::collections::HashSet;

/// Optimal string alignment distance: Levenshtein plus adjacent
/// transpositions, so "gti" is one edit from "git". Case-insensitive.
pub fn osa_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

/// Largest edit distance still considered a typo of a name this long.
fn max_typo_distance(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=4 => 1,
        5..=8 => 2,
        _ => 3,
    }
}

/// Up to `limit` names closest to `typed`, nearest first. Among equally near
/// names, ones in `popular` (commands people commonly mean) come first.
pub fn closest<'a>(
    typed: &str,
    candidates: impl Iterator<Item = &'a str>,
    popular: &HashSet<String>,
    limit: usize,
) -> Vec<String> {
    let typed_len = typed.chars().count();
    let max = max_typo_distance(typed_len);
    if max == 0 {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut scored: Vec<(usize, bool, usize, String)> = candidates
        .filter(|name| !name.eq_ignore_ascii_case(typed) && seen.insert(name.to_lowercase()))
        .filter_map(|name| {
            let len = name.chars().count();
            if len.abs_diff(typed_len) > max {
                return None;
            }
            let distance = osa_distance(typed, name);
            (distance <= max).then(|| (distance, !popular.contains(&name.to_lowercase()), len.abs_diff(typed_len), name.to_string()))
        })
        .collect();
    scored.sort();
    scored.into_iter().take(limit).map(|(.., name)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_transpositions_as_one_edit() {
        assert_eq!(osa_distance("gti", "git"), 1);
        assert_eq!(osa_distance("pyhton", "python"), 1);
        assert_eq!(osa_distance("Get-ChildItme", "get-childitem"), 1);
        assert_eq!(osa_distance("npm", "pnpm"), 1);
        assert_eq!(osa_distance("", "abc"), 3);
    }

    #[test]
    fn closest_prefers_popular_names_at_equal_distance() {
        let popular = HashSet::from(["git".to_string()]);
        let names = ["gci", "git", "gi", "gitk", "notepad"];
        let result = closest("gti", names.into_iter(), &popular, 3);
        assert_eq!(result[0], "git");
        assert!(!result.contains(&"notepad".to_string()));
    }

    #[test]
    fn very_short_names_get_no_suggestions() {
        assert!(closest("x", ["xx", "y"].into_iter(), &HashSet::new(), 3).is_empty());
    }
}
