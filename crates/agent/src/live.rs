use mediaa_core::agent::Connection;
use mediaa_engine::{Connections, Engine};
use std::sync::Arc;

pub fn engine() -> Arc<dyn Engine> {
    let connections = std::env::var("MEDIAA_LIVE_CONNECTIONS")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| connections_from(&text))
        .unwrap_or_default();
    Arc::new(Connections::new(connections))
}

fn connections_from(text: &str) -> Vec<Connection> {
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(text) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|value| {
            let mut connection: Connection = serde_json::from_value(value.clone()).ok()?;
            if let Some(key) = value.get("api_key").and_then(|key| key.as_str()) {
                connection.api_key = key.to_string();
            }
            Some(connection)
        })
        .collect()
}
