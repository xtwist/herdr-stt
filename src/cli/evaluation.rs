use super::{HistoryCommands, ensure_daemon, print_json};
use crate::{
    benchmark::{self, BenchmarkCase},
    client,
    config::Config,
    history,
    protocol::Transcript,
    vocabulary,
};
use anyhow::{Context, Result};
use serde_json::json;
use std::{fs, path::PathBuf};

pub(super) fn history_command(command: HistoryCommands) -> Result<()> {
    match command {
        HistoryCommands::List { limit, json } => {
            let entries = history::read()?;
            let start = entries.len().saturating_sub(limit);
            if json {
                print_json(&entries[start..].to_vec())
            } else {
                for entry in &entries[start..] {
                    println!(
                        "{}\t{}\t{}",
                        entry.timestamp,
                        entry.transcript.correction.status,
                        entry.transcript.text.replace('\n', " ")
                    );
                }
                Ok(())
            }
        }
        HistoryCommands::Export { path } => {
            let entries = history::read()?;
            let mut output = String::new();
            for entry in entries {
                output.push_str(&serde_json::to_string(&entry)?);
                output.push('\n');
            }
            fs::write(&path, output)
                .with_context(|| format!("failed to write {}", path.display()))?;
            println!("Exported transcript history to {}", path.display());
            Ok(())
        }
        HistoryCommands::Clear => {
            history::clear()?;
            println!("Transcript history cleared");
            Ok(())
        }
    }
}

pub(super) async fn benchmark_command(corpus: PathBuf, output: Option<PathBuf>) -> Result<()> {
    ensure_daemon().await?;
    let input = fs::read_to_string(&corpus)
        .with_context(|| format!("failed to read {}", corpus.display()))?;
    let config = Config::load()?;
    let mut results = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let case: BenchmarkCase = serde_json::from_str(line)
            .with_context(|| format!("invalid JSONL at line {}", index + 1))?;
        let audio = corpus
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(&case.audio)
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", case.audio))?;
        let cwd = case
            .cwd
            .clone()
            .or_else(|| audio.parent().map(|path| path.display().to_string()));
        let project_vocabulary =
            vocabulary::collect(cwd.as_deref(), &config.vocabulary).unwrap_or_default();
        let mut hotwords = case.hotwords.clone();
        hotwords.extend(project_vocabulary.terms.iter().cloned());
        let transcript: Transcript = client::call(
            "transcribe_file",
            json!({
                "path": audio,
                "context": {"cwd": cwd},
                "hotwords": hotwords,
                "vocabulary": project_vocabulary.terms,
                "record_history": false,
            }),
        )
        .await?;
        results.push((case, transcript));
    }
    let report = benchmark::build_report(&results);
    let rendered = serde_json::to_string_pretty(&report)?;
    if let Some(output) = output {
        fs::write(&output, format!("{rendered}\n"))
            .with_context(|| format!("failed to write {}", output.display()))?;
    }
    println!("{rendered}");
    Ok(())
}
