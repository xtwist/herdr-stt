use regex::Regex;
use serde::{Deserialize, Serialize};
use strsim::levenshtein;

use crate::protocol::Transcript;

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkCase {
    pub audio: String,
    pub reference: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub hotwords: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    pub cases: usize,
    pub raw: ErrorMetrics,
    pub deterministic: ErrorMetrics,
    pub corrected: ErrorMetrics,
    pub technical_identifier_accuracy: f64,
    pub protected_token_errors: usize,
    pub correction_applied: usize,
    pub correction_fallbacks: usize,
    pub correction_latency_p50_ms: Option<u64>,
    pub correction_latency_p95_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ErrorMetrics {
    pub cer: f64,
    pub wer: f64,
}

pub fn build_report(cases: &[(BenchmarkCase, Transcript)]) -> BenchmarkReport {
    let raw = stage_metrics(cases, |transcript| &transcript.raw_text);
    let deterministic = stage_metrics(cases, |transcript| &transcript.deterministic_text);
    let corrected = stage_metrics(cases, |transcript| &transcript.text);
    let mut identifier_total = 0usize;
    let mut identifier_hits = 0usize;
    let mut protected_errors = 0usize;
    let mut latencies = Vec::new();
    for (case, transcript) in cases {
        let identifiers = technical_identifiers(&case.reference);
        identifier_total += identifiers.len();
        identifier_hits += identifiers
            .iter()
            .filter(|identifier| transcript.text.contains(identifier.as_str()))
            .count();
        if protected_tokens(&case.reference) != protected_tokens(&transcript.text) {
            protected_errors += 1;
        }
        if let Some(latency) = transcript.correction.latency_ms {
            latencies.push(latency);
        }
    }
    latencies.sort_unstable();
    BenchmarkReport {
        cases: cases.len(),
        raw,
        deterministic,
        corrected,
        technical_identifier_accuracy: if identifier_total == 0 {
            1.0
        } else {
            identifier_hits as f64 / identifier_total as f64
        },
        protected_token_errors: protected_errors,
        correction_applied: cases
            .iter()
            .filter(|(_, transcript)| transcript.correction.status == "applied")
            .count(),
        correction_fallbacks: cases
            .iter()
            .filter(|(_, transcript)| transcript.correction.status == "fallback")
            .count(),
        correction_latency_p50_ms: percentile(&latencies, 0.50),
        correction_latency_p95_ms: percentile(&latencies, 0.95),
    }
}

fn stage_metrics<F>(cases: &[(BenchmarkCase, Transcript)], select: F) -> ErrorMetrics
where
    F: Fn(&Transcript) -> &str,
{
    let mut character_distance = 0usize;
    let mut character_total = 0usize;
    let mut word_distance = 0usize;
    let mut word_total = 0usize;
    for (case, transcript) in cases {
        let reference = normalize(&case.reference);
        let candidate = normalize(select(transcript));
        let reference_chars = reference.chars().collect::<Vec<_>>();
        let candidate_chars = candidate.chars().collect::<Vec<_>>();
        character_distance += levenshtein_chars(&reference_chars, &candidate_chars);
        character_total += reference_chars.len();
        let reference_words = reference.split_whitespace().collect::<Vec<_>>();
        let candidate_words = candidate.split_whitespace().collect::<Vec<_>>();
        word_distance += levenshtein_items(&reference_words, &candidate_words);
        word_total += reference_words.len();
    }
    ErrorMetrics {
        cer: character_distance as f64 / character_total.max(1) as f64,
        wer: word_distance as f64 / word_total.max(1) as f64,
    }
}

fn levenshtein_chars(left: &[char], right: &[char]) -> usize {
    let left = left.iter().collect::<String>();
    let right = right.iter().collect::<String>();
    levenshtein(&left, &right)
}

fn levenshtein_items<T: PartialEq>(left: &[T], right: &[T]) -> usize {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_item) in left.iter().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_item) in right.iter().enumerate() {
            current.push(
                (previous[right_index + 1] + 1)
                    .min(current[right_index] + 1)
                    .min(previous[right_index] + usize::from(left_item != right_item)),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn technical_identifiers(text: &str) -> Vec<String> {
    Regex::new(
        r"\b(?:[A-Za-z][A-Za-z0-9]*_[A-Za-z0-9_]+|[A-Za-z]+[A-Z][A-Za-z0-9]*|[A-Z][A-Z0-9]{2,})\b",
    )
    .expect("valid identifier regex")
    .find_iter(text)
    .map(|item| item.as_str().to_string())
    .collect()
}

fn protected_tokens(text: &str) -> Vec<String> {
    const PROTECTED: &[&str] = &[
        "не",
        "нет",
        "никогда",
        "только",
        "сначала",
        "после",
        "not",
        "never",
        "only",
        "before",
        "after",
        "without",
    ];
    normalize(text)
        .split_whitespace()
        .filter(|word| PROTECTED.contains(word))
        .map(ToOwned::to_owned)
        .collect()
}

fn percentile(values: &[u64], percentile: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    values.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::levenshtein_items;

    #[test]
    fn word_distance_handles_insertions() {
        assert_eq!(levenshtein_items(&["one", "two"], &["one", "x", "two"]), 1);
    }
}
