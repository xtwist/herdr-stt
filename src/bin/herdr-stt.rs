#[tokio::main]
async fn main() -> anyhow::Result<()> {
    herdr_stt::cli::run().await
}
