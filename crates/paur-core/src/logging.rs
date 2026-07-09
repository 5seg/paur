//! Tracing setup. Uses `RUST_LOG` if set, otherwise `info` for paur crates.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialise a process-wide tracing subscriber. Idempotent: subsequent
/// calls are a no-op (subscriber is global state). Returns whether setup
/// actually happened, which is mostly useful for tests.
pub fn init() -> bool {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,paur=debug,sqlx=warn"));
    let layer = fmt::layer().with_target(false).with_writer(std::io::stderr);
    let res = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
    res.is_ok()
}

/// Initialise with a fixed filter string. Useful for tests.
pub fn init_with(filter: &str) -> bool {
    let layer = fmt::layer().with_target(false).with_writer(std::io::stderr);
    tracing_subscriber::registry()
        .with(EnvFilter::new(filter))
        .with(layer)
        .try_init()
        .is_ok()
}
