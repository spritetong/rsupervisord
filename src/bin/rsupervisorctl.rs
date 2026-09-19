#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rsupervisord::cli::run().await
}
