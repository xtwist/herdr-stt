use super::{ApiFailure, ApiResult};
use crate::{config::Config, context::ResolvedContext, paths, protocol::HerdrContext};
use std::{
    os::unix::process::CommandExt,
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    process::{Child, Command},
    sync::oneshot,
};
static RECORDING_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) struct Recording {
    pub(super) child: Child,
    pub(super) process_group: i32,
    pub(super) path: PathBuf,
    pub(super) context: HerdrContext,
    pub(super) recognition: ResolvedContext,
    pub(super) vocabulary: Vec<String>,
    pub(super) timeout_cancel: Option<oneshot::Sender<()>>,
}

pub(super) async fn spawn_recorder(
    config: &Config,
    context: HerdrContext,
    recognition: ResolvedContext,
    vocabulary: Vec<String>,
) -> ApiResult<Recording> {
    let id = RECORDING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = paths::runtime_dir().join(format!(
        "recording-{}-{timestamp}-{id}.wav",
        std::process::id()
    ));
    let mut command = Command::new("pw-record");
    command
        .arg("--rate")
        .arg(config.audio.sample_rate.to_string())
        .arg("--channels")
        .arg(config.audio.channels.to_string())
        .arg("--channel-map")
        .arg("MONO")
        .arg("--format")
        .arg("s16");
    if let Some(target) = config
        .audio
        .target
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        command.arg("--target").arg(target);
    }
    command
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| ApiFailure::new("recording_failed", error.to_string()))?;
    let process_group = child
        .id()
        .ok_or_else(|| ApiFailure::new("recording_failed", "pw-record has no process id"))?
        as i32;
    tokio::time::sleep(Duration::from_millis(150)).await;
    if let Some(status) = child
        .try_wait()
        .map_err(|error| ApiFailure::new("recording_failed", error.to_string()))?
    {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ApiFailure::new(
            "recording_failed",
            format!("pw-record exited immediately: {status}"),
        ));
    }
    Ok(Recording {
        child,
        process_group,
        path,
        context,
        recognition,
        vocabulary,
        timeout_cancel: None,
    })
}

pub(super) async fn finish_recorder(recording: &mut Recording) -> ApiResult<()> {
    cancel_recording_timeout(recording);
    signal_group(recording.process_group, libc::SIGINT);
    match tokio::time::timeout(Duration::from_secs(3), recording.child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return Err(ApiFailure::new("recording_failed", error.to_string())),
        Err(_) => {
            signal_group(recording.process_group, libc::SIGTERM);
            let _ = tokio::time::timeout(Duration::from_secs(1), recording.child.wait()).await;
            let _ = recording.child.kill().await;
        }
    }
    if !recording.path.is_file() {
        return Err(ApiFailure::new(
            "recording_failed",
            "pw-record did not create an audio file",
        ));
    }
    Ok(())
}

pub(super) fn cancel_recording_timeout(recording: &mut Recording) {
    if let Some(cancel) = recording.timeout_cancel.take() {
        let _ = cancel.send(());
    }
}

pub(super) fn signal_group(process_group: i32, signal: i32) {
    if process_group > 1 {
        unsafe {
            libc::kill(-process_group, signal);
        }
    }
}
