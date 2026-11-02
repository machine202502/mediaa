use std::time::Duration;

pub fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("mediaa-backend")
        .build()
        .map_err(|error| anyhow::anyhow!("backend runtime: {error}"))
}

pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
