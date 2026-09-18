use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::config::Config;
use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub model: String,
    pub device: String,
    pub compute_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerTranscript {
    #[serde(default)]
    pub context_truncated: bool,
    pub text: String,
    pub language: Option<String>,
    pub language_probability: Option<f64>,
    pub duration_seconds: f64,
    pub duration_after_vad_seconds: Option<f64>,
    #[serde(default)]
    pub segments: Vec<WorkerSegment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub avg_logprob: f64,
    pub no_speech_prob: f64,
    #[serde(default)]
    pub words: Vec<WorkerWord>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerWord {
    pub start: f64,
    pub end: f64,
    pub word: String,
    pub probability: f64,
}

pub struct WorkerSupervisor {
    config: Config,
    worker: Worker,
}

impl WorkerSupervisor {
    pub async fn start(config: Config) -> Result<Self> {
        let worker = Worker::start(&config).await?;
        Ok(Self { config, worker })
    }

    pub fn info(&self) -> &WorkerInfo {
        &self.worker.info
    }

    pub async fn transcribe(
        &mut self,
        audio_path: &str,
        recognition: &crate::context::ResolvedContext,
    ) -> Result<WorkerTranscript> {
        match tokio::time::timeout(
            Duration::from_secs(self.config.worker.transcription_timeout_seconds),
            self.worker.transcribe(audio_path, recognition),
        )
        .await
        .context("transcription timed out")
        .and_then(|result| result)
        {
            Ok(transcript) => Ok(transcript),
            Err(first_error) => {
                tracing::warn!(error = %first_error, "STT worker failed; restarting once");
                self.worker.terminate().await;
                self.worker = Worker::start(&self.config)
                    .await
                    .context("failed to restart STT worker")?;
                let result = tokio::time::timeout(
                    Duration::from_secs(self.config.worker.transcription_timeout_seconds),
                    self.worker.transcribe(audio_path, recognition),
                )
                .await
                .context("transcription timed out after restart")
                .and_then(|result| result)
                .with_context(|| {
                    format!("worker failed after restart; first error: {first_error}")
                });
                if result.is_err() {
                    // Discard pending responses; the next request must never read a stale id.
                    self.worker.terminate().await;
                }
                result
            }
        }
    }

    pub async fn terminate(&mut self) {
        self.worker.terminate().await;
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    info: WorkerInfo,
    next_id: u64,
}

impl Worker {
    async fn start(config: &Config) -> Result<Self> {
        let python = config
            .worker
            .python
            .as_ref()
            .map(Into::into)
            .unwrap_or_else(paths::default_python);
        let mut command = Command::new(&python);
        command
            .arg("-m")
            .arg("herdr_stt_worker")
            .arg("--model")
            .arg(&config.model.name)
            .arg("--device")
            .arg(&config.model.device)
            .arg("--compute-type")
            .arg(&config.model.compute_type)
            .arg("--cpu-compute-type")
            .arg(&config.model.cpu_compute_type)
            .arg("--beam-size")
            .arg(config.transcription.beam_size.to_string())
            .arg("--min-silence-duration-ms")
            .arg(config.transcription.min_silence_duration_ms.to_string())
            .arg("--initial-prompt")
            .arg(&config.transcription.initial_prompt)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if config.model.cpu_fallback {
            command.arg("--cpu-fallback");
        }
        if config.transcription.multilingual {
            command.arg("--multilingual");
        }
        if !config.transcription.vad_filter {
            command.arg("--no-vad-filter");
        }
        if !config.transcription.condition_on_previous_text {
            command.arg("--no-condition-on-previous-text");
        }
        if config.correction.enabled {
            command.arg("--word-timestamps");
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start Python worker at {}", python.display()))?;
        let stdin = child.stdin.take().context("worker stdin was not piped")?;
        let stdout = child.stdout.take().context("worker stdout was not piped")?;
        let mut lines = BufReader::new(stdout).lines();
        let line = tokio::time::timeout(
            Duration::from_secs(config.worker.startup_timeout_seconds),
            lines.next_line(),
        )
        .await
        .context("worker startup timed out")??
        .ok_or_else(|| anyhow!("worker exited before reporting ready"))?;
        let message: WorkerMessage =
            serde_json::from_str(&line).context("invalid worker ready message")?;
        let info = match message {
            WorkerMessage::Ready {
                model,
                device,
                compute_type,
            } => WorkerInfo {
                model,
                device,
                compute_type,
            },
            WorkerMessage::Fatal { error } => {
                return Err(anyhow!("worker startup failed: {error}"));
            }
            WorkerMessage::Response { error, .. } => {
                return Err(anyhow!(
                    "worker returned response before ready: {}",
                    error.unwrap_or_else(|| "unknown error".into())
                ));
            }
        };
        tracing::info!(
            model = %info.model,
            device = %info.device,
            compute_type = %info.compute_type,
            "STT worker ready"
        );
        Ok(Self {
            child,
            stdin,
            lines,
            info,
            next_id: 1,
        })
    }

    async fn transcribe(
        &mut self,
        audio_path: &str,
        recognition: &crate::context::ResolvedContext,
    ) -> Result<WorkerTranscript> {
        let id = self.next_id;
        self.next_id += 1;
        let request = WorkerRequest {
            id,
            method: "transcribe",
            audio_path,
            hotwords: &recognition.hotwords,
            initial_prompt: &recognition.initial_prompt,
            language: recognition.language.as_deref(),
        };
        let mut line = serde_json::to_vec(&request)?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .context("failed to write to worker")?;
        self.stdin.flush().await?;

        let line = self
            .lines
            .next_line()
            .await
            .context("failed to read worker response")?
            .ok_or_else(|| anyhow!("worker exited during transcription"))?;
        let message: WorkerMessage =
            serde_json::from_str(&line).context("invalid worker response")?;
        match message {
            WorkerMessage::Response {
                id: response_id,
                ok: true,
                result: Some(result),
                ..
            } if response_id == id => Ok(result),
            WorkerMessage::Response {
                id: response_id,
                error,
                ..
            } if response_id == id => Err(anyhow!(
                "worker transcription failed: {}",
                error.unwrap_or_else(|| "unknown error".into())
            )),
            WorkerMessage::Fatal { error } => Err(anyhow!("worker fatal error: {error}")),
            other => Err(anyhow!("unexpected worker response: {other:?}")),
        }
    }

    async fn terminate(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

#[derive(Serialize)]
struct WorkerRequest<'a> {
    id: u64,
    method: &'static str,
    audio_path: &'a str,
    hotwords: &'a [String],
    initial_prompt: &'a str,
    language: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerMessage {
    Ready {
        model: String,
        device: String,
        compute_type: String,
    },
    Response {
        id: u64,
        ok: bool,
        result: Option<WorkerTranscript>,
        error: Option<String>,
    },
    Fatal {
        error: String,
    },
}
