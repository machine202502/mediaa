//! "Check models": every role in turn — Hugging Face models are downloaded if missing, local
//! models validated, provider models looked up with the provider's key.

use crate::{hub, local, providers, speech};
use mediaa_core::agent::{AgentSettings, ModelRole, ModelSource};
use std::path::Path;
use std::sync::Arc;

/// Why a role is fine.
#[derive(Debug, Clone, PartialEq)]
pub enum Ready {
    /// Model weights on disk: their count and total size.
    Files { files: usize, bytes: u64 },
    /// A language model that loaded and generated (its size).
    Ran { bytes: u64 },
    /// Served by a provider.
    Provider,
    /// Optional role left unset.
    NotUsed,
    /// The sub-agent uses the main agent's model.
    SameAsAgent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Checking,
    Downloading { done: u64, total: u64 },
    Ready(Ready),
    Failed(String),
}

/// Loads a GGUF model on the CPU with a small context and generates one token.
fn try_generate(path: &Path) -> anyhow::Result<()> {
    let model = crate::llm::LocalModel::load(path, false, 256)?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    model.complete("Hello", &crate::llm::Generate { max_tokens: 1, ..Default::default() }, &cancel, &mut |_| true)?;
    Ok(())
}

/// Downloads a model that comes with a role's model (voice activity, speaker segmentation).
fn companion(ai_dir: &Path, agent: &AgentSettings, spec: &str) -> anyhow::Result<()> {
    hub::download(ai_dir, spec, agent.hugging_face(), |_, _| {}).map(drop)
}

fn check_role(agent: &AgentSettings, ai_dir: &Path, role: ModelRole, report: &Arc<dyn Fn(ModelRole, Status) + Send + Sync>) -> Status {
    let source = match agent.models.get(role) {
        Some(s) => s,
        None if role == ModelRole::Subagent => return Status::Ready(Ready::SameAsAgent),
        None if role.required() => return Status::Failed("no model selected".into()),
        None => return Status::Ready(Ready::NotUsed),
    };
    // Models that run are loaded (a language model also answers one token): a broken or
    // unsupported model fails here, not later in a chat or a transcription.
    let files = |path: &Path| match local::check(path) {
        Ok((files, bytes)) => {
            let gguf = files.iter().find(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("gguf")));
            let run = match (role, gguf) {
                (ModelRole::SpeechRecognition, _) => companion(ai_dir, agent, speech::VAD_MODEL).and_then(|_| speech::Recognizer::load(path).map(drop)),
                (ModelRole::Diarization, _) => companion(ai_dir, agent, speech::SEGMENTATION_MODEL).and_then(|_| speech::check_embedding(path)),
                (ModelRole::Embedding, Some(g)) => crate::embed::Embedder::load(g, false).and_then(|e| e.passage("проверка").map(drop)),
                (ModelRole::Reranker, Some(g)) => crate::embed::Reranker::load(g, false).and_then(|r| r.score("вопрос", "ответ").map(drop)),
                (_, Some(g)) => try_generate(g),
                // Speech synthesis has no runtime yet: its files are all there is to check.
                (ModelRole::Speech, None) => return Status::Ready(Ready::Files { files: files.len(), bytes }),
                // The rest run through llama.cpp: without a GGUF file nothing can load.
                (_, None) => Err(anyhow::anyhow!(
                    "{}: no GGUF model inside (llama.cpp needs one): choose a repository with GGUF files, usually named …-GGUF",
                    path.display()
                )),
            };
            match run {
                Ok(()) => Status::Ready(Ready::Ran { bytes }),
                Err(e) => Status::Failed(format!("{e:#}")),
            }
        }
        Err(e) => Status::Failed(format!("{e:#}")),
    };
    match source {
        ModelSource::Local { path } => files(path),
        ModelSource::HuggingFace { repo } => {
            let r = report.clone();
            match hub::download(ai_dir, repo, agent.hugging_face(), move |done, total| r(role, Status::Downloading { done, total })) {
                Ok(dir) => files(&hub::split_spec(repo).1.map_or(dir.clone(), |f| dir.join(f))),
                Err(e) => Status::Failed(format!("{e:#}")),
            }
        }
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
pub fn check_all(agent: &AgentSettings, ai_dir: &Path, report: impl Fn(ModelRole, Status) + Send + Sync + 'static) -> bool {
    let report: Arc<dyn Fn(ModelRole, Status) + Send + Sync> = Arc::new(report);
    let mut ok = true;
    for role in ModelRole::ALL {
        report(role, Status::Checking);
        let status = check_role(agent, ai_dir, role, &report);
        ok &= matches!(status, Status::Ready(_));
        report(role, status);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::ModelChoices;
    use std::sync::Mutex;

    #[test]
    fn roles_without_network() {
        let dir = tempfile::tempdir().unwrap();
        // A file with a GGUF signature but no model in it: the load must catch it.
        let gguf = dir.path().join("m.gguf");
        std::fs::write(&gguf, [b"GGUF".as_slice(), &3u32.to_le_bytes()].concat()).unwrap();
        let fake_llm = ModelSource::Local { path: gguf };
        let onnx = dir.path().join("m.onnx");
        std::fs::write(&onnx, [0x08, 0x07]).unwrap();
        let local = ModelSource::Local { path: onnx };
        let agent = AgentSettings {
            providers: vec![],
            models: ModelChoices {
                embedding: Some(local.clone()),
                reranker: None,
                agent: Some(fake_llm),
                subagent: None,
                titles: Some(ModelSource::Provider { provider: "gone".into(), model: "x".into() }),
                speech_recognition: None,
                diarization: None,
                speech: Some(ModelSource::Local { path: dir.path().join("missing.onnx") }),
            },
            ..AgentSettings::default()
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let ok = check_all(&agent, dir.path(), move |role, status| s.lock().unwrap().push((role, status)));
        assert!(!ok);
        let last = |role| seen.lock().unwrap().iter().rev().find(|(r, _)| *r == role).map(|(_, s)| s.clone()).unwrap();
        assert!(matches!(last(ModelRole::Embedding), Status::Failed(e) if e.contains("no GGUF")), "a model llama.cpp cannot load fails the check");
        assert!(matches!(last(ModelRole::Agent), Status::Failed(_)), "a fake GGUF does not load");
        assert_eq!(last(ModelRole::Subagent), Status::Ready(Ready::SameAsAgent));
        assert!(matches!(last(ModelRole::Titles), Status::Failed(e) if e.contains("removed")));
        assert!(matches!(last(ModelRole::SpeechRecognition), Status::Failed(e) if e.contains("no model")), "required");
        assert_eq!(last(ModelRole::Diarization), Status::Ready(Ready::NotUsed), "optional");
        assert!(matches!(last(ModelRole::Speech), Status::Failed(_)));
        assert_eq!(seen.lock().unwrap().iter().filter(|(_, s)| *s == Status::Checking).count(), ModelRole::ALL.len());
    }
}
