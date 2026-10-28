use mediaa_core::agent::{AgentSettings, ModelRole};
use mediaa_engine::Engine;

#[derive(Debug, Clone, PartialEq)]
pub enum Ready {
    Checked,
    NotUsed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Checking,
    Ready(Ready),
    Failed(String),
}

fn check_role(agent: &AgentSettings, engine: &dyn Engine, role: ModelRole) -> Status {
    let Some(source) = agent.models.get(role) else {
        return if role.required() {
            Status::Failed("no model selected".into())
        } else {
            Status::Ready(Ready::NotUsed)
        };
    };
    if agent.connection(&source.connection).is_none() {
        return Status::Failed("the connection was removed".into());
    }
    match engine.check(source, role) {
        Ok(check) if check.ok => Status::Ready(Ready::Checked),
        Ok(check) => Status::Failed(if check.detail.is_empty() {
            "the model did not pass the check".into()
        } else {
            check.detail
        }),
        Err(error) => Status::Failed(format!("{error:#}")),
    }
}

pub fn check_all(
    agent: &AgentSettings,
    engine: &dyn Engine,
    mut report: impl FnMut(ModelRole, Status),
) -> bool {
    let mut ok = true;
    for role in ModelRole::ALL {
        report(role, Status::Checking);
        let status = check_role(agent, engine, role);
        ok &= matches!(status, Status::Ready(_));
        report(role, status);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{Connection, ConnectionKind, ModelChoices, ModelSource};
    use mediaa_engine::testing::FakeEngine;
    use std::sync::{Arc, Mutex};

    #[test]
    fn every_role_is_checked_through_the_engine() {
        let connection = Connection::new(ConnectionKind::OpenAiCompatible, "Custom");
        let source = ModelSource::new(connection.id.clone(), "gpt");
        let agent = AgentSettings {
            connections: vec![connection],
            models: ModelChoices {
                embedding: Some(source.clone()),
                reranker: None,
                agent: Some(ModelSource::new("gone", "x")),
                titles: Some(source.clone()),
                speech_recognition: None,
            },
            ..AgentSettings::default()
        };
        let engine = FakeEngine::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let report = seen.clone();
        let ok = check_all(&agent, &engine, move |role, status| {
            report.lock().unwrap().push((role, status));
        });
        assert!(!ok);
        let last = |role| {
            seen.lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(item, _)| *item == role)
                .map(|(_, status)| status.clone())
                .unwrap()
        };
        assert_eq!(last(ModelRole::Embedding), Status::Ready(Ready::Checked));
        assert!(matches!(
            last(ModelRole::Agent),
            Status::Failed(text) if text.contains("removed")
        ));
        assert_eq!(last(ModelRole::Reranker), Status::Ready(Ready::NotUsed));
        assert!(matches!(
            last(ModelRole::SpeechRecognition),
            Status::Failed(text) if text.contains("no model")
        ));
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|(_, status)| *status == Status::Checking)
                .count(),
            ModelRole::ALL.len()
        );
        assert!(engine
            .checks()
            .iter()
            .any(|(key, role)| key.ends_with("/gpt") && role.contains("Embedding")));
    }
}
