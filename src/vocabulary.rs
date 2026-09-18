use std::collections::{BTreeMap, HashSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::VocabularyConfig;
use crate::paths;

#[derive(Debug, Clone, Default)]
pub struct ProjectVocabulary {
    pub root: Option<PathBuf>,
    pub terms: Vec<String>,
    pub aliases: BTreeMap<String, String>,
}

impl ProjectVocabulary {
    pub fn from_terms(terms: Vec<String>) -> Self {
        let aliases = terms
            .iter()
            .filter_map(|term| {
                let alias = split_identifier(term);
                (!alias.eq_ignore_ascii_case(term) && alias.contains(' '))
                    .then(|| (alias.to_lowercase(), term.clone()))
            })
            .collect();
        Self {
            root: None,
            terms,
            aliases,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    key: String,
    terms: Vec<String>,
}

pub fn collect(cwd: Option<&str>, config: &VocabularyConfig) -> Result<ProjectVocabulary> {
    if !config.enabled {
        return Ok(ProjectVocabulary::default());
    }
    let Some(cwd) = cwd.filter(|value| !value.is_empty()) else {
        return Ok(ProjectVocabulary::default());
    };
    let Some(root) = git_output(cwd, ["rev-parse", "--show-toplevel"])? else {
        return Ok(ProjectVocabulary::default());
    };
    let root = PathBuf::from(root);
    let key = cache_key(&root, config)?;
    let cache_path = cache_path(&root);
    if let Ok(input) = fs::read_to_string(&cache_path)
        && let Ok(cache) = serde_json::from_str::<CacheEntry>(&input)
        && cache.key == key
    {
        return Ok(from_terms(root, cache.terms));
    }

    let terms = scan_repository(&root, config)?;
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache = CacheEntry {
        key,
        terms: terms.clone(),
    };
    let _ = fs::write(&cache_path, serde_json::to_vec(&cache)?);
    Ok(from_terms(root, terms))
}

pub fn apply_aliases(text: &str, vocabulary: &ProjectVocabulary) -> String {
    let mut output = text.to_string();
    for (alias, term) in &vocabulary.aliases {
        if alias.trim().is_empty() {
            continue;
        }
        let Ok(pattern) = Regex::new(&format!(r"(?iu)\b{}\b", regex::escape(alias))) else {
            continue;
        };
        output = pattern
            .replace_all(&output, regex::NoExpand(term.as_str()))
            .into_owned();
    }
    output
}

fn from_terms(root: PathBuf, terms: Vec<String>) -> ProjectVocabulary {
    let mut vocabulary = ProjectVocabulary::from_terms(terms);
    vocabulary.root = Some(root);
    vocabulary
}

fn scan_repository(root: &Path, config: &VocabularyConfig) -> Result<Vec<String>> {
    let mut scores: BTreeMap<String, (String, usize)> = BTreeMap::new();
    if let Some(name) = root.file_name().and_then(|name| name.to_str()) {
        add_term(&mut scores, name, 1_000);
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .context("failed to list git-tracked files for vocabulary")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter(|path| !path.is_empty())
        .take(config.max_files)
        .collect::<Vec<_>>();

    let camel = Regex::new(r"\b[A-Z][a-z0-9]+(?:[A-Z][A-Za-z0-9]+)+\b")?;
    let snake = Regex::new(r"\b[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+\b")?;
    let manifest_key = Regex::new(r#"(?m)^\s*[\"']?([A-Za-z][A-Za-z0-9_.-]{2,})[\"']?\s*[=:]"#)?;
    let mut bytes_read = 0usize;
    for relative in files {
        let path = root.join(relative);
        for component in Path::new(relative).components() {
            if let Some(component) = component.as_os_str().to_str() {
                add_term(
                    &mut scores,
                    component.trim_end_matches(extension(component)),
                    8,
                );
            }
        }
        if bytes_read >= config.max_bytes || !is_source_or_manifest(&path) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > 512 * 1024 {
            continue;
        }
        let Ok(input) = fs::read_to_string(&path) else {
            continue;
        };
        bytes_read = bytes_read.saturating_add(input.len());
        if config.include_identifiers {
            for capture in camel.find_iter(&input).chain(snake.find_iter(&input)) {
                add_term(&mut scores, capture.as_str(), 2);
            }
        }
        if is_manifest(&path) {
            for capture in manifest_key.captures_iter(&input) {
                add_term(&mut scores, &capture[1], 20);
            }
        }
    }

    let mut ranked = scores.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    Ok(ranked
        .into_iter()
        .map(|(term, _)| term)
        .take(config.max_terms)
        .collect())
}

fn add_term(scores: &mut BTreeMap<String, (String, usize)>, raw: &str, score: usize) {
    let term = raw
        .trim_matches(|character: char| !character.is_alphanumeric() && character != '_')
        .trim();
    if term.len() < 4 || term.len() > 64 || term.chars().all(|character| character.is_ascii_digit())
    {
        return;
    }
    let key = term.to_lowercase();
    if common_terms().contains(key.as_str()) {
        return;
    }
    let entry = scores.entry(key).or_insert_with(|| (term.to_string(), 0));
    entry.1 = entry.1.saturating_add(score);
}

fn common_terms() -> HashSet<&'static str> {
    [
        "self",
        "string",
        "result",
        "option",
        "default",
        "config",
        "error",
        "value",
        "public",
        "private",
        "function",
        "return",
        "async",
        "tests",
        "test",
        "source",
        "main",
        "index",
        "package",
        "version",
        "dependencies",
    ]
    .into_iter()
    .collect()
}

fn is_source_or_manifest(path: &Path) -> bool {
    is_manifest(path)
        || matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some(
                "rs" | "go"
                    | "py"
                    | "js"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "vue"
                    | "php"
                    | "java"
                    | "kt"
                    | "swift"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "h"
                    | "hpp"
            )
        )
}

fn is_manifest(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("Cargo.toml" | "package.json" | "pyproject.toml" | "go.mod" | "composer.json")
    )
}

fn extension(value: &str) -> &str {
    value.rfind('.').map_or("", |index| &value[index..])
}

fn split_identifier(term: &str) -> String {
    let mut output = String::new();
    let mut previous_lower = false;
    for character in term.chars() {
        if character == '_' || character == '-' || character == '.' {
            if !output.ends_with(' ') {
                output.push(' ');
            }
            previous_lower = false;
            continue;
        }
        if character.is_ascii_uppercase() && previous_lower && !output.ends_with(' ') {
            output.push(' ');
        }
        previous_lower = character.is_ascii_lowercase() || character.is_ascii_digit();
        output.extend(character.to_lowercase());
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cache_key(root: &Path, config: &VocabularyConfig) -> Result<String> {
    let root_string = root.to_string_lossy();
    let head = git_output(&root_string, ["rev-parse", "HEAD"])?.unwrap_or_else(|| "no-head".into());
    let dirty = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(true);
    let bucket = if dirty { now_seconds() / 60 } else { 0 };
    Ok(format!(
        "{head}:{bucket}:{}:{}:{}:{}",
        config.include_identifiers, config.max_terms, config.max_files, config.max_bytes
    ))
}

fn cache_path(root: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    paths::vocabulary_cache_dir().join(format!("{:016x}.json", hasher.finish()))
}

fn git_output<const N: usize>(cwd: &str, args: [&str; N]) -> Result<Option<String>> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output();
    let Ok(output) = output else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::split_identifier;

    #[test]
    fn splits_common_identifier_styles() {
        assert_eq!(split_identifier("UsernameParser"), "username parser");
        assert_eq!(split_identifier("handle_HTTP"), "handle http");
    }
}
