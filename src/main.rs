use std::error::Error;
use std::path::PathBuf;

use clap::Parser;
use http_credential_broker::{BrokerConfig, serve_with_shutdown};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the broker TOML configuration.
    #[arg(long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("http_credential_broker=info,tower_http=warn")),
        )
        .init();

    let args = Args::parse();
    let config = BrokerConfig::from_path(&args.config)?;
    let listener = TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;

    tracing::info!(%addr, services = config.services.len(), "http credential broker listening");

    serve_with_shutdown(listener, config, async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "failed to listen for ctrl-c");
        }
    })
    .await?;

    Ok(())
}
