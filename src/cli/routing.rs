use crate::{
    config::Config,
    herdr,
    intent::{Intent, WorkspaceResolution, parse, resolve_workspace},
    protocol::Transcript,
};
use anyhow::{Result, anyhow};

pub(super) async fn route_transcript(transcript: Transcript) -> Result<()> {
    let config = Config::load()?;
    let normalized = transcript.deterministic_text.trim().to_string();
    let context = transcript.context.clone();
    if normalized.trim().is_empty() {
        herdr::notify(
            &context,
            "No speech detected",
            Some("Nothing was inserted"),
            Some("request"),
        );
        return Ok(());
    }

    let result = match parse(&normalized) {
        Intent::Empty => Ok(RouteOutcome::NoSpeech),
        Intent::Insert(_) => {
            herdr::insert_text(&context, transcript.text.trim())?;
            Ok(RouteOutcome::Inserted)
        }
        Intent::SendPrompt(text) => {
            herdr::send_prompt(&context, &text)?;
            Ok(RouteOutcome::PromptSent)
        }
        Intent::SubmitBuffer => {
            herdr::submit_buffer(&context)?;
            Ok(RouteOutcome::PromptSent)
        }
        Intent::EnablePlanMode(text) => {
            let changed = herdr::enable_plan_mode(&context)?;
            let inserted = !text.is_empty();
            if inserted {
                herdr::insert_text(&context, &text)?;
            }
            Ok(RouteOutcome::PlanModeReady { changed, inserted })
        }
        Intent::FocusWorkspace(query) => {
            let workspaces = herdr::list_workspaces(&context)?;
            match resolve_workspace(&query, &workspaces, &config.workspaces) {
                WorkspaceResolution::Match(workspace) => {
                    herdr::focus_workspace(&context, &workspace.workspace_id)?;
                    Ok(RouteOutcome::WorkspaceFocused(workspace.label))
                }
                WorkspaceResolution::Ambiguous(candidates) => {
                    let candidates = candidates
                        .iter()
                        .map(|workspace| format!("{} ({})", workspace.label, workspace.number))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Err(anyhow!("workspace name is ambiguous: {candidates}"))
                }
                WorkspaceResolution::NotFound => Err(anyhow!("workspace not found: {query}")),
            }
        }
    };

    match result {
        Ok(RouteOutcome::Inserted) => {
            let body = if transcript.correction.status == "fallback" {
                "Qwen correction was unavailable; inserted deterministic Whisper text"
            } else if transcript.correction.status == "applied" {
                "Qwen corrections applied; review them, then press Enter"
            } else {
                "Review it, then press Enter"
            };
            herdr::notify(&context, "Voice text inserted", Some(body), Some("done"))
        }
        Ok(RouteOutcome::PromptSent) => herdr::notify(
            &context,
            "Prompt sent",
            Some("The target agent received the prompt"),
            Some("done"),
        ),
        Ok(RouteOutcome::WorkspaceFocused(label)) => {
            herdr::notify(&context, "Workspace focused", Some(&label), Some("done"))
        }
        Ok(RouteOutcome::PlanModeReady { changed, inserted }) => herdr::notify(
            &context,
            if changed {
                "Plan mode enabled"
            } else {
                "Plan mode already enabled"
            },
            Some(if inserted {
                "Planning prefix removed; voice text inserted without Enter"
            } else {
                "Codex will plan without changing files"
            }),
            Some("done"),
        ),
        Ok(RouteOutcome::NoSpeech) => herdr::notify(
            &context,
            "No speech detected",
            Some("Nothing was inserted"),
            Some("request"),
        ),
        Err(error) => {
            herdr::notify(
                &context,
                "Voice command failed",
                Some(&format!("{error}; recover with `herdr-stt last`")),
                Some("request"),
            );
            return Err(error);
        }
    }
    Ok(())
}

enum RouteOutcome {
    NoSpeech,
    Inserted,
    PromptSent,
    WorkspaceFocused(String),
    PlanModeReady { changed: bool, inserted: bool },
}
