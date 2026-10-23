//! "Check models": every role in turn — the engine downloads a model if missing and tries it
//! for the role, provider models are looked up with the provider's key.

use crate::assistant::hf_token;
use crate::providers;
use mediaa_core::agent::{AgentSettings, ModelRole, ModelSource};
use mediaa_engine::{role_name, Engine, ModelInspect};

/// Why a role is fine.
#[derive(Debug, Clone, PartialEq)]
pub enum Ready {
    /// The engine loaded the model and ran it once (its size).
    Ran { bytes: u64 },
    /// Served by a provider.
    Provider,
    /// Optional role left unset.
    NotUsed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Checking,
    Ready(Ready),
    Failed(String),
}

/// Why a model does not fit `role`, from what `inspect` found.
fn unfit(role: ModelRole, inspect: &ModelInspect) -> String {
    let key = match role {
        ModelRole::SpeechRecognition => "sherpa",
        ModelRole::Diarization => "speaker",
        _ => "GGUF",
    };
    inspect
        .problems
        .iter()
        .find(|problem| problem.contains(key))
        .cloned()
        .unwrap_or_else(|| format!("the model does not fit the {} role", role_name(role)))
}

/// Asks the engine to download the model (if a Hugging Face one) and try it for the role.
fn check_local(engine: &dyn Engine, role: ModelRole, source: &ModelSource, token: Option<&str>) -> Status {
    let spec = match crate::models::local_spec(source) {
        Ok(spec) => spec,
        Err(e) => return Status::Failed(format!("{e:#}")),
    };
    // Inspection tells the format and the roles a model fits without downloading it: a model
    // that cannot work in this role fails here, not after a multi-gigabyte download.
    match engine.inspect(&spec, token) {
        // Speech synthesis has no runtime in the engine yet: its files are all there is to try.
        Ok(inspect) if role != ModelRole::Speech && !inspect.roles.iter().any(|fits| fits == role_name(role)) => {
            return Status::Failed(unfit(role, &inspect))
        }
        Ok(_) => {}
        // A local path that cannot be listed or a repository that cannot be reached: the
        // engine's check reports the real problem.
        Err(e) => log::info!("checking {spec} without inspection: {e:#}"),
    }
    match engine.check_model(&spec, role, token) {
        Ok(check) if check.ok => Status::Ready(Ready::Ran { bytes: check.bytes }),
        Ok(_) => Status::Failed("the model did not pass the check".into()),
        Err(e) => Status::Failed(format!("{e:#}")),
    }
}

fn check_role(agent: &AgentSettings, engine: &dyn Engine, role: ModelRole, token: Option<&str>) -> Status {
    let source = match agent.models.get(role) {
        Some(s) => s,
        None if role.required() => return Status::Failed("no model selected".into()),
        None => return Status::Ready(Ready::NotUsed),
    };
    match source {
        ModelSource::Local { .. } | ModelSource::HuggingFace { .. } => check_local(engine, role, source, token),
        ModelSource::Provider { provider, model } => match agent.provider(provider) {
            None => Status::Failed("the provider was removed".into()),
            Some(p) => match providers::check_model(p, model) {
                Ok(()) => Status::Ready(Ready::Provider),
                Err(e) => Status::Failed(format!("{e:#}")),
            },
        },
    }
}

/// Checks every role in order, reporting each status change through `report`. Returns
/// whether every role is ready.
pub fn check_all(agent: &AgentSettings, engine: &dyn Engine, report: impl Fn(ModelRole, Status) + Send + Sync + 'static) -> bool {
    let token = hf_token(agent);
    let mut ok = true;
    for role in ModelRole::ALL {
        report(role, Status::Checking);
        let status = check_role(agent, engine, role, token.as_deref());
        ok &= matches!(status, Status::Ready(_));
        report(role, status);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::ModelChoices;
    use mediaa_engine::testing::FakeEngine;
    use std::sync::{Arc, Mutex};

    #[test]
    fn roles_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let chat = ModelSource::Local { path: dir.path().join("chat.gguf") };
        // A repository the engine cannot run and a folder with a speaker embedding model.
        let bad = ModelSource::HuggingFace { repo: "bad/model".into() };
        let wrong_role = ModelSource::Local { path: dir.path().join("embedding.onnx") };
        let voice = ModelSource::Local { path: dir.path().join("voice.onnx") };
        let agent = AgentSettings {
            providers: vec![],
            models: ModelChoices {
                embedding: Some(chat.clone()),
                reranker: Some(wrong_role),
                agent: Some(bad),
                titles: Some(ModelSource::Provider { provider: "gone".into(), model: "x".into() }),
                speech_recognition: None,
                diarization: None,
                speech: Some(voice),
            },
            ..AgentSettings::default()
        };
        let engine = FakeEngine::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let ok = check_all(&agent, &engine, move |role, status| s.lock().unwrap().push((role, status)));
        assert!(!ok);
        let last = |role| seen.lock().unwrap().iter().rev().find(|(r, _)| *r == role).map(|(_, s)| s.clone()).unwrap();
        assert_eq!(last(ModelRole::Embedding), Status::Ready(Ready::Ran { bytes: 123 }), "the engine tried it");
        assert!(matches!(last(ModelRole::Agent), Status::Failed(e) if e.contains("GGUF")), "the engine's error is reported");
        assert!(matches!(last(ModelRole::Reranker), Status::Failed(e) if e.contains("GGUF")), "a model of the wrong kind fails before a download");
        assert!(matches!(last(ModelRole::Titles), Status::Failed(e) if e.contains("removed")));
        assert!(matches!(last(ModelRole::SpeechRecognition), Status::Failed(e) if e.contains("no model")), "required");
        assert_eq!(last(ModelRole::Diarization), Status::Ready(Ready::NotUsed), "optional");
        assert_eq!(last(ModelRole::Speech), Status::Ready(Ready::Ran { bytes: 123 }));
        assert_eq!(seen.lock().unwrap().iter().filter(|(_, s)| *s == Status::Checking).count(), ModelRole::ALL.len());
        assert!(!engine.checks().iter().any(|(spec, _)| spec.ends_with("embedding.onnx")), "no useless download: the check stopped at the kind of model");
    }
}
