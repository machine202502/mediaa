use mediaa_core::agent::{Connection, ConnectionKind, ModelSource};
use mediaa_engine::{
    ChatMessage, ChatRequest, Connections, EmbeddingsRequest, Engine, TranscribeRequest,
};
use std::sync::atomic::AtomicBool;

fn connection(id: &str, kind: ConnectionKind, url: &str, key: &str) -> Connection {
    let mut connection = Connection::new(kind, id);
    connection.id = id.to_string();
    connection.base_url = url.to_string();
    connection.api_key = key.to_string();
    connection
}

#[test]
#[ignore = "needs the docker model stack on ports 8080, 8081 and 8293"]
fn docker_chat_embed_and_speech_answer() {
    let chat = connection(
        "chat",
        ConnectionKind::OpenAiCompatible,
        "http://127.0.0.1:8080/v1",
        "",
    );
    let embed = connection(
        "embed",
        ConnectionKind::OpenAiCompatible,
        "http://127.0.0.1:8081/v1",
        "",
    );
    let speech = connection(
        "speech",
        ConnectionKind::Transcription,
        "http://127.0.0.1:8293",
        "mediaa",
    );
    let engine = Connections::new(vec![chat, embed, speech]);
    let cancel = AtomicBool::new(false);
    let answer = engine
        .chat(
            &ChatRequest {
                source: ModelSource::new("chat", "qwen3-4b"),
                messages: vec![ChatMessage::user("Ответь одним словом: да")],
                max_tokens: 128,
                temperature: 0.0,
                ..ChatRequest::default()
            },
            &cancel,
            &mut |_| true,
        )
        .expect("chat");
    let said = if answer.content.trim().is_empty() {
        answer.reasoning.trim().to_string()
    } else {
        answer.content.trim().to_string()
    };
    assert!(!said.is_empty(), "blank chat: {answer:?}");
    eprintln!("chat: {said}");

    let vectors = engine
        .embeddings(
            &EmbeddingsRequest {
                source: ModelSource::new("embed", "qwen3-embedding"),
                input: vec!["проверка поиска".into()],
                ..EmbeddingsRequest::default()
            },
            &cancel,
        )
        .expect("embeddings");
    assert_eq!(vectors.len(), 1, "{vectors:?}");
    assert!(vectors[0].len() > 8, "short embedding {}", vectors[0].len());
    eprintln!("embedding dims: {}", vectors[0].len());

    let wav =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/live-speech.wav");
    let speech = engine
        .transcribe(
            &TranscribeRequest {
                source: ModelSource::new("speech", "gigaam"),
                path: wav,
                from: 0.0,
                to: 44.0,
                language: Some("ru".into()),
                word_times: true,
                speakers: true,
                voiceprint: true,
            },
            &cancel,
        )
        .expect("speech");
    let text = speech
        .phrases
        .iter()
        .flat_map(|phrase| phrase.words.iter().map(|word| word.text.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "speech phrases={} turns={} voices={} text={text}",
        speech.phrases.len(),
        speech.turns.len(),
        speech.voices.len()
    );
    for word in speech
        .phrases
        .iter()
        .flat_map(|phrase| phrase.words.iter())
        .take(12)
    {
        eprintln!("  {:.2}-{:.2} {}", word.start, word.end, word.text);
    }
    let folded = text.to_lowercase();
    assert!(
        folded.contains("ирин") || folded.contains("чайник") || folded.contains("тетрад"),
        "speech was not recognized: {text}"
    );
    assert!(!speech.turns.is_empty(), "no speakers");
    assert!(
        speech.voices.iter().any(|voice| voice.len() > 8),
        "no voiceprint"
    );
}
