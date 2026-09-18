use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::context::{self, RecognitionHints, ResolvedContext};
use crate::history;
use crate::paths;
use crate::pipeline::build_transcript;
use crate::protocol::{
    API_VERSION, HerdrContext, Request, Response, StartParams, StatusResult, Transcript,
};
use crate::vocabulary::ProjectVocabulary;
use crate::worker::WorkerSupervisor;
use anyhow::{Context, Result, anyhow};
use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, oneshot};

mod recording;
use recording::{
    Recording, cancel_recording_timeout, finish_recorder, signal_group, spawn_recorder,
};

#[derive(Debug, Parser)]
#[command(version, about = "Local faster-whisper daemon for Herdr")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
}

struct Shared {
    config: Config,
    state: Mutex<AppState>,
    worker: Mutex<WorkerSupervisor>,
    worker_info: crate::worker::WorkerInfo,
}

struct AppState {
    phase: Phase,
    last: Option<Transcript>,
}

enum Phase {
    Idle,
    Recording(Box<Recording>),
    Transcribing,
}

#[derive(Deserialize)]
struct TranscribeFileParams {
    #[serde(default)]
    recognition: RecognitionHints,
    path: String,
    #[serde(default)]
    hotwords: Vec<String>,
    #[serde(default)]
    vocabulary: Vec<String>,
    #[serde(default)]
    context: HerdrContext,
    #[serde(default = "default_true")]
    record_history: bool,
}

pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "herdr_stt=info".into()),
        )
        .init();
    let args = Args::parse();
    let config = match args.config {
        Some(path) => Config::load_from(&path)?,
        None => Config::load()?,
    };
    prepare_runtime_dir()?;
    let socket_path = paths::socket_path();
    let listener = bind_socket(&socket_path).await?;
    let worker = WorkerSupervisor::start(config.clone()).await?;
    let shared = Arc::new(Shared {
        config,
        state: Mutex::new(AppState {
            phase: Phase::Idle,
            last: None,
        }),
        worker_info: worker.info().clone(),
        worker: Mutex::new(worker),
    });
    tracing::info!(socket = %socket_path.display(), "herdr-sttd ready");
    let shutdown_signal = shutdown_signal();
    tokio::pin!(shutdown_signal);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    if let Err(error) = serve_connection(stream, shared).await {
                        tracing::warn!(%error, "client request failed");
                    }
                });
            }
            _ = &mut shutdown_signal => {
                tracing::info!("shutdown requested");
                shutdown(&shared).await;
                break;
            }
        }
    }
    let _ = tokio::fs::remove_file(&socket_path).await;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn serve_connection(stream: UnixStream, shared: Arc<Shared>) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        BufReader::new(read_half.take(1_048_577)).read_line(&mut line),
    )
    .await??;
    anyhow::ensure!(line.len() <= 1_048_576, "request exceeds 1 MiB");
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => handle_request(request, &shared).await,
        Err(error) => Response::failure("unknown", "invalid_request", error.to_string()),
    };
    let mut payload = serde_json::to_vec(&response)?;
    payload.push(b'\n');
    write_half.write_all(&payload).await?;
    write_half.shutdown().await?;
    Ok(())
}

async fn handle_request(request: Request, shared: &Arc<Shared>) -> Response {
    if request.version != API_VERSION {
        return Response::failure(
            request.request_id,
            "unsupported_version",
            format!("expected API version {API_VERSION}"),
        );
    }
    let request_id = request.request_id.clone();
    let result = match request.method.as_str() {
        "ping" => Ok(json!({"type": "pong"})),
        "status" => status(shared).await,
        "start" => start_recording(shared, request.params).await,
        "stop" => stop_and_transcribe(shared).await,
        "cancel" => cancel_recording(shared).await,
        "last" => last_transcript(shared).await,
        "transcribe_file" => transcribe_file(shared, request.params).await,
        _ => Err(ApiFailure::new(
            "method_not_found",
            format!("unknown method {}", request.method),
        )),
    };
    match result {
        Ok(value) => Response::success(request_id, value),
        Err(error) => Response::failure(request_id, error.code, error.message),
    }
}

async fn status(shared: &Shared) -> ApiResult<Value> {
    let state = shared.state.lock().await;
    let phase = match state.phase {
        Phase::Idle => "idle",
        Phase::Recording(_) => "recording",
        Phase::Transcribing => "transcribing",
    };
    let has_last = state.last.is_some();
    drop(state);
    let info = shared
        .worker
        .try_lock()
        .map(|worker| worker.info().clone())
        .unwrap_or_else(|_| shared.worker_info.clone());
    Ok(serde_json::to_value(StatusResult {
        state: phase.into(),
        model: info.model.clone(),
        device: info.device.clone(),
        compute_type: info.compute_type.clone(),
        has_last_transcript: has_last,
        correction_enabled: shared.config.correction.enabled,
        correction_backend: shared
            .config
            .correction
            .enabled
            .then(|| format!("{:?}", shared.config.correction.backend).to_lowercase()),
    })?)
}

async fn start_recording(shared: &Shared, params: Value) -> ApiResult<Value> {
    let params: StartParams = serde_json::from_value(params)
        .map_err(|error| ApiFailure::new("invalid_params", error.to_string()))?;
    let recognition = resolve_context(
        &shared.config,
        &params.context,
        &params.recognition,
        &params.hotwords,
    )?;
    let mut state = shared.state.lock().await;
    match state.phase {
        Phase::Idle => {}
        Phase::Recording(_) => {
            return Err(ApiFailure::new("already_recording", "recording is active"));
        }
        Phase::Transcribing => return Err(ApiFailure::new("busy", "transcription is in progress")),
    }
    let mut recording = spawn_recorder(
        &shared.config,
        params.context,
        recognition,
        params.vocabulary,
    )
    .await?;
    let process_group = recording.process_group;
    let max_seconds = shared.config.audio.max_seconds;
    let (timeout_cancel, timeout_cancelled) = oneshot::channel();
    recording.timeout_cancel = Some(timeout_cancel);
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(max_seconds)) => {
                signal_group(process_group, libc::SIGINT);
            }
            _ = timeout_cancelled => {}
        }
    });
    state.phase = Phase::Recording(Box::new(recording));
    Ok(json!({"state": "recording"}))
}

async fn stop_and_transcribe(shared: &Shared) -> ApiResult<Value> {
    let mut recording = {
        let mut state = shared.state.lock().await;
        let phase = std::mem::replace(&mut state.phase, Phase::Transcribing);
        match phase {
            Phase::Recording(recording) => recording,
            Phase::Idle => {
                state.phase = Phase::Idle;
                return Err(ApiFailure::new("not_recording", "no recording is active"));
            }
            Phase::Transcribing => {
                state.phase = Phase::Transcribing;
                return Err(ApiFailure::new(
                    "busy",
                    "transcription is already in progress",
                ));
            }
        }
    };

    let result = async {
        finish_recorder(&mut recording).await?;
        let path = recording.path.to_string_lossy().to_string();
        let (worker_transcript, info) = {
            let mut worker = shared.worker.lock().await;
            let transcript = worker
                .transcribe(&path, &recording.recognition)
                .await
                .map_err(|error| ApiFailure::new("transcription_failed", error.to_string()))?;
            (transcript, worker.info().clone())
        };
        let mut terms = recording.recognition.hotwords.clone();
        terms.extend(recording.vocabulary.clone());
        let vocabulary = ProjectVocabulary::from_terms(terms);
        Ok::<_, ApiFailure>(
            build_transcript(
                &shared.config,
                worker_transcript,
                info,
                recording.context.clone(),
                vocabulary,
                true,
                &recording.recognition,
            )
            .await,
        )
    }
    .await;
    let _ = tokio::fs::remove_file(&recording.path).await;

    let mut state = shared.state.lock().await;
    state.phase = Phase::Idle;
    match result {
        Ok(transcript) => {
            if let Err(error) = history::append(&shared.config.history, &transcript) {
                tracing::warn!(%error, "failed to persist transcript history");
            }
            state.last = Some(transcript.clone());
            Ok(serde_json::to_value(transcript)?)
        }
        Err(error) => Err(error),
    }
}

async fn cancel_recording(shared: &Shared) -> ApiResult<Value> {
    let mut recording = {
        let mut state = shared.state.lock().await;
        match std::mem::replace(&mut state.phase, Phase::Idle) {
            Phase::Recording(recording) => recording,
            Phase::Idle => return Ok(json!({"state": "idle", "cancelled": false})),
            Phase::Transcribing => {
                state.phase = Phase::Transcribing;
                return Err(ApiFailure::new(
                    "busy",
                    "cannot cancel active transcription",
                ));
            }
        }
    };
    cancel_recording_timeout(&mut recording);
    signal_group(recording.process_group, libc::SIGINT);
    let mut child = recording.child;
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    let _ = child.kill().await;
    let _ = tokio::fs::remove_file(recording.path).await;
    Ok(json!({"state": "idle", "cancelled": true}))
}

async fn last_transcript(shared: &Shared) -> ApiResult<Value> {
    let state = shared.state.lock().await;
    Ok(serde_json::to_value(&state.last)?)
}

async fn transcribe_file(shared: &Shared, params: Value) -> ApiResult<Value> {
    let params: TranscribeFileParams = serde_json::from_value(params)
        .map_err(|error| ApiFailure::new("invalid_params", error.to_string()))?;
    let recognition = resolve_context(
        &shared.config,
        &params.context,
        &params.recognition,
        &params.hotwords,
    )?;
    let record_history = params.record_history;
    if !Path::new(&params.path).is_file() {
        return Err(ApiFailure::new(
            "file_not_found",
            format!("{} is not a file", params.path),
        ));
    }
    {
        let mut state = shared.state.lock().await;
        if !matches!(state.phase, Phase::Idle) {
            return Err(ApiFailure::new("busy", "daemon is not idle"));
        }
        state.phase = Phase::Transcribing;
    }
    let result = {
        let (worker_result, info) = {
            let mut worker = shared.worker.lock().await;
            let result = worker.transcribe(&params.path, &recognition).await;
            (result, worker.info().clone())
        };
        match worker_result {
            Ok(transcript) => {
                let mut terms = recognition.hotwords.clone();
                terms.extend(params.vocabulary);
                let vocabulary = ProjectVocabulary::from_terms(terms);
                Ok(build_transcript(
                    &shared.config,
                    transcript,
                    info,
                    params.context,
                    vocabulary,
                    false,
                    &recognition,
                )
                .await)
            }
            Err(error) => Err(error),
        }
    };
    let mut state = shared.state.lock().await;
    state.phase = Phase::Idle;
    match result {
        Ok(transcript) => {
            if record_history
                && let Err(error) = history::append(&shared.config.history, &transcript)
            {
                tracing::warn!(%error, "failed to persist transcript history");
            }
            state.last = Some(transcript.clone());
            Ok(serde_json::to_value(transcript)?)
        }
        Err(error) => Err(ApiFailure::new("transcription_failed", error.to_string())),
    }
}

fn default_true() -> bool {
    true
}

async fn shutdown(shared: &Shared) {
    let recording = {
        let mut state = shared.state.lock().await;
        match std::mem::replace(&mut state.phase, Phase::Idle) {
            Phase::Recording(recording) => Some(recording),
            _ => None,
        }
    };
    if let Some(mut recording) = recording {
        cancel_recording_timeout(&mut recording);
        signal_group(recording.process_group, libc::SIGINT);
        let _ = tokio::fs::remove_file(recording.path).await;
    }
    shared.worker.lock().await.terminate().await;
}

fn prepare_runtime_dir() -> Result<()> {
    let runtime = paths::runtime_dir();
    std::fs::create_dir_all(&runtime)
        .with_context(|| format!("failed to create {}", runtime.display()))?;
    std::fs::set_permissions(&runtime, Permissions::from_mode(0o700))?;
    Ok(())
}

async fn bind_socket(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err(anyhow!(
                "another daemon is already listening at {}",
                path.display()
            ));
        }
        tokio::fs::remove_file(path).await?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("failed to bind {}", path.display()))?;
    std::fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok(listener)
}

type ApiResult<T> = std::result::Result<T, ApiFailure>;

#[derive(Debug)]
struct ApiFailure {
    code: String,
    message: String,
}

impl ApiFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<serde_json::Error> for ApiFailure {
    fn from(error: serde_json::Error) -> Self {
        Self::new("serialization_failed", error.to_string())
    }
}

fn resolve_context(
    config: &Config,
    target: &HerdrContext,
    hints: &RecognitionHints,
    hotwords: &[String],
) -> ApiResult<ResolvedContext> {
    let mut recognition = context::resolve(config, target.cwd.as_deref(), hints)
        .map_err(|error| ApiFailure::new("invalid_context", error.to_string()))?;
    recognition.hotwords.extend_from_slice(hotwords);
    context::limit_hotwords(
        &mut recognition.hotwords,
        config.context.max_hotwords,
        &mut recognition.truncated,
    );
    Ok(recognition)
}
