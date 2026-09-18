use std::collections::BTreeMap;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
    pub context: crate::context::ContextConfig,
    pub model: ModelConfig,
    pub audio: AudioConfig,
    pub transcription: TranscriptionConfig,
    pub normalization: NormalizationConfig,
    pub vocabulary: VocabularyConfig,
    pub correction: CorrectionConfig,
    pub history: HistoryConfig,
    pub workspaces: WorkspaceConfig,
    pub worker: WorkerConfig,
}

impl Config {
    pub fn load() -> Result<Self> {
        let configured = paths::config_path();
        let legacy = paths::legacy_config_path();
        let path = if configured.exists() || !legacy.exists() {
            configured
        } else {
            legacy
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let input = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut config: Self = toml::from_str(&input)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        config.source_path = Some(path.to_path_buf());
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        self.context.validate()?;
        anyhow::ensure!(
            self.worker.startup_timeout_seconds > 0,
            "worker.startup_timeout_seconds must be positive"
        );
        anyhow::ensure!(
            self.worker.transcription_timeout_seconds > 0,
            "worker.transcription_timeout_seconds must be positive"
        );
        anyhow::ensure!(
            self.audio.sample_rate >= 8_000,
            "audio.sample_rate is too low"
        );
        anyhow::ensure!(self.audio.channels == 1, "only mono recording is supported");
        anyhow::ensure!(
            self.audio.max_seconds > 0,
            "audio.max_seconds must be positive"
        );
        anyhow::ensure!(
            self.transcription.beam_size > 0,
            "beam_size must be positive"
        );
        anyhow::ensure!(
            self.vocabulary.max_terms > 0,
            "vocabulary.max_terms must be positive"
        );
        anyhow::ensure!(
            self.vocabulary.max_files > 0,
            "vocabulary.max_files must be positive"
        );
        anyhow::ensure!(
            self.vocabulary.max_bytes > 0,
            "vocabulary.max_bytes must be positive"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.correction.min_word_probability),
            "correction.min_word_probability must be between 0 and 1"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.correction.low_confidence_word_ratio),
            "correction.low_confidence_word_ratio must be between 0 and 1"
        );
        anyhow::ensure!(
            self.correction.timeout_seconds > 0,
            "correction.timeout_seconds must be positive"
        );
        anyhow::ensure!(
            self.correction.max_edits > 0,
            "correction.max_edits must be positive"
        );
        anyhow::ensure!(
            self.correction.max_chunks > 0,
            "correction.max_chunks must be positive"
        );
        anyhow::ensure!(
            self.correction.managed.idle_seconds > 0,
            "correction.managed.idle_seconds must be positive"
        );
        anyhow::ensure!(
            self.correction.managed.context_tokens >= 512,
            "correction.managed.context_tokens must be at least 512"
        );
        anyhow::ensure!(
            !self.correction.managed.gpu_layers.trim().is_empty(),
            "correction.managed.gpu_layers must not be empty"
        );
        anyhow::ensure!(
            self.history.retention_days > 0,
            "history.retention_days must be positive"
        );
        anyhow::ensure!(
            self.history.max_entries > 0,
            "history.max_entries must be positive"
        );
        if self.correction.enabled {
            let url = reqwest::Url::parse(&self.correction.endpoint)
                .context("correction.endpoint is not a valid URL")?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https"),
                "correction.endpoint must use http or https"
            );
            let host = url
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("correction.endpoint has no host"))?;
            let loopback = host.eq_ignore_ascii_case("localhost")
                || IpAddr::from_str(host.trim_matches(['[', ']']))
                    .is_ok_and(|address| address.is_loopback());
            if self.correction.backend == CorrectionBackend::Managed {
                anyhow::ensure!(
                    loopback,
                    "managed correction.endpoint must use a loopback address"
                );
            } else if !self.correction.allow_remote {
                anyhow::ensure!(
                    loopback,
                    "correction.endpoint must be loopback unless allow_remote=true"
                );
            }
        }
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.workspaces.fuzzy_threshold),
            "workspaces.fuzzy_threshold must be between 0 and 1"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.workspaces.fuzzy_margin),
            "workspaces.fuzzy_margin must be between 0 and 1"
        );
        Ok(())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut config: Self = toml::from_str(&input)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        config.source_path = Some(path.to_path_buf());
        config.validate()?;
        Ok(config)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub name: String,
    pub device: String,
    pub compute_type: String,
    pub cpu_fallback: bool,
    pub cpu_compute_type: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: "small".into(),
            device: "cuda".into(),
            compute_type: "int8_float16".into(),
            cpu_fallback: true,
            cpu_compute_type: "int8".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub target: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub max_seconds: u64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            target: None,
            sample_rate: 16_000,
            channels: 1,
            max_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub beam_size: usize,
    pub multilingual: bool,
    pub vad_filter: bool,
    pub min_silence_duration_ms: u64,
    pub hotwords: Vec<String>,
    pub initial_prompt: String,
    pub condition_on_previous_text: bool,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            beam_size: 3,
            multilingual: true,
            vad_filter: true,
            min_silence_duration_ms: 500,
            hotwords: vec![
                "Herdr".into(),
                "Codex".into(),
                "Rust".into(),
                "Tokio".into(),
                "Redis".into(),
                "ClickHouse".into(),
                "PostgreSQL".into(),
                "SOCKS5".into(),
            ],
            initial_prompt: "Техническая software-development диктовка на русском и английском. Сохраняй названия технологий и идентификаторы.".into(),
            condition_on_previous_text: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NormalizationConfig {
    pub replacements: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VocabularyConfig {
    pub enabled: bool,
    pub include_identifiers: bool,
    pub max_terms: usize,
    pub max_files: usize,
    pub max_bytes: usize,
}

impl Default for VocabularyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            include_identifiers: true,
            max_terms: 96,
            max_files: 2_000,
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionBackend {
    #[default]
    Managed,
    Openai,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionTrigger {
    #[default]
    Uncertain,
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CorrectionConfig {
    pub enabled: bool,
    pub backend: CorrectionBackend,
    pub trigger: CorrectionTrigger,
    pub endpoint: String,
    pub model: String,
    pub timeout_seconds: u64,
    pub min_word_probability: f64,
    pub min_segment_avg_logprob: f64,
    pub low_confidence_word_ratio: f64,
    pub max_edits: usize,
    pub max_chunks: usize,
    pub allow_remote: bool,
    pub managed: ManagedCorrectionConfig,
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: CorrectionBackend::Managed,
            trigger: CorrectionTrigger::Uncertain,
            endpoint: "http://127.0.0.1:18118/v1".into(),
            model: "qwen3-4b".into(),
            timeout_seconds: 30,
            min_word_probability: 0.55,
            min_segment_avg_logprob: -0.75,
            low_confidence_word_ratio: 0.20,
            max_edits: 8,
            max_chunks: 4,
            allow_remote: false,
            managed: ManagedCorrectionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedCorrectionConfig {
    pub idle_seconds: u64,
    pub context_tokens: usize,
    pub gpu_layers: String,
    pub binary: Option<PathBuf>,
    pub model_path: Option<PathBuf>,
}

impl Default for ManagedCorrectionConfig {
    fn default() -> Self {
        Self {
            idle_seconds: 120,
            context_tokens: 2_048,
            gpu_layers: "all".into(),
            binary: None,
            model_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub enabled: bool,
    pub retention_days: u64,
    pub max_entries: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 7,
            max_entries: 200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub fuzzy_threshold: f64,
    pub fuzzy_margin: f64,
    pub aliases: BTreeMap<String, Vec<String>>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            fuzzy_threshold: 0.90,
            fuzzy_margin: 0.08,
            aliases: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerConfig {
    pub python: Option<String>,
    pub startup_timeout_seconds: u64,
    pub transcription_timeout_seconds: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            python: None,
            startup_timeout_seconds: 240,
            transcription_timeout_seconds: 240,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, CorrectionBackend};

    #[test]
    fn example_config_is_valid() {
        let config: Config = toml::from_str(include_str!("../config.example.toml"))
            .expect("example config should parse");
        config.validate().expect("example config should validate");
    }

    #[test]
    fn remote_corrector_requires_explicit_opt_in() {
        let mut config = Config::default();
        config.correction.enabled = true;
        config.correction.endpoint = "https://example.com/v1".into();
        assert!(config.validate().is_err());
        config.correction.allow_remote = true;
        config.correction.backend = CorrectionBackend::Openai;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn managed_corrector_is_always_loopback() {
        let mut config = Config::default();
        config.correction.enabled = true;
        config.correction.endpoint = "http://192.0.2.1:18118/v1".into();
        config.correction.allow_remote = true;
        assert!(config.validate().is_err());
    }

    #[test]
    fn enabled_corrector_always_validates_the_endpoint() {
        let mut config = Config::default();
        config.correction.enabled = true;
        config.correction.backend = CorrectionBackend::Openai;
        config.correction.allow_remote = true;
        config.correction.endpoint = "not a URL".into();
        assert!(config.validate().is_err());
    }
}
