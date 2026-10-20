//! The engine the live (`#[ignore]`d) tests run models on: the models folder from
//! `MEDIAA_TEST_MODELS`, else the app's (`mediaa_core::paths::models_dir()`), under the default
//! budget.

use mediaa_core::agent::Budget;
use mediaa_engine::{Engine, LocalEngine};
use std::sync::Arc;

pub fn engine() -> Arc<dyn Engine> {
    let dir = std::env::var_os("MEDIAA_TEST_MODELS").map_or_else(mediaa_core::paths::models_dir, std::path::PathBuf::from);
    Arc::new(LocalEngine::new(dir, Budget::default().into()))
}
