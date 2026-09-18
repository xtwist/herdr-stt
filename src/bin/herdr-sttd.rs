#[tokio::main]
async fn main() -> anyhow::Result<()> {
    herdr_stt::daemon::run().await
}
