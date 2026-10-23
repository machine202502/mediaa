//! The engine's spec of the models chosen in the settings.

use mediaa_core::agent::ModelSource;

/// The engine's spec of a model: `owner/repo` for a Hugging Face one, the absolute path for a
/// local one. A provider's model does not run locally. The engine reports a loaded model by the
/// same name, which is how the agent recognizes its own model in [`mediaa_engine::Status`].
pub fn local_spec(source: &ModelSource) -> anyhow::Result<String> {
    mediaa_engine::runtime::source_name(source)
}
