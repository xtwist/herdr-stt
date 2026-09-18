mod backend;
mod validation;
use backend::{ensure_backend_ready, request_edits};
use validation::apply_validated_edits;

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use strsim::normalized_levenshtein;

use crate::config::{CorrectionBackend, CorrectionConfig, CorrectionTrigger};
use crate::protocol::{AsrQuality, CorrectionReport};
use crate::vocabulary::ProjectVocabulary;
use crate::worker::WorkerTranscript;

pub const LLAMA_CPP_TAG: &str = "b10715";
pub const LLAMA_CPP_ARCHIVE: &str = "llama-b10715-bin-ubuntu-vulkan-x64.tar.gz";
pub const LLAMA_CPP_SHA256: &str =
    "1246c764f630f1cfc7c0921353a9719603c5d9ccfa7ced621bac216ffd9b2d87";
pub const LLAMA_CPP_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b10715/llama-b10715-bin-ubuntu-vulkan-x64.tar.gz";
pub const QWEN_FILE: &str = "Qwen3-4B-Q4_K_M.gguf";
pub const QWEN_SHA256: &str = "7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5";
pub const QWEN_URL: &str =
    "https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q4_K_M.gguf";

#[derive(Debug, Clone)]
pub struct CorrectionOutcome {
    pub text: String,
    pub quality: AsrQuality,
    pub report: CorrectionReport,
}

#[derive(Debug, Deserialize)]
struct PatchResponse {
    #[serde(default)]
    edits: Vec<TextEdit>,
}

#[derive(Debug, Deserialize)]
struct TextEdit {
    source: String,
    replacement: String,
}

pub fn asr_quality(transcript: &WorkerTranscript, config: &CorrectionConfig) -> AsrQuality {
    let min_segment = transcript
        .segments
        .iter()
        .map(|segment| segment.avg_logprob)
        .reduce(f64::min);
    let probabilities = transcript
        .segments
        .iter()
        .flat_map(|segment| segment.words.iter().map(|word| word.probability))
        .collect::<Vec<_>>();
    let min_word = probabilities.iter().copied().reduce(f64::min);
    let low_count = probabilities
        .iter()
        .filter(|probability| **probability < config.min_word_probability)
        .count();
    AsrQuality {
        min_segment_avg_logprob: min_segment,
        min_word_probability: min_word,
        low_confidence_word_ratio: (!probabilities.is_empty())
            .then_some(low_count as f64 / probabilities.len() as f64),
        word_count: probabilities.len(),
    }
}

pub async fn correct(
    deterministic_text: &str,
    transcript: &WorkerTranscript,
    vocabulary: &ProjectVocabulary,
    config: &CorrectionConfig,
    allow_correction: bool,
    protect_intent: bool,
    reference_context: &str,
) -> CorrectionOutcome {
    let quality = asr_quality(transcript, config);
    if !config.enabled {
        return outcome(deterministic_text, quality, "disabled", Vec::new());
    }
    if !allow_correction {
        return outcome(deterministic_text, quality, "skipped_command", Vec::new());
    }

    let reasons = trigger_reasons(deterministic_text, &quality, vocabulary, config);
    if config.trigger == CorrectionTrigger::Uncertain && reasons.is_empty() {
        return outcome(deterministic_text, quality, "not_needed", reasons);
    }

    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(config.timeout_seconds),
        correct_inner(
            deterministic_text,
            vocabulary,
            config,
            protect_intent,
            reference_context,
        ),
    )
    .await
    .context("correction deadline exceeded")
    .and_then(|result| result);
    match result {
        Ok((text, applied, rejected)) => {
            let status = if applied == 0 { "unchanged" } else { "applied" };
            CorrectionOutcome {
                text,
                quality,
                report: CorrectionReport {
                    status: status.into(),
                    backend: Some(backend_name(config.backend).into()),
                    trigger_reasons: reasons,
                    edits_applied: applied,
                    edits_rejected: rejected,
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                    detail: None,
                },
            }
        }
        Err(error) => CorrectionOutcome {
            text: deterministic_text.into(),
            quality,
            report: CorrectionReport {
                status: "fallback".into(),
                backend: Some(backend_name(config.backend).into()),
                trigger_reasons: reasons,
                edits_applied: 0,
                edits_rejected: 0,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                detail: Some(sanitize_error(&error)),
            },
        },
    }
}

fn outcome(
    text: &str,
    quality: AsrQuality,
    status: &str,
    reasons: Vec<String>,
) -> CorrectionOutcome {
    CorrectionOutcome {
        text: text.into(),
        quality,
        report: CorrectionReport {
            status: status.into(),
            trigger_reasons: reasons,
            ..CorrectionReport::default()
        },
    }
}

fn trigger_reasons(
    text: &str,
    quality: &AsrQuality,
    vocabulary: &ProjectVocabulary,
    config: &CorrectionConfig,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if config.trigger == CorrectionTrigger::Always {
        reasons.push("always".into());
    }
    if quality
        .min_segment_avg_logprob
        .is_some_and(|value| value < config.min_segment_avg_logprob)
    {
        reasons.push("segment_logprob".into());
    }
    if quality
        .low_confidence_word_ratio
        .is_some_and(|value| value >= config.low_confidence_word_ratio)
    {
        reasons.push("word_probability".into());
    }
    if suspicious_vocabulary_match(text, vocabulary) {
        reasons.push("technical_vocabulary".into());
    }
    reasons
}

fn suspicious_vocabulary_match(text: &str, vocabulary: &ProjectVocabulary) -> bool {
    let normalized = normalize_words(text);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    vocabulary.terms.iter().any(|term| {
        let target = normalize_words(&split_identifier(term));
        let count = target.split_whitespace().count().clamp(1, 3);
        words.windows(count).any(|window| {
            let candidate = window.join(" ");
            candidate != target
                && (normalized_levenshtein(&candidate, &target) >= 0.78
                    || normalized_levenshtein(
                        &phonetic_normalize(&candidate),
                        &phonetic_normalize(&target),
                    ) >= 0.78)
        })
    })
}

async fn correct_inner(
    text: &str,
    vocabulary: &ProjectVocabulary,
    config: &CorrectionConfig,
    protect_intent: bool,
    reference_context: &str,
) -> Result<(String, usize, usize)> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(config.timeout_seconds))
        .build()?;
    ensure_backend_ready(&client, config).await?;
    let chunks = text_chunks(text, config.max_chunks);
    let mut edits = Vec::new();
    for chunk in chunks {
        let response =
            request_edits(&client, &chunk, vocabulary, config, reference_context).await?;
        edits.extend(response.edits);
        if edits.len() >= config.max_edits {
            edits.truncate(config.max_edits);
            break;
        }
    }
    Ok(apply_validated_edits(
        text,
        edits,
        vocabulary,
        config.max_edits,
        protect_intent,
    ))
}

fn text_chunks(text: &str, max_chunks: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        current.push(character);
        if current.len() >= 3_000 {
            chunks.push(std::mem::take(&mut current));
            if chunks.len() >= max_chunks {
                return chunks;
            }
        }
    }
    if !current.is_empty() && chunks.len() < max_chunks {
        chunks.push(current);
    }
    chunks
}

fn split_identifier(term: &str) -> String {
    let mut output = String::new();
    let mut lower = false;
    for character in term.chars() {
        if matches!(character, '_' | '-' | '.') {
            output.push(' ');
            lower = false;
        } else {
            if character.is_ascii_uppercase() && lower {
                output.push(' ');
            }
            lower = character.is_ascii_lowercase() || character.is_ascii_digit();
            output.push(character);
        }
    }
    output
}

fn normalize_words(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '\'' {
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

fn phonetic_normalize(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            let replacement = match character {
                'а' => "a",
                'б' => "b",
                'в' => "v",
                'г' => "g",
                'д' => "d",
                'е' | 'э' => "e",
                'ё' => "yo",
                'ж' => "zh",
                'з' => "z",
                'и' => "i",
                'й' => "y",
                'к' => "k",
                'л' => "l",
                'м' => "m",
                'н' => "n",
                'о' => "o",
                'п' => "p",
                'р' => "r",
                'с' => "s",
                'т' => "t",
                'у' => "u",
                'ф' => "f",
                'х' => "h",
                'ц' => "ts",
                'ч' => "ch",
                'ш' | 'щ' => "sh",
                'ы' => "y",
                'ю' => "yu",
                'я' => "ya",
                'ь' | 'ъ' => "",
                _ => return character.to_string().chars().collect::<Vec<_>>(),
            };
            replacement.chars().collect::<Vec<_>>()
        })
        .collect()
}

fn backend_name(backend: CorrectionBackend) -> &'static str {
    match backend {
        CorrectionBackend::Managed => "managed",
        CorrectionBackend::Openai => "openai",
    }
}

fn sanitize_error(error: &anyhow::Error) -> String {
    let message = error.to_string();
    if message.chars().count() > 160 {
        format!("{}…", message.chars().take(160).collect::<String>())
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::backend::managed_response_format;
    use super::validation::{preserve_numbers, preserve_protected_words};
    use super::{TextEdit, apply_validated_edits, correct};
    use crate::config::{CorrectionBackend, CorrectionConfig, CorrectionTrigger};
    use crate::vocabulary::ProjectVocabulary;
    use crate::worker::WorkerTranscript;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn rejects_changes_to_numbers_and_negations() {
        assert!(!preserve_numbers("порт 8080", "порт 8081"));
        assert!(!preserve_protected_words("не меняй код", "меняй код"));
    }

    #[test]
    fn managed_format_uses_llama_cpp_schema_contract() {
        let format = managed_response_format();
        assert_eq!(format["type"], "json_object");
        assert_eq!(
            format["schema"]["properties"]["edits"]["items"]["required"],
            json!(["source", "replacement"])
        );
    }

    #[test]
    fn applies_known_vocabulary_patch() {
        let vocabulary = ProjectVocabulary {
            terms: vec!["UsernameParser".into()],
            ..ProjectVocabulary::default()
        };
        let (text, applied, rejected) = apply_validated_edits(
            "проверь юзернейм парсер",
            vec![TextEdit {
                source: "юзернейм парсер".into(),
                replacement: "UsernameParser".into(),
            }],
            &vocabulary,
            8,
            true,
        );
        assert_eq!(text, "проверь UsernameParser");
        assert_eq!((applied, rejected), (1, 0));
    }

    #[tokio::test]
    async fn openai_backend_applies_patch_and_commands_are_never_corrected() {
        let (endpoint, server) = fake_server(
            json!({"edits":[{"source":"юзернейм парсер","replacement":"UsernameParser"}]})
                .to_string(),
        )
        .await;
        let config = CorrectionConfig {
            enabled: true,
            backend: CorrectionBackend::Openai,
            trigger: CorrectionTrigger::Always,
            endpoint,
            ..CorrectionConfig::default()
        };
        let transcript = worker_transcript("проверь юзернейм парсер");
        let vocabulary = ProjectVocabulary::from_terms(vec!["UsernameParser".into()]);
        let outcome = correct(
            &transcript.text,
            &transcript,
            &vocabulary,
            &config,
            true,
            true,
            "",
        )
        .await;
        server.await.expect("fake server task");
        assert_eq!(outcome.text, "проверь UsernameParser");
        assert_eq!(outcome.report.status, "applied");

        let skipped = correct(
            "отправь промпт ничего не меняй",
            &worker_transcript("отправь промпт ничего не меняй"),
            &vocabulary,
            &config,
            false,
            true,
            "",
        )
        .await;
        assert_eq!(skipped.report.status, "skipped_command");
    }

    #[tokio::test]
    async fn rejects_patch_that_creates_an_intent() {
        let (endpoint, server) = fake_server(
            json!({"edits":[{"source":"проверь код","replacement":"отправь промпт проверь код"}]})
                .to_string(),
        )
        .await;
        let config = CorrectionConfig {
            enabled: true,
            backend: CorrectionBackend::Openai,
            trigger: CorrectionTrigger::Always,
            endpoint,
            ..CorrectionConfig::default()
        };
        let transcript = worker_transcript("проверь код");
        let outcome = correct(
            &transcript.text,
            &transcript,
            &ProjectVocabulary::default(),
            &config,
            true,
            true,
            "",
        )
        .await;
        server.await.expect("fake server task");
        assert_eq!(outcome.text, "проверь код");
        assert_eq!(outcome.report.edits_applied, 0);
    }

    #[tokio::test]
    async fn invalid_patch_response_falls_back_with_context() {
        let (endpoint, server) = fake_server("not valid JSON".into()).await;
        let config = CorrectionConfig {
            enabled: true,
            backend: CorrectionBackend::Openai,
            trigger: CorrectionTrigger::Always,
            endpoint,
            ..CorrectionConfig::default()
        };
        let transcript = worker_transcript("проверь код");
        let result = correct(
            &transcript.text,
            &transcript,
            &ProjectVocabulary::default(),
            &config,
            true,
            true,
            "Справочная информация",
        )
        .await;
        server.await.unwrap();
        assert_eq!(result.text, transcript.text);
        assert_eq!(result.report.status, "fallback");
    }

    #[test]
    fn known_vocabulary_cannot_replace_existing_code_identifiers() {
        let (text, applied, rejected) = apply_validated_edits(
            "проверь ExistingParser",
            vec![TextEdit {
                source: "ExistingParser".into(),
                replacement: "DifferentParser".into(),
            }],
            &ProjectVocabulary::from_terms(vec!["DifferentParser".into()]),
            8,
            true,
        );
        assert_eq!(text, "проверь ExistingParser");
        assert_eq!((applied, rejected), (0, 1));
    }

    #[test]
    fn vocabulary_patch_cannot_swallow_a_neighboring_verb() {
        let (text, applied, rejected) = apply_validated_edits(
            "проверь юзернейм парсер, не меняй порт 8080",
            vec![TextEdit {
                source: "проверь юзернейм парсер".into(),
                replacement: "UsernameParser".into(),
            }],
            &ProjectVocabulary::from_terms(vec!["UsernameParser".into()]),
            8,
            true,
        );
        assert_eq!(text, "проверь юзернейм парсер, не меняй порт 8080");
        assert_eq!((applied, rejected), (0, 1));
    }

    fn worker_transcript(text: &str) -> WorkerTranscript {
        WorkerTranscript {
            context_truncated: false,
            text: text.into(),
            language: Some("ru".into()),
            language_probability: Some(0.9),
            duration_seconds: 1.0,
            duration_after_vad_seconds: Some(1.0),
            segments: Vec::new(),
        }
    }

    async fn fake_server(content: String) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake server");
        let address = listener.local_addr().expect("fake server address");
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = vec![0u8; 16 * 1024];
            let _ = stream.read(&mut request).await;
            let body = json!({"choices":[{"message":{"content":content}}]}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        (format!("http://{address}/v1"), task)
    }
}
