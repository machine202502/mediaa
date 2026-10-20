//! The engine's spec of the models chosen in the settings.

use anyhow::Context;
use mediaa_core::agent::ModelSource;

/// The engine's spec of a model: `owner/repo` for a Hugging Face one, the absolute path for a
/// local one. A provider's model does not run in the engine.
pub fn local_spec(source: &ModelSource) -> anyhow::Result<String> {
    match source {
        ModelSource::HuggingFace { repo } => Ok(repo.trim().to_string()),
        ModelSource::Local { path } => {
            let path = std::path::absolute(path).with_context(|| format!("the path of a model: {}", path.display()))?;
            Ok(path.display().to_string())
        }
        ModelSource::Provider { .. } => anyhow::bail!("a provider's model does not run in the engine"),
    }
}
