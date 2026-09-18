use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::HistoryConfig;
use crate::paths;
use crate::protocol::Transcript;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: u64,
    pub transcript: Transcript,
}

pub fn append(config: &HistoryConfig, transcript: &Transcript) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }
    let path = paths::history_path();
    let mut entries = read_from(&path)?;
    entries.push(HistoryEntry {
        timestamp: now_seconds(),
        transcript: transcript.clone(),
    });
    rotate(&mut entries, config);
    write_atomic(&path, &entries)
}

pub fn read() -> Result<Vec<HistoryEntry>> {
    read_from(&paths::history_path())
}

pub fn clear() -> Result<()> {
    let path = paths::history_path();
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn read_from(path: &Path) -> Result<Vec<HistoryEntry>> {
    let Ok(input) = fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    Ok(input
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn rotate(entries: &mut Vec<HistoryEntry>, config: &HistoryConfig) {
    let oldest = now_seconds().saturating_sub(config.retention_days.saturating_mul(86_400));
    entries.retain(|entry| entry.timestamp >= oldest);
    if entries.len() > config.max_entries {
        entries.drain(0..entries.len() - config.max_entries);
    }
}

fn write_atomic(path: &Path, entries: &[HistoryEntry]) -> Result<()> {
    let parent = path.parent().context("history path has no parent")?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let temporary = path.with_extension(format!("jsonl.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    for entry in entries {
        serde_json::to_writer(&mut file, entry)?;
        file.write_all(b"\n")?;
    }
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{HistoryEntry, rotate};
    use crate::config::HistoryConfig;
    use crate::protocol::Transcript;

    #[test]
    fn rotation_honors_entry_limit() {
        let transcript = Transcript::default_for_test("test");
        let mut entries = (0..5)
            .map(|timestamp| HistoryEntry {
                timestamp: u64::MAX - timestamp,
                transcript: transcript.clone(),
            })
            .collect();
        rotate(
            &mut entries,
            &HistoryConfig {
                enabled: true,
                retention_days: u64::MAX / 86_400,
                max_entries: 2,
            },
        );
        assert_eq!(entries.len(), 2);
    }
}
