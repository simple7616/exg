//! Stage 0 server binary. Delegates to `exg_server::run_with_config`.

use std::path::PathBuf;

use anyhow::Result;
use metrics_exporter_prometheus::PrometheusBuilder;

#[actix_web::main]
async fn main() -> Result<()> {
    // Tracing
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .json()
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "exg-server stage 0 starting"
    );

    // Prometheus exporter (preserved from prior main.rs; spec §4.5 step 12)
    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], 9000))
        .install()
        .expect("Failed to install Prometheus exporter");

    metrics::describe_counter!("exg_api_requests_total", "Total API requests");
    metrics::describe_histogram!(
        "exg_matching_engine_latency_seconds",
        "Matching engine order processing latency"
    );

    // Config
    let cfg_path: PathBuf = std::env::var_os("EXG_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/default.toml"));
    let cfg = exg_config::ExgConfig::load(&cfg_path)?;

    // Boot
    let handle = exg_server::run_with_config(cfg).await?;
    tracing::info!(port = handle.bound_port, "exg-server ready");

    // Wait for ctrl_c, then graceful shutdown (spec §4.6).
    tokio::signal::ctrl_c().await.expect("ctrl_c handler");
    tracing::info!("ctrl_c received, shutting down");
    handle.shutdown().await?;
    tracing::info!("exg-server stage 0 stopped");
    Ok(())
}
