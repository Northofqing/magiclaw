use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Initialize tracing. Business logs go to stderr only (MCP stdio zero-pollution).
/// stdout is reserved for protocol output (MCP JSON-RPC).
pub fn init_tracing(default_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .json()
        .with_target(true);

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_tracing_does_not_panic() {
        init_tracing("warn");
        tracing::info!("this should not appear with warn level");
        tracing::warn!("tracing initialized successfully");
    }
}
