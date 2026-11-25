//! Real Rust daemon and JSON-lines worker, isolated from microphones, models and services.
use herdr_stt::protocol::{API_VERSION, Request, Response};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    process::Command,
};

async fn call(socket: &Path, method: &str, params: Value) -> Response {
    let mut stream = UnixStream::connect(socket).await.unwrap();
    let request = Request {
        version: API_VERSION,
        request_id: "test".into(),
        method: method.into(),
        params,
    };
    let payload = serde_json::to_string(&request).unwrap() + "\n";
    stream.write_all(payload.as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(stream).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn context_reaches_worker_errors_recover_and_status_does_not_wait_for_model() {
    let temp = tempfile::tempdir().unwrap();
    let worker = temp.path().join("fake-worker");
    std::fs::write(&worker, r##"#!/usr/bin/env python3
import json, sys, time
from pathlib import Path
print(json.dumps(dict(type='ready', model='fixture', device='cpu', compute_type='int8')), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    time.sleep(2.0 if request['audio_path'].endswith('hang.wav') else 0.35)
    audio = Path(request['audio_path'])
    if audio.name == 'fail.wav' or (
        audio.name.startswith('recording-') and Path(__file__).with_name('worker-fail').exists()
    ):
        print(json.dumps(dict(type='response', id=request['id'], ok=False, error='test failure')), flush=True)
        continue
    result = dict(text=request['initial_prompt'], language=request['language'], language_probability=1.0,
                  duration_seconds=1.0, duration_after_vad_seconds=1.0, segments=[])
    print(json.dumps(dict(type='response', id=request['id'], ok=True, result=result)), flush=True)
"##).unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    let recorder = temp.path().join("pw-record");
    std::fs::write(
        &recorder,
        r##"#!/usr/bin/env python3
import signal, sys, time
from pathlib import Path
if Path(__file__).with_name('recorder-fail').exists(): sys.exit(1)
Path(sys.argv[-1]).write_text('fixture')
signal.signal(signal.SIGINT, lambda *_: sys.exit(0))
if Path(__file__).with_name('recorder-exit').exists():
    time.sleep(0.45)
    sys.exit(1)
while True: time.sleep(0.05)
"##,
    )
    .unwrap();
    std::fs::set_permissions(&recorder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let background = temp.path().join("background.txt");
    std::fs::write(&background, "").unwrap();
    let config = temp.path().join("config.toml");
    let mut settings = herdr_stt::config::Config::default();
    settings.worker.python = Some(worker.display().to_string());
    settings.audio.max_seconds = 1;
    settings.history.enabled = false;
    settings.context.files = vec![background.clone()];
    settings.worker.transcription_timeout_seconds = 1;
    settings.transcription.initial_prompt.clear();
    settings.context.profiles.insert(
        "project".into(),
        herdr_stt::context::Guidance {
            text: "Project background".into(),
            language: Some("uk".into()),
            ..Default::default()
        },
    );
    std::fs::write(&config, toml::to_string(&settings).unwrap()).unwrap();
    let socket = temp.path().join("stt.sock");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_herdr-sttd"))
        .arg("--config")
        .arg(&config)
        .env("HERDR_STT_SOCKET", &socket)
        .env(
            "PATH",
            format!(
                "{}:{}",
                temp.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("XDG_RUNTIME_DIR", temp.path())
        .env("HERDR_PLUGIN_STATE_DIR", temp.path().join("state"))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        assert!(
            daemon.try_wait().unwrap().is_none(),
            "daemon exited during startup"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(call(&socket, "ping", json!({})).await.ok);
    let audio = temp.path().join("sample.wav");
    std::fs::write(&audio, "fixture").unwrap();
    let bad = call(
        &socket,
        "transcribe_file",
        json!({"path": audio, "recognition":{"profile":"missing"}}),
    )
    .await;
    assert!(!bad.ok);
    assert_eq!(bad.error.unwrap().code, "invalid_context");
    let socket_copy = socket.clone();
    let pending = tokio::spawn(async move {
        call(
            &socket_copy,
            "transcribe_file",
            json!({"path": audio, "recognition": {"profile":"project", "text":"Task background"}}),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let status = tokio::time::timeout(
        Duration::from_millis(150),
        call(&socket, "status", json!({})),
    )
    .await
    .expect("status blocked on worker");
    assert_eq!(status.result.unwrap()["state"], "transcribing");
    let transcript = pending.await.unwrap().result.unwrap();
    assert_eq!(transcript["text"], "Task background\n\nProject background");
    assert_eq!(transcript["language"], "uk");
    assert_eq!(transcript["context_profile"], "project");
    let fail = temp.path().join("fail.wav");
    std::fs::write(&fail, "fixture").unwrap();
    assert!(
        !call(&socket, "transcribe_file", json!({"path": fail}))
            .await
            .ok
    );
    assert_eq!(
        call(&socket, "status", json!({})).await.result.unwrap()["state"],
        "idle"
    );
    assert_eq!(
        call(&socket, "last", json!({})).await.result.unwrap()["context_profile"],
        "project"
    );
    let hang = temp.path().join("hang.wav");
    std::fs::write(&hang, "fixture").unwrap();
    assert!(
        !call(&socket, "transcribe_file", json!({"path": hang}))
            .await
            .ok
    );
    assert_eq!(
        call(&socket, "status", json!({})).await.result.unwrap()["state"],
        "idle"
    );
    let recovery = temp.path().join("recovery.wav");
    std::fs::write(&recovery, "fixture").unwrap();
    assert!(
        call(&socket, "transcribe_file", json!({"path": recovery}))
            .await
            .ok
    );
    std::fs::write(&background, "Original snapshot").unwrap();
    assert!(
        call(
            &socket,
            "start",
            json!({"context": {"pane_id":"original-pane"}})
        )
        .await
        .ok
    );
    std::fs::write(&background, "New background for the next recording").unwrap();
    assert_eq!(
        call(&socket, "status", json!({})).await.result.unwrap()["capture"],
        "active"
    );
    wait_for_capture(&socket, "recorded").await;
    let stopped = call(&socket, "stop", json!({})).await.result.unwrap();
    assert_eq!(stopped["text"], "Original snapshot");
    assert_eq!(stopped["context"]["pane_id"], "original-pane");
    assert!(call(&socket, "start", json!({})).await.ok);
    assert!(call(&socket, "cancel", json!({})).await.ok);

    let recorder_exit = temp.path().join("recorder-exit");
    std::fs::write(&recorder_exit, "").unwrap();
    assert!(call(&socket, "start", json!({})).await.ok);
    wait_for_capture(&socket, "failed").await;
    assert!(call(&socket, "cancel", json!({})).await.ok);
    std::fs::remove_file(recorder_exit).unwrap();

    std::fs::write(&background, "").unwrap();
    verify_cli_feedback(temp.path(), &socket, &config).await;
    assert!(
        std::fs::read_dir(temp.path().join("herdr-stt"))
            .unwrap()
            .all(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_none_or(|ext| ext != "wav"))
    );
    unsafe {
        libc::kill(daemon.id().unwrap() as i32, libc::SIGTERM);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(3), daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!socket.exists());
}

async fn wait_for_capture(socket: &Path, expected: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let result = call(socket, "status", json!({})).await.result.unwrap();
            if result["capture"] == expected {
                assert_eq!(result["state"], "recording");
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("capture did not reach {expected}"));
}

async fn verify_cli_feedback(dir: &Path, socket: &Path, config: &Path) {
    let python = dir.join("python/.venv/bin/python");
    std::fs::create_dir_all(python.parent().unwrap()).unwrap();
    std::fs::write(&python, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o700)).unwrap();
    let herdr = dir.join("fake-herdr");
    let command_log = dir.join("herdr-commands");
    std::fs::write(
        &herdr,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$HERDR_FIXTURE_LOG"
case "$1 $2" in
  "workspace list") echo '{"result":{"workspaces":[]}}' ;;
  "pane send-text") test -z "$HERDR_FIXTURE_ROUTE_FAILURE" ;;
  *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o700)).unwrap();

    let cli = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_herdr-stt"));
        command
            .env("HERDR_STT_SOCKET", socket)
            .env("HERDR_STT_CONFIG", config)
            .env("HERDR_STT_PROJECT_ROOT", dir)
            .env("HERDR_BIN_PATH", &herdr)
            .env("HERDR_FIXTURE_LOG", &command_log)
            .env("HERDR_PLUGIN_ID", "herdr.stt")
            .env("HERDR_PANE_ID", "original-pane")
            .env("XDG_RUNTIME_DIR", dir)
            .current_dir(dir);
        command
    };
    let report = || {
        serde_json::from_slice::<Value>(
            &std::fs::read(dir.join("herdr-stt-tray/result.json")).unwrap(),
        )
        .unwrap()["report"]
            .clone()
    };
    let results = [
        (
            "Test dictation",
            "toggle",
            None,
            "Done",
            "Voice text inserted",
        ),
        ("", "toggle", None, "NoSpeech", "No speech detected"),
        ("", "cancel", None, "Cancelled", "Recording cancelled"),
        (
            "Test dictation",
            "toggle",
            Some("worker-fail"),
            "Error",
            "Voice input error",
        ),
        (
            "Test dictation",
            "toggle",
            Some("route-fail"),
            "Error",
            "Voice input error",
        ),
    ];
    for (hint, stop_command, failure, expected_status, title) in results {
        let start = cli()
            .args(["toggle", "--context", hint])
            .output()
            .await
            .unwrap();
        assert!(start.status.success(), "{:?}", start.stderr);
        let mut stop = cli();
        if let Some(failure) = failure {
            if failure == "worker-fail" {
                std::fs::write(dir.join(failure), "").unwrap();
            } else {
                stop.env("HERDR_FIXTURE_ROUTE_FAILURE", "1");
            }
        }
        let result = stop.arg(stop_command).output().await.unwrap();
        assert_eq!(
            result.status.success(),
            failure.is_none(),
            "{:?}",
            result.stderr
        );
        if failure == Some("worker-fail") {
            std::fs::remove_file(dir.join("worker-fail")).unwrap();
        }
        assert_eq!(report()["status"], expected_status);
        assert_eq!(report()["title"], title);
    }
    std::fs::write(dir.join("recorder-fail"), "").unwrap();
    let bad_start = cli().arg("toggle").output().await.unwrap();
    assert!(!bad_start.status.success());
    assert_eq!(report()["detail"], "Unable to start recording");
    let calls = std::fs::read_to_string(command_log).unwrap();
    assert!(calls.contains("pane send-text original-pane Test dictation"));
    assert!(!calls.contains("notification"));
}
