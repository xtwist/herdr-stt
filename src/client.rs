use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::paths;
use crate::protocol::{API_VERSION, Request, Response};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

pub async fn call<T: DeserializeOwned>(method: &str, params: impl Serialize) -> Result<T> {
    let socket = paths::socket_path();
    let stream = UnixStream::connect(&socket)
        .await
        .with_context(|| format!("failed to connect to {}", socket.display()))?;
    let request_id = request_id();
    let request = Request {
        version: API_VERSION,
        request_id: request_id.clone(),
        method: method.into(),
        params: serde_json::to_value(params)?,
    };
    let (read_half, mut write_half) = stream.into_split();
    let mut payload = serde_json::to_vec(&request)?;
    payload.push(b'\n');
    write_half.write_all(&payload).await?;
    write_half.shutdown().await?;

    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(if matches!(method, "stop" | "transcribe_file") {
            1800
        } else {
            5
        }),
        BufReader::new(read_half).read_line(&mut line),
    )
    .await
    .context("daemon response timed out")??;
    if line.trim().is_empty() {
        return Err(anyhow!("daemon closed the socket without a response"));
    }
    let response: Response = serde_json::from_str(&line).context("invalid daemon response")?;
    anyhow::ensure!(
        response.version == API_VERSION,
        "unsupported_version: daemon uses API {}; run herdr-stt reload",
        response.version
    );
    if response.request_id != request_id {
        return Err(anyhow!("daemon response request_id mismatch"));
    }
    if !response.ok {
        let error = response
            .error
            .map(|error| format!("{}: {}", error.code, error.message))
            .unwrap_or_else(|| "unknown daemon error".into());
        return Err(anyhow!(error));
    }
    serde_json::from_value(response.result.unwrap_or(Value::Null)).context("invalid daemon result")
}

pub async fn ping() -> Result<()> {
    let _: Value = call("ping", json!({})).await?;
    Ok(())
}

fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{counter}", std::process::id())
}
