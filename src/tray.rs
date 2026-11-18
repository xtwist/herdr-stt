use crate::{
    client,
    protocol::{CaptureStatus, StatusResult},
};
use anyhow::{Context, Result};
use ksni::{Tray, TrayMethods, menu::StandardItem};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, DirBuilder, File},
    io::{self, Write},
    os::{fd::AsRawFd, unix::fs::DirBuilderExt},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Status {
    Ready,
    Recording,
    Finishing,
    Recorded,
    Processing,
    Done,
    Cancelled,
    NoSpeech,
    Error,
    Offline,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Report {
    status: Status,
    title: String,
    detail: Option<String>,
}

impl Report {
    pub(crate) fn new(status: Status, title: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            status,
            title: title.into(),
            detail,
        }
    }

    pub(crate) fn no_speech() -> Self {
        Self::new(Status::NoSpeech, "No speech detected", None)
    }
}

// Linux's monotonic clock orders attempts consistently across separate CLI processes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct Attempt {
    seconds: libc::time_t,
    nanoseconds: libc::c_long,
    pid: u32,
}

impl Attempt {
    pub(crate) fn start() -> Self {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // CLOCK_MONOTONIC is available on the plugin's supported platform.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
        Self {
            seconds: time.tv_sec,
            nanoseconds: time.tv_nsec,
            pid: std::process::id(),
        }
    }

    pub(crate) fn finish(self, result: Result<Option<Report>>) -> Result<()> {
        let report = match &result {
            Ok(report) => report.clone(),
            Err(error) => Some(Report::new(
                Status::Error,
                "Voice input error",
                Some(
                    error
                        .to_string()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(160)
                        .collect(),
                ),
            )),
        };
        if let (Some(report), Some(dir)) = (report, runtime_dir())
            && let Err(error) = publish(&dir, self, report)
        {
            eprintln!("Could not update the Herdr STT tray: {error:#}");
        }
        result.map(|_| ())
    }
}

#[derive(Deserialize, Serialize)]
struct LastResult {
    attempt: Attempt,
    report: Report,
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|path| PathBuf::from(path).join("herdr-stt-tray"))
}

fn prepare_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

fn lock(dir: &Path, name: &str, flags: libc::c_int) -> io::Result<File> {
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(name))?;
    if unsafe { libc::flock(file.as_raw_fd(), flags) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

fn read_result(dir: &Path) -> Option<LastResult> {
    serde_json::from_slice(&fs::read(dir.join("result.json")).ok()?).ok()
}

fn publish(dir: &Path, attempt: Attempt, report: Report) -> Result<()> {
    prepare_dir(dir)?;
    let _guard = lock(dir, "result.lock", libc::LOCK_EX)?;
    if read_result(dir).is_some_and(|previous| previous.attempt > attempt) {
        return Ok(());
    }
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut temp, &LastResult { attempt, report })?;
    temp.flush()?;
    temp.persist(dir.join("result.json"))?;
    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
struct Indicator {
    report: Report,
    last: Option<Report>,
    model: Option<String>,
}

impl Indicator {
    async fn poll(dir: &Path) -> Self {
        let daemon = tokio::time::timeout(
            Duration::from_millis(350),
            client::call::<StatusResult>("status", json!({})),
        )
        .await
        .ok()
        .and_then(Result::ok);
        let last = read_result(dir).map(|last| last.report);
        let (status, title, detail) = match daemon.as_ref() {
            None => (Status::Offline, "STT service unavailable", None),
            Some(daemon) => match (daemon.capture, daemon.state.as_str()) {
                (Some(CaptureStatus::Active), _) | (None, "recording") => {
                    (Status::Recording, "Recording", Some("Ctrl+Alt+V to stop"))
                }
                (Some(CaptureStatus::Finishing), _) => {
                    (Status::Finishing, "Finishing recording", None)
                }
                (Some(CaptureStatus::Recorded), _) => (
                    Status::Recorded,
                    "Recording saved",
                    Some("Ctrl+Alt+V to transcribe"),
                ),
                (Some(CaptureStatus::Failed), _) => {
                    (Status::Error, "Recorder stopped unexpectedly", None)
                }
                (None, "transcribing") => (Status::Processing, "Transcribing", None),
                (None, _) => (Status::Ready, "Ready", None),
            },
        };
        let reported = Report::new(status, title, detail.map(str::to_owned));
        let (report, last) = if status == Status::Ready {
            (last.unwrap_or(reported), None)
        } else {
            (reported, last)
        };
        Self {
            report,
            last,
            model: daemon.map(|daemon| {
                format!(
                    "{} · {} / {}",
                    daemon.model, daemon.device, daemon.compute_type
                )
            }),
        }
    }
}

impl Tray for Indicator {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "herdr-stt".into()
    }

    fn title(&self) -> String {
        format!("Herdr STT · {}", self.report.title)
    }

    fn status(&self) -> ksni::Status {
        ksni::Status::Active
    }

    fn icon_name(&self) -> String {
        match self.report.status {
            Status::Ready => "audio-input-microphone-symbolic",
            Status::Recording => "media-record-symbolic",
            Status::Finishing => "media-playback-pause-symbolic",
            Status::Recorded => "media-playback-stop-symbolic",
            Status::Processing => "view-refresh-symbolic",
            Status::Done => "object-select-symbolic",
            Status::Cancelled => "process-stop-symbolic",
            Status::NoSpeech => "dialog-warning-symbolic",
            Status::Error => "dialog-error-symbolic",
            Status::Offline => "microphone-disabled-symbolic",
        }
        .into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.title(),
            description: self.report.detail.clone().unwrap_or_default(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let mut labels = vec![self.title()];
        labels.extend(self.report.detail.iter().cloned());
        if let Some(last) = &self.last {
            labels.push(format!("Last result · {}", last.title));
            labels.extend(last.detail.iter().cloned());
        }
        labels.extend(self.model.iter().cloned());
        labels
            .into_iter()
            .map(|label| {
                StandardItem {
                    label,
                    enabled: false,
                    ..Default::default()
                }
                .into()
            })
            .collect()
    }
}

pub(crate) async fn run() -> Result<()> {
    let dir = runtime_dir().context("XDG_RUNTIME_DIR is required by the Herdr STT tray")?;
    prepare_dir(&dir)?;
    let _guard = match lock(&dir, "indicator.lock", libc::LOCK_EX | libc::LOCK_NB) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut last = Indicator::poll(&dir).await;
    let tray = last.clone().assume_sni_available(true).spawn().await?;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        anyhow::ensure!(
            !tray.is_closed(),
            "Herdr STT tray lost its D-Bus connection"
        );
        let current = Indicator::poll(&dir).await;
        if current != last {
            last = current;
            tray.update(|indicator| indicator.clone_from(&last))
                .await
                .context("Herdr STT tray lost its D-Bus connection")?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_result_cannot_overwrite_a_newer_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let old = Attempt {
            seconds: 1,
            nanoseconds: 0,
            pid: 1,
        };
        let new = Attempt { seconds: 2, ..old };
        let last = Report::no_speech();
        publish(dir.path(), new, last.clone()).unwrap();
        publish(
            dir.path(),
            old,
            Report::new(Status::Done, "Text inserted", None),
        )
        .unwrap();
        assert_eq!(read_result(dir.path()).unwrap().report, last);
    }
}
