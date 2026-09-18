//! User-owned recognition hints. Repository contents never become configuration.
use std::collections::{BTreeMap, HashSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{config::Config, paths};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Guidance {
    pub text: String,
    pub files: Vec<PathBuf>,
    pub hotwords: Vec<String>,
    /// Whisper language code, or "auto" to restore automatic detection.
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    pub default_profile: Option<String>,
    pub text: String,
    pub files: Vec<PathBuf>,
    pub hotwords: Vec<String>,
    pub language: Option<String>,
    pub profiles: BTreeMap<String, Guidance>,
    pub bindings: Vec<ContextBinding>,
    pub max_chars: usize,
    pub max_prompt_chars: usize,
    pub max_file_bytes: usize,
    pub max_hotwords: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            default_profile: None,
            text: String::new(),
            files: Vec::new(),
            hotwords: Vec::new(),
            language: None,
            profiles: BTreeMap::new(),
            bindings: Vec::new(),
            max_chars: 4_000,
            max_prompt_chars: 800,
            max_file_bytes: 65_536,
            max_hotwords: 128,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBinding {
    pub path: PathBuf,
    pub profile: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecognitionHints {
    pub profile: Option<String>,
    pub text: String,
    pub hotwords: Vec<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedContext {
    pub profile: Option<String>,
    pub text: String,
    pub initial_prompt: String,
    pub hotwords: Vec<String>,
    pub language: Option<String>,
    pub truncated: bool,
}

impl ContextConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=32_000).contains(&self.max_chars),
            "context.max_chars must be 1..32000"
        );
        ensure!(
            (1..=8_000).contains(&self.max_prompt_chars),
            "context.max_prompt_chars must be 1..8000"
        );
        ensure!(
            (1..=1_048_576).contains(&self.max_file_bytes),
            "context.max_file_bytes must be 1..1048576"
        );
        ensure!(
            (1..=512).contains(&self.max_hotwords),
            "context.max_hotwords must be 1..512"
        );
        validate_language(self.language.as_deref())?;
        if let Some(profile) = &self.default_profile {
            ensure!(
                self.profiles.contains_key(profile),
                "unknown default context profile: {profile}"
            );
        }
        for (name, profile) in &self.profiles {
            ensure!(
                !name.trim().is_empty(),
                "context profile name cannot be empty"
            );
            validate_language(profile.language.as_deref())?;
        }
        for binding in &self.bindings {
            ensure!(
                !binding.path.as_os_str().is_empty(),
                "context binding path cannot be empty"
            );
            ensure!(
                self.profiles.contains_key(&binding.profile),
                "unknown bound context profile: {}",
                binding.profile
            );
        }
        Ok(())
    }
}

pub fn resolve(
    config: &Config,
    cwd: Option<&str>,
    hints: &RecognitionHints,
) -> Result<ResolvedContext> {
    let settings = &config.context;
    let config_path = config
        .source_path
        .clone()
        .unwrap_or_else(paths::config_path);
    let base = config_path.parent().unwrap_or(Path::new("."));
    let bound = cwd.and_then(|cwd| {
        let cwd = Path::new(cwd).canonicalize().ok()?;
        settings
            .bindings
            .iter()
            .filter_map(|binding| {
                let path = absolute_path(&binding.path, base).canonicalize().ok()?;
                cwd.starts_with(&path)
                    .then_some((path.components().count(), &binding.profile))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, profile)| profile)
    });
    let selected = hints
        .profile
        .as_ref()
        .or(bound)
        .or(settings.default_profile.as_ref());
    let profile = selected
        .map(|name| {
            settings
                .profiles
                .get(name)
                .with_context(|| format!("unknown context profile: {name}"))
        })
        .transpose()?;
    validate_language(hints.language.as_deref())?;
    let language = hints
        .language
        .as_ref()
        .or_else(|| profile.and_then(|p| p.language.as_ref()))
        .or(settings.language.as_ref())
        .filter(|language| language.as_str() != "auto")
        .cloned();

    // Most specific information comes first so it survives bounded prompt truncation.
    let mut pieces = vec![hints.text.clone()];
    if let Some(profile) = profile {
        pieces.push(profile.text.clone());
        for file in &profile.files {
            pieces.push(read_context_file(
                &absolute_path(file, base),
                settings.max_file_bytes,
            )?);
        }
    }
    pieces.push(settings.text.clone());
    for file in &settings.files {
        pieces.push(read_context_file(
            &absolute_path(file, base),
            settings.max_file_bytes,
        )?);
    }
    let mut truncated = false;
    let text = bounded(
        &pieces
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        settings.max_chars,
        &mut truncated,
    );
    let prompt = [text.as_str(), config.transcription.initial_prompt.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let initial_prompt = bounded(&prompt, settings.max_prompt_chars, &mut truncated);
    let mut hotwords = hints.hotwords.clone();
    if let Some(profile) = profile {
        hotwords.extend(profile.hotwords.clone());
    }
    hotwords.extend(settings.hotwords.clone());
    hotwords.extend(config.transcription.hotwords.clone());
    limit_hotwords(&mut hotwords, settings.max_hotwords, &mut truncated);
    Ok(ResolvedContext {
        profile: selected.cloned(),
        text,
        initial_prompt,
        hotwords,
        language,
        truncated,
    })
}

pub fn read_context_file(path: &Path, max_bytes: usize) -> Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("cannot read context file {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "context must be a regular file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= max_bytes,
        "context file exceeds {max_bytes} bytes: {}",
        path.display()
    );
    String::from_utf8(bytes)
        .with_context(|| format!("context file must be UTF-8: {}", path.display()))
}

fn absolute_path(path: &Path, base: &Path) -> PathBuf {
    if let Ok(suffix) = path.strip_prefix("~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(suffix);
    }
    base.join(path)
}

fn validate_language(language: Option<&str>) -> Result<()> {
    if let Some(language) = language {
        ensure!(
            language == "auto"
                || ((2..=3).contains(&language.len())
                    && language.bytes().all(|c| c.is_ascii_lowercase())),
            "language must be a lowercase Whisper language code (ru, en, uk, …) or auto"
        );
    }
    Ok(())
}

fn bounded(text: &str, max_chars: usize, truncated: &mut bool) -> String {
    if let Some((end, _)) = text.char_indices().nth(max_chars) {
        *truncated = true;
        text[..end].to_string()
    } else {
        text.to_string()
    }
}

pub fn limit_hotwords(words: &mut Vec<String>, max: usize, truncated: &mut bool) {
    let mut seen = HashSet::new();
    for word in words.iter_mut() {
        *word = word.trim().to_owned();
    }
    words.retain(|word| !word.is_empty() && seen.insert(word.to_lowercase()));
    for word in words.iter_mut() {
        *word = bounded(word, 120, truncated);
    }
    if words.len() > max {
        *truncated = true;
        words.truncate(max);
    }
}
