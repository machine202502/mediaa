use crate::engine::{
    ChatAnswer, ChatRequest, DiarizeRequest, EmbeddingsRequest, Engine, EngineError, ModelCheck,
    ModelInfo, RerankRequest, RerankResult, SpeechBody, TranscribeRequest, VoiceRequest,
};
use mediaa_core::agent::{ModelRole, ModelSource};
use mediaa_core::slots::Slots;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct Gated {
    inner: Arc<dyn Engine>,
    slots: Slots,
}

impl Gated {
    pub fn new(inner: Arc<dyn Engine>, slots: Slots) -> Gated {
        Gated { inner, slots }
    }

    fn hold(
        &self,
        connection: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<mediaa_core::slots::Lease>, EngineError> {
        let Some(ticket) = self.slots.provider(connection) else {
            return Ok(None);
        };
        ticket
            .wait(cancel)
            .map(Some)
            .map_err(|_| EngineError::cancelled("cancelled"))
    }
}

impl Engine for Gated {
    fn chat(
        &self,
        request: &ChatRequest,
        cancel: &AtomicBool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<ChatAnswer, EngineError> {
        let _slot = self.hold(&request.source.connection, cancel)?;
        self.inner.chat(request, cancel, on_text)
    }

    fn embeddings(
        &self,
        request: &EmbeddingsRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        let _slot = self.hold(&request.source.connection, cancel)?;
        self.inner.embeddings(request, cancel)
    }

    fn rerank(
        &self,
        request: &RerankRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<RerankResult>, EngineError> {
        let _slot = self.hold(&request.source.connection, cancel)?;
        self.inner.rerank(request, cancel)
    }

    fn transcribe(
        &self,
        request: &TranscribeRequest,
        cancel: &AtomicBool,
    ) -> Result<SpeechBody, EngineError> {
        let _slot = self.hold(&request.source.connection, cancel)?;
        self.inner.transcribe(request, cancel)
    }

    fn diarize(
        &self,
        request: &DiarizeRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<mediaa_core::transcript::Turn>, EngineError> {
        let _slot = self.hold(&request.source.connection, cancel)?;
        self.inner.diarize(request, cancel)
    }

    fn voice_embeddings(
        &self,
        request: &VoiceRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        let _slot = self.hold(&request.source.connection, cancel)?;
        self.inner.voice_embeddings(request, cancel)
    }

    fn models(&self, connection: &str) -> Result<Vec<ModelInfo>, EngineError> {
        let _slot = self
            .slots
            .provider(connection)
            .map(|ticket| ticket.wait(&AtomicBool::new(false)))
            .transpose()
            .map_err(|_| EngineError::cancelled("cancelled"))?;
        self.inner.models(connection)
    }

    fn check(&self, source: &ModelSource, role: ModelRole) -> Result<ModelCheck, EngineError> {
        let _slot = self
            .slots
            .provider(&source.connection)
            .map(|ticket| ticket.wait(&AtomicBool::new(false)))
            .transpose()
            .map_err(|_| EngineError::cancelled("cancelled"))?;
        self.inner.check(source, role)
    }

    fn pull(
        &self,
        connection: &str,
        model: &str,
        on_progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<(), EngineError> {
        let _slot = self.hold(connection, cancel)?;
        self.inner.pull(connection, model, on_progress, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ChatMessage, ChatRole, ModelInfo, RerankResult, SpeechBody};
    use mediaa_core::agent::{AgentSettings, Connection, ConnectionKind, Group};
    use mediaa_core::transcript::Turn;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct State {
        entered: usize,
        inside: usize,
        most: usize,
        release: bool,
    }

    #[derive(Default)]
    struct Blocking {
        state: Mutex<State>,
        changed: Condvar,
    }

    impl Blocking {
        fn wait_entered(&self, n: usize) {
            let mut state = self.state.lock().expect("state");
            while state.entered < n {
                state = self.changed.wait(state).expect("wait");
            }
        }

        fn release(&self) {
            self.state.lock().expect("state").release = true;
            self.changed.notify_all();
        }

        fn entered(&self) -> usize {
            self.state.lock().expect("state").entered
        }

        fn most_inside(&self) -> usize {
            self.state.lock().expect("state").most
        }
    }

    impl Engine for Blocking {
        fn chat(
            &self,
            _: &ChatRequest,
            _: &AtomicBool,
            _: &mut dyn FnMut(&str) -> bool,
        ) -> Result<ChatAnswer, EngineError> {
            let mut state = self.state.lock().expect("state");
            state.entered += 1;
            state.inside += 1;
            state.most = state.most.max(state.inside);
            self.changed.notify_all();
            while !state.release {
                state = self.changed.wait(state).expect("wait");
            }
            state.inside -= 1;
            self.changed.notify_all();
            Ok(ChatAnswer::default())
        }

        fn embeddings(
            &self,
            _: &EmbeddingsRequest,
            _: &AtomicBool,
        ) -> Result<Vec<Vec<f32>>, EngineError> {
            Err(EngineError::unavailable("not used"))
        }

        fn rerank(
            &self,
            _: &RerankRequest,
            _: &AtomicBool,
        ) -> Result<Vec<RerankResult>, EngineError> {
            Err(EngineError::unavailable("not used"))
        }

        fn transcribe(
            &self,
            _: &TranscribeRequest,
            _: &AtomicBool,
        ) -> Result<SpeechBody, EngineError> {
            Err(EngineError::unavailable("not used"))
        }

        fn diarize(&self, _: &DiarizeRequest, _: &AtomicBool) -> Result<Vec<Turn>, EngineError> {
            Err(EngineError::unavailable("not used"))
        }

        fn voice_embeddings(
            &self,
            _: &VoiceRequest,
            _: &AtomicBool,
        ) -> Result<Vec<Vec<f32>>, EngineError> {
            Err(EngineError::unavailable("not used"))
        }

        fn models(&self, _: &str) -> Result<Vec<ModelInfo>, EngineError> {
            Err(EngineError::unavailable("not used"))
        }

        fn check(&self, _: &ModelSource, _: ModelRole) -> Result<ModelCheck, EngineError> {
            Err(EngineError::unavailable("not used"))
        }
    }

    fn connection(id: &str, group: &str) -> Connection {
        Connection {
            id: id.into(),
            name: id.into(),
            kind: ConnectionKind::OpenAiCompatible,
            base_url: "http://localhost:1/v1".into(),
            api_key: String::new(),
            proxy: String::new(),
            group: group.into(),
        }
    }

    fn settings(places: &[(&str, &str)]) -> AgentSettings {
        AgentSettings {
            connections: places
                .iter()
                .map(|(id, group)| connection(id, group))
                .collect(),
            groups: vec![Group::new("Single", 1), Group::new("Local", 1)],
            ..AgentSettings::default()
        }
    }

    fn request(id: &str) -> ChatRequest {
        ChatRequest {
            source: ModelSource::new(id, "model"),
            messages: vec![ChatMessage::new(ChatRole::User, "hi")],
            ..ChatRequest::default()
        }
    }

    fn chat(
        gate: Arc<Gated>,
        id: &str,
    ) -> std::thread::JoinHandle<Result<ChatAnswer, EngineError>> {
        let request = request(id);
        std::thread::spawn(move || {
            let cancel = AtomicBool::new(false);
            gate.chat(&request, &cancel, &mut |_| true)
        })
    }

    #[test]
    fn an_unattached_provider_does_not_wait_for_a_full_group() {
        let settings = settings(&[("a", "Single"), ("b", "")]);
        let slots = Slots::new();
        slots.configure(&settings);
        let engine = Arc::new(Blocking::default());
        let gate = Arc::new(Gated::new(engine.clone(), slots.clone()));
        let _held = slots.provider("a").unwrap().try_take().unwrap();
        let second = chat(gate, "b");
        engine.wait_entered(1);
        assert_eq!(
            engine.entered(),
            1,
            "the unattached provider went to the model at once"
        );
        engine.release();
        second.join().unwrap().unwrap();
    }

    #[test]
    fn two_providers_of_one_group_do_not_call_the_model_together() {
        let settings = settings(&[("a", "Single"), ("b", "Single")]);
        let slots = Slots::new();
        slots.configure(&settings);
        let engine = Arc::new(Blocking::default());
        let gate = Arc::new(Gated::new(engine.clone(), slots));
        let first = chat(gate.clone(), "a");
        engine.wait_entered(1);
        let second = chat(gate, "b");
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(engine.entered(), 1, "the second provider is still waiting");
        engine.release();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert_eq!(engine.most_inside(), 1, "the group kept its one slot");
        assert_eq!(engine.entered(), 2);
    }

    #[test]
    fn providers_of_different_groups_call_the_model_together() {
        let settings = settings(&[("a", "Single"), ("c", "Local")]);
        let slots = Slots::new();
        slots.configure(&settings);
        let engine = Arc::new(Blocking::default());
        let gate = Arc::new(Gated::new(engine.clone(), slots));
        let first = chat(gate.clone(), "a");
        let second = chat(gate, "c");
        engine.wait_entered(2);
        assert_eq!(engine.most_inside(), 2);
        engine.release();
        assert!(first.join().unwrap().is_ok());
        assert!(second.join().unwrap().is_ok());
    }

    #[test]
    fn a_waiter_of_a_removed_group_reaches_the_model() {
        let settings = settings(&[("a", "Single"), ("b", "Single")]);
        let slots = Slots::new();
        slots.configure(&settings);
        let engine = Arc::new(Blocking::default());
        let gate = Arc::new(Gated::new(engine.clone(), slots.clone()));
        let first = chat(gate.clone(), "a");
        engine.wait_entered(1);
        let second = chat(gate, "b");
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(engine.entered(), 1);
        slots.configure(&AgentSettings {
            groups: vec![Group::new("Local", 1)],
            connections: settings.connections.clone(),
            ..AgentSettings::default()
        });
        engine.wait_entered(2);
        engine.release();
        assert!(first.join().unwrap().is_ok());
        assert!(second.join().unwrap().is_ok());
    }
}
