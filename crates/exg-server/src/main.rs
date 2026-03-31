use metrics_exporter_prometheus::PrometheusBuilder;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .json()
        .init();

    tracing::info!("EXG Exchange Server starting...");
    tracing::info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // Initialize Prometheus metrics exporter on port 9000
    let builder = PrometheusBuilder::new();
    builder
        .with_http_listener(([0, 0, 0, 0], 9000))
        .install()
        .expect("Failed to install Prometheus exporter");

    tracing::info!("Prometheus metrics exporter listening on :9000");

    // Register key metrics
    metrics::describe_histogram!(
        "exg_matching_engine_latency_seconds",
        "Matching engine order processing latency"
    );
    metrics::describe_counter!(
        "exg_orders_total",
        "Total number of orders processed"
    );
    metrics::describe_gauge!(
        "exg_active_positions",
        "Number of active positions"
    );
    metrics::describe_gauge!(
        "exg_insurance_fund_balance",
        "Insurance fund balance in quote currency"
    );
    metrics::describe_counter!(
        "exg_api_requests_total",
        "Total API requests"
    );
    metrics::describe_gauge!(
        "exg_websocket_connections",
        "Active WebSocket connections"
    );

    // TODO: Initialize exchange components
    // 1. Load config
    // 2. Initialize WAL
    // 3. Initialize Ring Buffers
    // 4. Start Matching Engine
    // 5. Start Clearing Service
    // 6. Start Market Data Service
    // 7. Start API Gateway (HTTP + WS)
    // 8. Start Wallet Service scanners

    tracing::info!("EXG Exchange Server ready");

    // Keep running until signal
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl+c");
    tracing::info!("Shutting down...");
}
