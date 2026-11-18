mod managed;
use managed::{corrector_install, corrector_serve, corrector_status};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
mod evaluation;
use crate::client;
use crate::config::Config;
use crate::herdr;
use evaluation::{benchmark_command, history_command};
mod routing;
use crate::context::{self, RecognitionHints};
use crate::paths;
use crate::protocol::{StartParams, StatusResult, Transcript};
use crate::tray::{self, Attempt, Report, Status};
use crate::vocabulary;
use routing::route_transcript;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Default, clap::Args)]
struct ContextArgs {
    /// Named context profile from config.toml.
    #[arg(long)]
    profile: Option<String>,
    /// Additional background for this recording only.
    #[arg(long = "context")]
    text: Option<String>,
    /// UTF-8 background file; may be repeated.
    #[arg(long = "context-file")]
    files: Vec<PathBuf>,
    /// Exact spelling of a name or term; may be repeated.
    #[arg(long = "hotword")]
    hotwords: Vec<String>,
    /// Whisper language code, or auto.
    #[arg(long)]
    language: Option<String>,
    /// Project directory used for vocabulary and profile selection.
    #[arg(long)]
    cwd: Option<PathBuf>,
}

impl ContextArgs {
    fn hints(&self, config: &Config) -> Result<RecognitionHints> {
        let mut pieces = vec![self.text.clone().unwrap_or_default()];
        for file in &self.files {
            pieces.push(context::read_context_file(
                file,
                config.context.max_file_bytes,
            )?);
        }
        Ok(RecognitionHints {
            profile: self.profile.clone(),
            text: pieces.join("\n\n"),
            hotwords: self.hotwords.clone(),
            language: self.language.clone(),
        })
    }

    fn cwd(&self, target: &crate::protocol::HerdrContext) -> Result<Option<String>> {
        if let Some(cwd) = &self.cwd {
            let cwd = cwd.canonicalize().context("cannot resolve --cwd")?;
            anyhow::ensure!(cwd.is_dir(), "--cwd must be a directory");
            return Ok(Some(cwd.display().to_string()));
        }
        Ok(herdr::pane_cwd(target).unwrap_or_default().or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string())
        }))
    }
}

#[derive(Debug, Parser)]
#[command(version, about = "Control the Herdr speech-to-text daemon")]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start recording when idle; stop, transcribe, and route when recording.
    Toggle {
        #[command(flatten)]
        context: ContextArgs,
    },
    /// Initialize config and user services. Add --keybinding to bind Ctrl+Alt+V.
    Setup {
        #[arg(long)]
        keybinding: bool,
    },
    /// Preview the effective recognition context without loading a model.
    Context {
        #[command(flatten)]
        context: ContextArgs,
    },
    /// List configured context profiles.
    Profiles,
    /// Show daemon and model state.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Show the recording state in the desktop system tray.
    Tray,
    /// Cancel the current recording without transcription.
    Cancel,
    /// Print the last successful transcript.
    Last {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        raw: bool,
    },
    /// Transcribe an existing audio file without routing it to Herdr.
    Transcribe {
        path: PathBuf,
        #[command(flatten)]
        context: ContextArgs,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        raw: bool,
    },
    /// Check binaries, configuration, daemon, PipeWire, and Herdr context.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Reload config by restarting the daemon and any active managed corrector.
    Reload,
    /// Install or inspect the optional local Qwen corrector.
    Corrector {
        #[command(subcommand)]
        command: CorrectorCommands,
    },
    /// Inspect or clear the local transcript history.
    History {
        #[command(subcommand)]
        command: HistoryCommands,
    },
    /// Evaluate raw, deterministic, and corrected transcripts from a JSONL corpus.
    Benchmark {
        corpus: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum CorrectorCommands {
    /// Download the pinned llama.cpp runtime and Qwen3-4B model.
    Install,
    /// Show corrector files, service, and endpoint status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Internal systemd entrypoint for the managed llama-server.
    #[command(hide = true)]
    Serve,
}

#[derive(Debug, Subcommand)]
enum HistoryCommands {
    /// List recent transcript records.
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Export the retained JSONL records.
    Export { path: PathBuf },
    /// Delete retained transcript records.
    Clear,
}

pub async fn run() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Commands::Toggle { context } => Attempt::start().finish(toggle(context).await),
        Commands::Setup { keybinding } => Attempt::start().finish(
            setup(keybinding)
                .map(|()| None)
                .context("Voice input setup failed"),
        ),
        Commands::Profiles => {
            let config = Config::load()?;
            print_json(&config.context.profiles.keys().collect::<Vec<_>>())
        }
        Commands::Context { context } => {
            let config = Config::load()?;
            let cwd = context.cwd(&herdr::context_from_env())?;
            print_json(&context::resolve(
                &config,
                cwd.as_deref(),
                &context.hints(&config)?,
            )?)
        }
        Commands::Status { json } => Attempt::start().finish(
            show_status(json)
                .await
                .map(|()| None)
                .context("Unable to read STT status"),
        ),
        Commands::Tray => tray::run().await,
        Commands::Cancel => {
            Attempt::start().finish(cancel().await.context("Unable to cancel recording"))
        }
        Commands::Last { json, raw } => show_last(json, raw).await,
        Commands::Transcribe {
            path,
            json,
            raw,
            context,
        } => Attempt::start().finish(
            transcribe(path, json, raw, context)
                .await
                .context("File transcription failed"),
        ),
        Commands::Doctor { json } => doctor(json).await,
        Commands::Reload => Attempt::start().finish(
            reload()
                .await
                .map(|()| None)
                .context("Voice input reload failed"),
        ),
        Commands::Corrector { command } => match command {
            CorrectorCommands::Install => corrector_install(),
            CorrectorCommands::Status { json } => corrector_status(json).await,
            CorrectorCommands::Serve => corrector_serve(),
        },
        Commands::History { command } => history_command(command),
        Commands::Benchmark { corpus, output } => benchmark_command(corpus, output).await,
    }
}

async fn toggle(options: ContextArgs) -> Result<Option<Report>> {
    ensure_daemon()
        .await
        .context("Unable to start the STT service")?;
    let status: StatusResult = client::call("status", json!({})).await?;
    let current_context = herdr::context_from_env();
    match status.state.as_str() {
        "idle" => {
            let workspaces = herdr::list_workspaces(&current_context).unwrap_or_default();
            let mut context = current_context.clone();
            if let Some(workspace_id) = &context.workspace_id {
                context.workspace_label = workspaces
                    .iter()
                    .find(|workspace| &workspace.workspace_id == workspace_id)
                    .map(|workspace| workspace.label.clone());
            }
            context.cwd = options.cwd(&context)?;
            let config = Config::load()?;
            let recognition = options.hints(&config)?;
            context::resolve(&config, context.cwd.as_deref(), &recognition)?;
            let project_vocabulary =
                vocabulary::collect(context.cwd.as_deref(), &Config::load()?.vocabulary)
                    .unwrap_or_default();
            let mut hotwords = Vec::new();
            if let Some(label) = &context.workspace_label {
                hotwords.push(label.clone());
            }
            hotwords.extend(project_vocabulary.terms.iter().cloned());
            hotwords.extend([
                "Plan mode".into(),
                "режим планирования".into(),
                "планирование".into(),
            ]);
            hotwords.extend(workspaces.iter().map(|workspace| workspace.label.clone()));
            let _: Value = client::call(
                "start",
                StartParams {
                    recognition,
                    context,
                    hotwords,
                    vocabulary: project_vocabulary.terms,
                },
            )
            .await
            .context("Unable to start recording")?;
            Ok(None)
        }
        "recording" => {
            let transcript: Transcript = client::call("stop", json!({}))
                .await
                .context("Unable to finish or transcribe recording")?;
            route_transcript(transcript)
                .map(Some)
                .context("Voice command failed · recover the text with herdr-stt last")
        }
        "transcribing" => Ok(None),
        other => Err(anyhow!("unknown daemon state {other}")),
    }
}

async fn show_status(json_output: bool) -> Result<()> {
    let status: StatusResult = client::call("status", json!({})).await?;
    if json_output {
        print_json(&status)?;
    } else {
        println!(
            "state={} model={} device={} compute_type={} last={} correction={} backend={}",
            status.state,
            status.model,
            status.device,
            status.compute_type,
            status.has_last_transcript,
            status.correction_enabled,
            status.correction_backend.as_deref().unwrap_or("disabled")
        );
    }
    Ok(())
}

async fn cancel() -> Result<Option<Report>> {
    let result: Value = client::call("cancel", json!({})).await?;
    if std::env::var_os("HERDR_PLUGIN_ID").is_none() {
        print_json(&result)?;
    }
    Ok((result["cancelled"] == true)
        .then(|| Report::new(Status::Cancelled, "Recording cancelled", None)))
}

async fn show_last(json_output: bool, raw: bool) -> Result<()> {
    let transcript: Option<Transcript> = client::call("last", json!({})).await?;
    match transcript {
        Some(transcript) if json_output => print_json(&transcript),
        Some(transcript) => {
            println!(
                "{}",
                if raw {
                    &transcript.raw_text
                } else {
                    &transcript.text
                }
            );
            Ok(())
        }
        None => Err(anyhow!(
            "there is no successful transcript in daemon memory"
        )),
    }
}

async fn transcribe(
    path: PathBuf,
    json_output: bool,
    raw: bool,
    options: ContextArgs,
) -> Result<Option<Report>> {
    let path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    let mut context = herdr::context_from_env();
    context.cwd = options.cwd(&context)?;
    let config = Config::load()?;
    let recognition = options.hints(&config)?;
    context::resolve(&config, context.cwd.as_deref(), &recognition)?;
    let project_vocabulary =
        vocabulary::collect(context.cwd.as_deref(), &Config::load()?.vocabulary)
            .unwrap_or_default();
    ensure_daemon().await?;
    let transcript: Transcript = client::call(
        "transcribe_file",
        json!({
            "path": path,
            "recognition": recognition,
            "context": context,
            "hotwords": project_vocabulary.terms,
            "vocabulary": project_vocabulary.terms,
        }),
    )
    .await?;
    if json_output {
        print_json(&transcript)?;
    } else {
        println!(
            "{}",
            if raw {
                &transcript.raw_text
            } else {
                &transcript.text
            }
        );
    }
    Ok(Some(if transcript.deterministic_text.trim().is_empty() {
        Report::no_speech()
    } else {
        Report::new(Status::Done, "Audio file transcribed", None)
    }))
}

#[derive(Serialize)]
struct DoctorReport {
    config_path: String,
    config_ok: bool,
    python_path: String,
    python_ok: bool,
    pw_record_ok: bool,
    systemd_unit_ok: bool,
    daemon: String,
    herdr_env: bool,
    herdr_context: crate::protocol::HerdrContext,
    corrector_enabled: bool,
    corrector_binary_ok: bool,
    corrector_model_ok: bool,
    history_path: String,
}

async fn doctor(json_output: bool) -> Result<()> {
    let config = Config::load();
    let config_ok = config.is_ok();
    let python = config
        .as_ref()
        .ok()
        .and_then(|config| config.worker.python.clone().map(PathBuf::from))
        .unwrap_or_else(paths::default_python);
    let corrector_enabled = config
        .as_ref()
        .is_ok_and(|config| config.correction.enabled);
    let corrector_binary = config
        .as_ref()
        .ok()
        .and_then(|config| config.correction.managed.binary.clone())
        .unwrap_or_else(paths::corrector_binary);
    let corrector_model = config
        .as_ref()
        .ok()
        .and_then(|config| config.correction.managed.model_path.clone())
        .unwrap_or_else(paths::corrector_model);
    let daemon = match client::call::<StatusResult>("status", json!({})).await {
        Ok(status) => format!(
            "{} · {} / {} / {}",
            status.state, status.model, status.device, status.compute_type
        ),
        Err(error) => format!("unavailable: {error}"),
    };
    let report = DoctorReport {
        config_path: paths::config_path().display().to_string(),
        config_ok,
        python_path: python.display().to_string(),
        python_ok: python.is_file(),
        pw_record_ok: command_exists("pw-record"),
        systemd_unit_ok: systemd_unit_exists(),
        daemon,
        herdr_env: std::env::var("HERDR_ENV").as_deref() == Ok("1"),
        herdr_context: herdr::context_from_env(),
        corrector_enabled,
        corrector_binary_ok: corrector_binary.is_file(),
        corrector_model_ok: corrector_model.is_file(),
        history_path: paths::history_path().display().to_string(),
    };
    if json_output {
        print_json(&report)
    } else {
        println!("config: {} ({})", report.config_path, ok(report.config_ok));
        println!("python: {} ({})", report.python_path, ok(report.python_ok));
        println!("pw-record: {}", ok(report.pw_record_ok));
        println!("systemd unit: {}", ok(report.systemd_unit_ok));
        println!("daemon: {}", report.daemon);
        println!(
            "corrector: enabled={} binary={} model={}",
            report.corrector_enabled,
            ok(report.corrector_binary_ok),
            ok(report.corrector_model_ok)
        );
        println!("history: {}", report.history_path);
        println!("HERDR_ENV: {}", ok(report.herdr_env));
        println!(
            "Herdr pane: {}",
            report
                .herdr_context
                .pane_id
                .as_deref()
                .unwrap_or("unavailable")
        );
        Ok(())
    }
}

async fn reload() -> Result<()> {
    Config::load()?;
    setup(false)?;
    let output = Command::new("systemctl")
        .args(["--user", "restart", "herdr-stt.service"])
        .output()
        .context("failed to restart herdr-stt.service")?;
    anyhow::ensure!(
        output.status.success(),
        "failed to restart herdr-stt.service: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let _ = Command::new("systemctl")
        .args(["--user", "try-restart", "herdr-stt-corrector.service"])
        .status();
    ensure_daemon().await?;
    println!("Herdr STT configuration reloaded");
    Ok(())
}

fn setup(keybinding: bool) -> Result<()> {
    let root = paths::project_root();
    let mut command = Command::new(paths::default_python());
    command
        .arg(root.join("scripts/setup.py"))
        .arg("--root")
        .arg(&root)
        .arg("--config")
        .arg(paths::config_path())
        .arg("--state")
        .arg(paths::state_dir());
    if keybinding {
        command.arg("--keybinding");
    }
    let status = command
        .status()
        .context("failed to run plugin setup (Python 3.12+ required)")?;
    anyhow::ensure!(status.success(), "plugin setup failed");
    Ok(())
}

async fn ensure_daemon() -> Result<()> {
    setup(false)?;
    if client::ping().await.is_ok() {
        return Ok(());
    }
    let active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "herdr-stt.service"])
        .status()
        .is_ok_and(|status| status.success());
    let operation = if active {
        // A daemon may still be loading its model. Restart only an incompatible instance.
        match client::call::<Value>("ping", json!({})).await {
            Err(error) if error.to_string().contains("unsupported_version") => "restart",
            _ => "start",
        }
    } else {
        "start"
    };
    let output = Command::new("systemctl")
        .args(["--user", operation, "herdr-stt.service"])
        .output()
        .context("failed to start herdr-stt.service")?;
    if !output.status.success() {
        return Err(anyhow!(
            "systemctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let deadline =
        Instant::now() + Duration::from_secs(Config::load()?.worker.startup_timeout_seconds + 10);
    while Instant::now() < deadline {
        if client::ping().await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(anyhow!(
        "herdr-sttd did not become ready; inspect `journalctl --user -u herdr-stt.service`"
    ))
}

fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v \"$1\" >/dev/null 2>&1", "sh", command])
        .status()
        .is_ok_and(|status| status.success())
}

fn systemd_unit_exists() -> bool {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .is_some_and(|config| config.join("systemd/user/herdr-stt.service").is_file())
}

fn ok(value: bool) -> &'static str {
    if value { "ok" } else { "missing" }
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
