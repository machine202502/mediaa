use mediaa_core::agent::Connection;
use mediaa_engine::{Connections, Engine};
use std::sync::Arc;

pub fn engine() -> Arc<dyn Engine> {
    let connections = std::env::var("MEDIAA_LIVE_CONNECTIONS")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Vec<Connection>>(&text).ok())
        .unwrap_or_default();
    Arc::new(Connections::new(connections))
}
