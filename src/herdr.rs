use std::env;
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

use crate::intent::Workspace;
use crate::protocol::HerdrContext;

pub fn context_from_env() -> HerdrContext {
    HerdrContext {
        socket_path: env_value(&["HERDR_SOCKET_PATH"]),
        herdr_bin_path: env_value(&["HERDR_BIN_PATH"]).or_else(|| Some("herdr".into())),
        workspace_id: env_value(&["HERDR_WORKSPACE_ID", "HERDR_ACTIVE_WORKSPACE_ID"]),
        workspace_label: None,
        tab_id: env_value(&["HERDR_TAB_ID", "HERDR_ACTIVE_TAB_ID"]),
        pane_id: env_value(&["HERDR_PANE_ID", "HERDR_ACTIVE_PANE_ID"]),
        cwd: None,
    }
}

pub fn pane_cwd(context: &HerdrContext) -> Result<Option<String>> {
    let Some(pane_id) = context.pane_id.as_deref() else {
        return Ok(None);
    };
    let mut args = vec!["pane", "list"];
    if let Some(workspace_id) = context.workspace_id.as_deref() {
        args.extend(["--workspace", workspace_id]);
    }
    let output = run(context, args)?;
    ensure_success(&output, "pane list")?;
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("failed to parse `herdr pane list` output")?;
    Ok(value
        .pointer("/result/panes")
        .and_then(serde_json::Value::as_array)
        .and_then(|panes| {
            panes.iter().find(|pane| {
                pane.get("pane_id").and_then(serde_json::Value::as_str) == Some(pane_id)
            })
        })
        .and_then(|pane| {
            pane.get("foreground_cwd")
                .and_then(serde_json::Value::as_str)
                .or_else(|| pane.get("cwd").and_then(serde_json::Value::as_str))
        })
        .map(ToOwned::to_owned))
}

pub fn list_workspaces(context: &HerdrContext) -> Result<Vec<Workspace>> {
    let output = run(context, ["workspace", "list"])?;
    ensure_success(&output, "workspace list")?;
    #[derive(Deserialize)]
    struct Envelope {
        result: WorkspaceList,
    }
    #[derive(Deserialize)]
    struct WorkspaceList {
        workspaces: Vec<Workspace>,
    }
    let envelope: Envelope = serde_json::from_slice(&output.stdout)
        .context("failed to parse `herdr workspace list` output")?;
    Ok(envelope.result.workspaces)
}

pub fn insert_text(context: &HerdrContext, text: &str) -> Result<()> {
    let pane = context
        .pane_id
        .as_deref()
        .ok_or_else(|| anyhow!("recording context does not contain a pane id"))?;
    let output = run(context, ["pane", "send-text", pane, text])?;
    ensure_success(&output, "pane send-text")
}

pub fn send_prompt(context: &HerdrContext, text: &str) -> Result<()> {
    let pane = context
        .pane_id
        .as_deref()
        .ok_or_else(|| anyhow!("recording context does not contain a pane id"))?;
    let output = run(context, ["agent", "prompt", pane, text])?;
    ensure_success(&output, "agent prompt")
}

pub fn submit_buffer(context: &HerdrContext) -> Result<()> {
    let pane = context
        .pane_id
        .as_deref()
        .ok_or_else(|| anyhow!("recording context does not contain a pane id"))?;
    let get = run(context, ["agent", "get", pane])?;
    ensure_success(&get, "agent get")?;
    let value: serde_json::Value =
        serde_json::from_slice(&get.stdout).context("failed to parse `herdr agent get` output")?;
    let status = value
        .pointer("/result/agent/agent_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    if status == "blocked" {
        return Err(anyhow!(
            "target agent is blocked and cannot accept Enter safely"
        ));
    }
    let output = run(context, ["agent", "send-keys", pane, "enter"])?;
    ensure_success(&output, "agent send-keys")
}

pub fn enable_plan_mode(context: &HerdrContext) -> Result<bool> {
    let pane = context
        .pane_id
        .as_deref()
        .ok_or_else(|| anyhow!("recording context does not contain a pane id"))?;

    let get = run(context, ["agent", "get", pane])?;
    ensure_success(&get, "agent get")?;
    let value: serde_json::Value =
        serde_json::from_slice(&get.stdout).context("failed to parse `herdr agent get` output")?;
    let agent = value
        .pointer("/result/agent/agent")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    if agent != "codex" {
        return Err(anyhow!(
            "Plan mode is only supported for Codex panes; target agent is {agent}"
        ));
    }

    if plan_mode_is_visible(context, pane)? {
        return Ok(false);
    }

    let status = value
        .pointer("/result/agent/agent_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    if !matches!(status, "idle" | "done") {
        return Err(anyhow!(
            "Codex must be idle before enabling Plan mode; current status is {status}"
        ));
    }

    let output = run(context, ["agent", "send-keys", pane, "shift+tab"])?;
    ensure_success(&output, "agent send-keys shift+tab")?;
    thread::sleep(Duration::from_millis(150));

    if !plan_mode_is_visible(context, pane)? {
        return Err(anyhow!("Codex did not enter Plan mode after Shift+Tab"));
    }
    Ok(true)
}

fn plan_mode_is_visible(context: &HerdrContext, pane: &str) -> Result<bool> {
    let output = run(
        context,
        [
            "agent", "read", pane, "--source", "visible", "--format", "text", "--lines", "8",
        ],
    )?;
    ensure_success(&output, "agent read")?;
    let visible = String::from_utf8_lossy(&output.stdout);
    Ok(visible
        .lines()
        .rev()
        .take(3)
        .any(|line| line.contains("Plan mode (shift+tab to cycle)")))
}

pub fn focus_workspace(context: &HerdrContext, workspace_id: &str) -> Result<()> {
    let output = run(context, ["workspace", "focus", workspace_id])?;
    ensure_success(&output, "workspace focus")
}

pub fn notify(context: &HerdrContext, title: &str, body: Option<&str>, sound: Option<&str>) {
    let mut args = vec!["notification", "show", title];
    if let Some(body) = body {
        args.extend(["--body", body]);
    }
    if let Some(sound) = sound {
        args.extend(["--sound", sound]);
    }
    let _ = run(context, args);
}

fn run<I, S>(context: &HerdrContext, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let binary = context.herdr_bin_path.as_deref().unwrap_or("herdr");
    let mut command = Command::new(binary);
    command.args(args);
    if let Some(socket) = &context.socket_path {
        command.env("HERDR_SOCKET_PATH", socket);
    }
    command
        .output()
        .with_context(|| format!("failed to execute {binary}"))
}

fn ensure_success(output: &Output, operation: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(anyhow!(
        "{operation} failed: {}",
        if stderr.is_empty() {
            output.status.to_string()
        } else {
            stderr
        }
    ))
}

fn env_value(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()))
}
