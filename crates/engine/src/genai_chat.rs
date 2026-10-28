use crate::engine::{
    apply_embed_prefix, ChatAnswer, ChatRequest, ChatRole, EmbeddingsRequest, EngineError,
    ToolChoice, Usage,
};
use genai::adapter::AdapterKind;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest as GenChat, ChatResponseFormat, ChatStreamEvent,
    JsonSpec, Tool,
};
use genai::embed::EmbedRequest;
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget, WebConfig};
use mediaa_core::agent::{Connection, ConnectionKind, ReasoningDepth};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("tokio runtime")
    })
}

fn adapter(kind: ConnectionKind) -> AdapterKind {
    match kind {
        ConnectionKind::OpenRouter => AdapterKind::OpenRouter,
        ConnectionKind::DeepSeek => AdapterKind::DeepSeek,
        ConnectionKind::Ollama => AdapterKind::Ollama,
        ConnectionKind::HuggingFace
        | ConnectionKind::OpenAiCompatible
        | ConnectionKind::Transcription => AdapterKind::OpenAI,
    }
}

fn target(connection: &Connection, model: &str) -> ServiceTarget {
    let url = if connection.kind == ConnectionKind::Ollama {
        connection.root_url()
    } else {
        connection.api_url()
    };
    let key = connection.api_key.trim();
    ServiceTarget {
        endpoint: Endpoint::from_owned(url),
        auth: AuthData::from_single(if key.is_empty() { "local" } else { key }),
        model: ModelIden::new(adapter(connection.kind), model),
    }
}

fn client(connection: &Connection) -> Result<Client, EngineError> {
    let mut web = WebConfig::default();
    if let Some(proxy) = connection.proxy() {
        web = web
            .with_proxy_url(proxy)
            .map_err(|error| EngineError::bad_request(format!("proxy \"{proxy}\": {error}")))?;
    }
    Client::builder()
        .with_web_config(web)
        .build()
        .map_err(|error| EngineError::internal(format!("{error}")))
}

fn depth(depth: ReasoningDepth) -> genai::chat::ReasoningEffort {
    match depth {
        ReasoningDepth::Low => genai::chat::ReasoningEffort::Low,
        ReasoningDepth::Medium => genai::chat::ReasoningEffort::Medium,
        ReasoningDepth::High => genai::chat::ReasoningEffort::High,
        ReasoningDepth::XHigh => genai::chat::ReasoningEffort::XHigh,
    }
}

fn usage_of(usage: &genai::chat::Usage) -> Usage {
    Usage {
        prompt_tokens: usage.prompt_tokens.unwrap_or(0).max(0) as u32,
        completion_tokens: usage.completion_tokens.unwrap_or(0).max(0) as u32,
    }
}

pub fn chat(
    connection: &Connection,
    request: &ChatRequest,
    cancel: &AtomicBool,
    on_text: &mut dyn FnMut(&str) -> bool,
) -> Result<ChatAnswer, EngineError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(EngineError::cancelled("cancelled"));
    }
    let client = client(connection)?;
    let target = target(connection, &request.source.model);
    let mut messages = Vec::new();
    let mut system = None;
    for message in &request.messages {
        match message.role {
            ChatRole::System if system.is_none() => system = Some(message.content.clone()),
            ChatRole::System => messages.push(ChatMessage::system(&message.content)),
            ChatRole::User => messages.push(ChatMessage::user(&message.content)),
            ChatRole::Assistant => messages.push(ChatMessage::assistant(&message.content)),
        }
    }
    let mut gen = GenChat::new(messages);
    if let Some(system) = system {
        gen = gen.with_system(system);
    }
    if !request.tools.is_empty() && request.tool_choice != ToolChoice::None {
        let tools = request.tools.iter().map(|tool| {
            Tool::new(tool.name.clone())
                .with_description(tool.description.clone())
                .with_schema(tool.parameters.clone())
        });
        gen = gen.with_tools(tools);
    }
    let mut options = ChatOptions::default()
        .with_temperature(f64::from(request.temperature))
        .with_max_tokens(request.max_tokens)
        .with_capture_usage(true)
        .with_capture_reasoning_content(true);
    if request.thinking.is_some() {
        options = options.with_reasoning_effort(depth(request.source.thinking_depth));
    }
    if let Some(schema) = &request.json_schema {
        options = options.with_response_format(ChatResponseFormat::JsonSpec(JsonSpec::new(
            "answer",
            schema.clone(),
        )));
    }
    options = options.with_tool_choice(match request.tool_choice {
        ToolChoice::Required => genai::chat::ToolChoice::Required,
        ToolChoice::None => genai::chat::ToolChoice::None,
        ToolChoice::Auto => genai::chat::ToolChoice::Auto,
    });
    runtime().block_on(async move {
        let stream = client
            .exec_chat_stream(target, gen, Some(&options))
            .await
            .map_err(|error| EngineError::bad_request(format!("{error}")))?;
        let mut answer = ChatAnswer::default();
        let mut stream = stream.stream;
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            mediaa_core::pulse::beat();
            if cancel.load(Ordering::Relaxed) {
                return Err(EngineError::cancelled("cancelled"));
            }
            match event.map_err(|error| EngineError::bad_request(format!("{error}")))? {
                ChatStreamEvent::Chunk(chunk) => {
                    if !on_text(&chunk.content) {
                        break;
                    }
                    answer.content.push_str(&chunk.content);
                }
                ChatStreamEvent::ReasoningChunk(chunk) => {
                    answer.reasoning.push_str(&chunk.content);
                }
                ChatStreamEvent::End(end) => {
                    if answer.content.is_empty() {
                        if let Some(text) = end.captured_first_text() {
                            answer.content = text.to_string();
                        }
                    }
                    if answer.reasoning.is_empty() {
                        if let Some(text) = end.captured_reasoning_content.as_deref() {
                            answer.reasoning = text.to_string();
                        }
                    }
                    if let Some(calls) = end.captured_tool_calls() {
                        answer.tool_calls = calls
                            .into_iter()
                            .map(|call| crate::engine::ToolCall {
                                name: call.fn_name.clone(),
                                arguments: call.fn_arguments.to_string(),
                            })
                            .collect();
                    }
                    if let Some(usage) = end.captured_usage.as_ref() {
                        answer.usage = Some(usage_of(usage));
                    }
                }
                _ => {}
            }
        }
        Ok(answer)
    })
}

pub fn embeddings(
    connection: &Connection,
    request: &EmbeddingsRequest,
) -> Result<Vec<Vec<f32>>, EngineError> {
    let texts = apply_embed_prefix(&request.source.model, request.kind, &request.input);
    let client = client(connection)?;
    let target = target(connection, &request.source.model);
    runtime().block_on(async move {
        let response = client
            .exec_embed(target, EmbedRequest::from_texts(texts), None)
            .await
            .map_err(|error| EngineError::bad_request(format!("{error}")))?;
        Ok(response
            .vectors()
            .into_iter()
            .map(|vector| vector.clone())
            .collect())
    })
}
