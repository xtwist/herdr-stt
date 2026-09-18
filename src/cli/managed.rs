use super::{Config, ok, paths, print_json, setup};
use crate::corrector::{
    LLAMA_CPP_ARCHIVE, LLAMA_CPP_SHA256, LLAMA_CPP_TAG, LLAMA_CPP_URL, QWEN_FILE, QWEN_SHA256,
    QWEN_URL,
};
use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use std::time::Duration;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::Command,
};

pub(super) fn corrector_install() -> Result<()> {
    anyhow::ensure!(
        std::env::consts::ARCH == "x86_64",
        "managed corrector installer supports Linux x86_64; configure your own binary and model on other architectures"
    );
    setup(false)?;
    let root = paths::corrector_dir();
    fs::create_dir_all(root.join("models"))?;

    let archive = root.join(format!("{LLAMA_CPP_ARCHIVE}.download"));
    download_verified(LLAMA_CPP_URL, &archive, LLAMA_CPP_SHA256)?;
    let staging = root.join(format!("runtime.next-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .arg("--strip-components=1")
        .status()
        .context("failed to extract llama.cpp runtime")?;
    anyhow::ensure!(status.success(), "failed to extract llama.cpp runtime");
    let runtime = root.join("runtime");
    let old_runtime = root.join("runtime.old");
    if old_runtime.exists() {
        fs::remove_dir_all(&old_runtime)?;
    }
    if runtime.exists() {
        fs::rename(&runtime, &old_runtime)?;
    }
    fs::rename(&staging, &runtime)?;
    if old_runtime.exists() {
        fs::remove_dir_all(&old_runtime)?;
    }
    let _ = fs::remove_file(&archive);

    let model = paths::corrector_model();
    download_verified(QWEN_URL, &model, QWEN_SHA256)?;
    fs::set_permissions(paths::corrector_binary(), fs::Permissions::from_mode(0o755))?;
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    println!(
        "Installed llama.cpp {LLAMA_CPP_TAG} and {QWEN_FILE}. Set correction.enabled=true and correction.backend=\"managed\" in {}.",
        paths::config_path().display()
    );
    Ok(())
}

fn download_verified(url: &str, destination: &Path, expected_sha256: &str) -> Result<()> {
    if destination.is_file() && sha256(destination).is_ok_and(|actual| actual == expected_sha256) {
        return Ok(());
    }
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let status = Command::new("curl")
        .args(["--fail", "--location", "--retry", "3", "--continue-at", "-"])
        .arg("--output")
        .arg(destination)
        .arg(url)
        .status()
        .with_context(|| format!("failed to download {url}"))?;
    anyhow::ensure!(status.success(), "download failed: {url}");
    let actual = sha256(destination)?;
    if actual != expected_sha256 {
        let _ = fs::remove_file(destination);
        return Err(anyhow!(
            "checksum mismatch for {}: expected {}, got {}; the invalid file was removed",
            destination.display(),
            expected_sha256,
            actual
        ));
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .context("failed to execute sha256sum")?;
    anyhow::ensure!(
        output.status.success(),
        "sha256sum failed for {}",
        path.display()
    );
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("sha256sum returned no digest"))
}

#[derive(Serialize)]
struct CorrectorStatus {
    enabled: bool,
    backend: String,
    binary: String,
    binary_ok: bool,
    model: String,
    model_ok: bool,
    service_active: bool,
    endpoint: String,
    endpoint_healthy: bool,
}

pub(super) async fn corrector_status(json_output: bool) -> Result<()> {
    let config = Config::load()?;
    let binary = config
        .correction
        .managed
        .binary
        .clone()
        .unwrap_or_else(paths::corrector_binary);
    let model = config
        .correction
        .managed
        .model_path
        .clone()
        .unwrap_or_else(paths::corrector_model);
    let service_active = Command::new("systemctl")
        .args([
            "--user",
            "is-active",
            "--quiet",
            "herdr-stt-corrector.service",
        ])
        .status()
        .is_ok_and(|status| status.success());
    let health_url = format!(
        "{}/health",
        config
            .correction
            .endpoint
            .trim_end_matches("/v1")
            .trim_end_matches('/')
    );
    let endpoint_healthy = reqwest::Client::new()
        .get(health_url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success());
    let report = CorrectorStatus {
        enabled: config.correction.enabled,
        backend: format!("{:?}", config.correction.backend).to_lowercase(),
        binary: binary.display().to_string(),
        binary_ok: binary.is_file(),
        model: model.display().to_string(),
        model_ok: model.is_file(),
        service_active,
        endpoint: config.correction.endpoint,
        endpoint_healthy,
    };
    if json_output {
        print_json(&report)
    } else {
        println!("enabled={} backend={}", report.enabled, report.backend);
        println!("binary: {} ({})", report.binary, ok(report.binary_ok));
        println!("model: {} ({})", report.model, ok(report.model_ok));
        println!("service active: {}", report.service_active);
        println!(
            "endpoint: {} (healthy={})",
            report.endpoint, report.endpoint_healthy
        );
        Ok(())
    }
}

pub(super) fn corrector_serve() -> Result<()> {
    let config = Config::load()?;
    anyhow::ensure!(
        config.correction.enabled
            && config.correction.backend == crate::config::CorrectionBackend::Managed,
        "managed corrector is not enabled in config"
    );
    let binary = config
        .correction
        .managed
        .binary
        .clone()
        .unwrap_or_else(paths::corrector_binary);
    let model = config
        .correction
        .managed
        .model_path
        .clone()
        .unwrap_or_else(paths::corrector_model);
    anyhow::ensure!(
        binary.is_file(),
        "llama-server is not installed; run `herdr-stt corrector install`"
    );
    anyhow::ensure!(
        model.is_file(),
        "Qwen model is not installed; run `herdr-stt corrector install`"
    );
    let endpoint = reqwest::Url::parse(&config.correction.endpoint)?;
    let host = endpoint.host_str().unwrap_or("127.0.0.1");
    let port = endpoint.port_or_known_default().unwrap_or(18118);
    let runtime = binary.parent().context("llama-server path has no parent")?;
    let mut command = Command::new(&binary);
    command
        .arg("--model")
        .arg(model)
        .args(["--alias", &config.correction.model])
        .args(["--host", host])
        .args(["--port", &port.to_string()])
        .args([
            "--ctx-size",
            &config.correction.managed.context_tokens.to_string(),
        ])
        .args(["--parallel", "1"])
        .args(["--gpu-layers", &config.correction.managed.gpu_layers])
        .args([
            "--sleep-idle-seconds",
            &config.correction.managed.idle_seconds.to_string(),
        ])
        .args(["--reasoning", "off"])
        .arg("--jinja")
        .env("LD_LIBRARY_PATH", runtime);
    let error = command.exec();
    Err(anyhow!("failed to execute {}: {error}", binary.display()))
}
