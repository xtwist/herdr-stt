use crate::{
    config::Config,
    herdr,
    intent::{Intent, WorkspaceResolution, parse, resolve_workspace},
    protocol::Transcript,
    tray::{Report, Status},
};
use anyhow::{Result, anyhow};

pub(super) fn route_transcript(transcript: Transcript) -> Result<Report> {
    let config = Config::load()?;
    let context = &transcript.context;
    let (title, detail) = match parse(transcript.deterministic_text.trim()) {
        Intent::Empty => return Ok(Report::no_speech()),
        Intent::Insert(_) => {
            herdr::insert_text(context, transcript.text.trim())?;
            let detail = match transcript.correction.status.as_str() {
                "fallback" => "Qwen unavailable · Whisper text inserted",
                "applied" => "Qwen corrections applied · review before sending",
                _ => "Review before sending",
            };
            ("Voice text inserted", Some(detail.into()))
        }
        Intent::SendPrompt(text) => {
            herdr::send_prompt(context, &text)?;
            ("Prompt sent", None)
        }
        Intent::SubmitBuffer => {
            herdr::submit_buffer(context)?;
            ("Prompt sent", None)
        }
        Intent::EnablePlanMode(text) => {
            let changed = herdr::enable_plan_mode(context)?;
            let detail = if text.is_empty() {
                None
            } else {
                herdr::insert_text(context, &text)?;
                Some("Voice text inserted · review before sending".into())
            };
            let title = if changed {
                "Plan mode enabled"
            } else {
                "Plan mode already enabled"
            };
            (title, detail)
        }
        Intent::FocusWorkspace(query) => {
            let workspaces = herdr::list_workspaces(context)?;
            match resolve_workspace(&query, &workspaces, &config.workspaces) {
                WorkspaceResolution::Match(workspace) => {
                    herdr::focus_workspace(context, &workspace.workspace_id)?;
                    ("Workspace focused", Some(workspace.label))
                }
                WorkspaceResolution::Ambiguous(candidates) => {
                    let candidates = candidates
                        .iter()
                        .map(|workspace| format!("{} ({})", workspace.label, workspace.number))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(anyhow!("workspace name is ambiguous: {candidates}"));
                }
                WorkspaceResolution::NotFound => {
                    return Err(anyhow!("workspace not found: {query}"));
                }
            }
        }
    };
    Ok(Report::new(Status::Done, title, detail))
}
