use super::brain::{parse_step, AnswerStream, Brain, Call, Message, Piece, Role, Step, ToolSpec, MAX_CALLS};
use crate::providers::chat_completion_stream;
use crate::scope::Scope;
use crate::slots::Slots;
use mediaa_core::agent::{Provider, ProviderKind, WorkKind};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub struct ProviderBrain {
    pub provider: Provider,
    pub model: String,
    pub slots: Slots,
    pub max_tokens: u32,
    pub scope: Scope,
    pub thinking: bool,
}

pub fn request(model: &str, messages: &[Message], tools: &[ToolSpec], may_answer: bool, max_tokens: u32, reasoning: Option<ProviderKind>) -> Value {
    let messages: Vec<Value> = messages.iter().map(|m| json!({"role": m.role.as_str(), "content": m.content})).collect();
    let mut body = json!({"model": model, "messages": messages, "max_tokens": max_tokens, "temperature": 0.2});
    match reasoning {
        Some(ProviderKind::OpenRouter) => body["reasoning"] = json!({"effort": "medium"}),
        Some(_) => body["reasoning_effort"] = json!("medium"),
        None => {}
    }
    if !tools.is_empty() {
        let specs: Vec<Value> = tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.params}}))
            .collect();
        body["tools"] = Value::Array(specs);
        body["tool_choice"] = json!(if may_answer { "auto" } else { "required" });
        body["parallel_tool_calls"] = json!(true);
    }
    body
}

#[derive(Default)]
struct Collected {
    content: String,
    calls: std::collections::BTreeMap<u64, (String, String)>,
    json_text: Option<AnswerStream>,
    prompt_tokens: u64,
    completion_tokens: u64,
}

impl Collected {
    fn add(&mut self, chunk: &Value, on_piece: &mut dyn FnMut(Piece)) {
        if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
            self.prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
            self.completion_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);
        }
        let delta = &chunk["choices"][0]["delta"];
        if let Some(thought) = delta["reasoning_content"].as_str().or_else(|| delta["reasoning"].as_str()).filter(|t| !t.is_empty()) {
            on_piece(Piece::Thinking(thought));
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for c in calls {
                let call = self.calls.entry(c["index"].as_u64().unwrap_or(0)).or_default();
                call.0.push_str(c["function"]["name"].as_str().unwrap_or_default());
                call.1.push_str(c["function"]["arguments"].as_str().unwrap_or_default());
            }
        }
        let Some(piece) = delta["content"].as_str() else { return };
        self.content.push_str(piece);
        if !self.calls.is_empty() {
            return;
        }
        if self.json_text.is_none() && self.content.trim_start().starts_with('{') {
            let mut s = AnswerStream::default();
            let shown = s.push(&self.content);
            self.json_text = Some(s);
            if !shown.is_empty() {
                on_piece(Piece::Answer(&shown));
            }
            return;
        }
        let text = match &mut self.json_text {
            Some(s) => s.push(piece),
            None if self.content.trim().is_empty() => return,
            None => piece.to_string(),
        };
        if !text.is_empty() {
            on_piece(Piece::Answer(&text));
        }
    }

    fn response(&self) -> Value {
        let mut message = json!({"content": self.content});
        if !self.calls.is_empty() {
            let calls: Vec<Value> = self.calls.values().map(|(name, arguments)| json!({"function": {"name": name, "arguments": arguments}})).collect();
            message["tool_calls"] = Value::Array(calls);
        }
        json!({"choices": [{"message": message}]})
    }
}

pub fn read_response(body: &Value) -> anyhow::Result<Step> {
    let msg = &body["choices"][0]["message"];
    anyhow::ensure!(msg.is_object(), "no message in the answer: {body}");
    let content = msg["content"].as_str().unwrap_or_default().trim().to_string();
    if let Some(items) = msg["tool_calls"].as_array().filter(|c| !c.is_empty()) {
        let mut calls: Vec<Call> = Vec::new();
        for item in items {
            let tool = item["function"]["name"].as_str().unwrap_or_default().to_string();
            let raw = item["function"]["arguments"].as_str().unwrap_or("{}");
            let mut args: Value = serde_json::from_str(raw).map_err(|e| anyhow::anyhow!(crate::prompts::render("errors/arguments_not_json.j2", minijinja::context! { tool => &tool, error => e.to_string(), raw })))?;
            if let Some(obj) = args.as_object_mut() {
                obj.remove("title");
            }
            calls.push(Call { tool, args });
        }
        if calls.len() > MAX_CALLS {
            log::warn!("the model wrote {} calls in one step; only the first {MAX_CALLS} are kept", calls.len());
            calls.truncate(MAX_CALLS);
        }
        return Ok(Step::Tool { thought: content, calls });
    }
    if content.starts_with('{') {
        if let Ok(step) = parse_step(&content) {
            return Ok(step);
        }
    }
    anyhow::ensure!(!content.is_empty(), "the model returned nothing");
    Ok(Step::Answer { thought: String::new(), text: content })
}

impl ProviderBrain {
    fn reasoning(&self) -> Option<ProviderKind> {
        self.thinking.then_some(self.provider.kind)
    }

    fn count(&self, started: Instant, collected: &Collected) {
        self.scope.record_call(&self.model, started, collected.prompt_tokens, collected.completion_tokens);
    }
}

impl Brain for ProviderBrain {
    fn step(&self, messages: &[Message], tools: &[ToolSpec], may_answer: bool, cancel: &AtomicBool, on_piece: &mut dyn FnMut(Piece)) -> anyhow::Result<Step> {
        let mut messages = messages.to_vec();
        let mut retried = false;
        loop {
            let lease = self.slots.request(WorkKind::ProviderCalls, true).wait(cancel)?;
            let started = Instant::now();
            let mut collected = Collected::default();
            let body = request(&self.model, &messages, tools, may_answer, self.max_tokens, self.reasoning());
            let streamed = chat_completion_stream(&self.provider, &body, &mut |chunk| {
                collected.add(chunk, on_piece);
            });
            drop(lease);
            self.count(started, &collected);
            match streamed.and_then(|()| read_response(&collected.response())) {
                Ok(step) => return Ok(step),
                Err(e) if !retried && e.to_string().contains("not JSON") => {
                    retried = true;
                    messages.push(Message::new(Role::User, crate::prompts::render("nudges/malformed_call.j2", minijinja::context! { error => e.to_string() })));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn complete(&self, messages: &[Message], cancel: &AtomicBool) -> anyhow::Result<String> {
        let lease = self.slots.request(WorkKind::ProviderCalls, true).wait(cancel)?;
        let started = Instant::now();
        let mut collected = Collected::default();
        let streamed = chat_completion_stream(&self.provider, &request(&self.model, messages, &[], true, self.max_tokens, None), &mut |chunk| {
            collected.add(chunk, &mut |_| {});
        });
        drop(lease);
        self.count(started, &collected);
        streamed?;
        Ok(collected.content.trim().to_string())
    }

    fn context(&self) -> u32 {
        self.provider.context
    }

    fn thinking(&self) -> bool {
        self.thinking
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::Budget;
    use mediaa_queue::{Store, TaskId};
    use std::sync::{Arc, Mutex};

    fn spec() -> ToolSpec {
        ToolSpec { name: "search", description: "find".into(), params: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}), example: json!({}) }
    }

    #[test]
    fn requests_carry_tools_and_the_choice() {
        let body = request("m", &[Message::new(Role::User, "q")], &[spec()], false, 100, None);
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["parallel_tool_calls"], true, "several calls in one answer are asked for");
        let f = &body["tools"][0]["function"];
        assert_eq!(f["name"], "search");
        assert_eq!(f["parameters"]["required"], json!(["query"]));
        assert!(f["parameters"]["properties"].get("title").is_none());
        assert_eq!(body["messages"][0], json!({"role": "user", "content": "q"}));
        assert_eq!(request("m", &[], &[spec()], true, 100, None)["tool_choice"], "auto");
        assert!(request("m", &[], &[], true, 100, None).get("tools").is_none());
        assert!(request("m", &[], &[], true, 100, None).get("parallel_tool_calls").is_none(), "no tools: nothing to call");
    }

    #[test]
    fn responses_become_steps() {
        let call = json!({"choices": [{"message": {"content": null, "tool_calls": [{"function": {"name": "search", "arguments": "{\"query\":\"бюджет\",\"title\":\"Ищу бюджет\"}"}}]}}]});
        assert_eq!(read_response(&call).unwrap(), Step::Tool { thought: String::new(), calls: vec![Call { tool: "search".into(), args: json!({"query": "бюджет"}) }] });
        let text = json!({"choices": [{"message": {"content": "Ответ [1:00]."}}]});
        assert_eq!(read_response(&text).unwrap(), Step::Answer { thought: String::new(), text: "Ответ [1:00].".into() });
        let as_text = json!({"choices": [{"message": {"content": "{\"thought\":\"\",\"action\":\"read\",\"title\":\"Читаю\",\"args\":{\"from\":\"1:00\",\"to\":\"2:00\"}}"}}]});
        assert!(matches!(read_response(&as_text).unwrap(), Step::Tool { calls, .. } if calls[0].tool == "read"));
        let bad = json!({"choices": [{"message": {"tool_calls": [{"function": {"name": "search", "arguments": "{query:"}}]}}]});
        assert!(read_response(&bad).unwrap_err().to_string().contains("not JSON"));
        assert!(read_response(&json!({"error": "x"})).is_err());
        let two = json!({"choices": [{"message": {"tool_calls": [
            {"function": {"name": "search", "arguments": "{\"query\":\"бюджет\"}"}},
            {"function": {"name": "read", "arguments": "{\"from\":\"0:00\",\"to\":\"1:00\"}"}}
        ]}}]});
        let Step::Tool { calls, .. } = read_response(&two).unwrap() else { panic!("not a step") };
        assert_eq!(calls, [Call { tool: "search".into(), args: json!({"query": "бюджет"}) }, Call { tool: "read".into(), args: json!({"from": "0:00", "to": "1:00"}) }]);
        let many = json!({"choices": [{"message": {"tool_calls": (0..4).map(|i| json!({"function": {"name": format!("t{i}"), "arguments": "{}"}})).collect::<Vec<_>>()}}]});
        let Step::Tool { calls, .. } = read_response(&many).unwrap() else { panic!("not a step") };
        assert_eq!(calls.len(), MAX_CALLS);
    }

    fn sse_server(script: Vec<Vec<Value>>) -> (String, Arc<Mutex<Vec<Value>>>) {
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = srv.server_addr().to_ip().unwrap();
        let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
        let b = bodies.clone();
        std::thread::spawn(move || {
            for (n, mut req) in srv.incoming_requests().enumerate() {
                let mut s = String::new();
                req.as_reader().read_to_string(&mut s).unwrap();
                b.lock().unwrap().push(serde_json::from_str(&s).unwrap());
                let mut events: String = script[n].iter().map(|d| format!("data: {}

", if d.get("usage").is_some() { d.clone() } else { json!({"choices": [{"delta": d}]}) })).collect();
                events.push_str("data: [DONE]

");
                let _ = req.respond(tiny_http::Response::from_string(events));
            }
        });
        (format!("http://{addr}/v1"), bodies)
    }

    fn brain(url: String, dir: &std::path::Path) -> (ProviderBrain, Arc<dyn Store>, TaskId) {
        let mut p = Provider::new(ProviderKind::OpenAiCompatible, "Local");
        p.base_url = url;
        let store = crate::run::open_store(dir).unwrap();
        let task = store.begin_inline("model-step", "step", &json!({}), None, None).unwrap();
        let scope = Scope::attach(store.clone(), task, Arc::new(mediaa_engine::testing::FakeEngine::new()), None);
        (ProviderBrain { provider: p, model: "m".into(), slots: Slots::new(Budget::default()), max_tokens: 100, scope, thinking: false }, store, task)
    }

    fn call(name: &str, args: &str) -> Value {
        json!({"tool_calls": [{"index": 0, "function": {"name": name, "arguments": args}}]})
    }

    #[test]
    fn several_calls_stream_in_one_answer_in_their_order() {
        let (url, _) = sse_server(vec![vec![
            json!({"content": "Ищу."}),
            json!({"tool_calls": [{"index": 1, "function": {"name": "read", "arguments": "{\"from\":\"0:00\","}}]}),
            json!({"tool_calls": [{"index": 0, "function": {"name": "search", "arguments": "{\"query\":\"бюд"}}]}),
            json!({"tool_calls": [{"index": 1, "function": {"arguments": "\"to\":\"1:00\"}"}}]}),
            json!({"tool_calls": [{"index": 0, "function": {"arguments": "жет\"}"}}]}),
            json!({"content": " не ответ"}),
        ]]);
        let dir = tempfile::tempdir().unwrap();
        let (b, ..) = brain(url, dir.path());
        let mut shown = String::new();
        let step = b
            .step(&[Message::new(Role::User, "q")], &[spec()], false, &AtomicBool::new(false), &mut |p| {
                if let Piece::Answer(t) = p {
                    shown.push_str(t)
                }
            })
            .unwrap();
        assert_eq!(
            step,
            Step::Tool {
                thought: "Ищу. не ответ".into(),
                calls: vec![
                    Call { tool: "search".into(), args: json!({"query": "бюджет"}) },
                    Call { tool: "read".into(), args: json!({"from": "0:00", "to": "1:00"}) },
                ],
            },
            "the calls in the order of their indexes"
        );
        assert_eq!(shown, "Ищу.", "only the text before the calls is shown as the answer");
    }

    #[test]
    fn streams_answers_and_retries_a_malformed_call_once() {
        let (url, bodies) = sse_server(vec![
            vec![call("search", "{query:")],
            vec![call("search", r#"{"query":"бюд"#), call("", r#"жет","title":"Ищу"}"#)],
            vec![json!({"content": "Да, "}), json!({"content": "[1:00]."})],
            vec![json!({"content": r#"{"thought":"","action":"answer","#}), json!({"content": r#""answer":"Нет."}"#})],
        ]);
        let dir = tempfile::tempdir().unwrap();
        let (b, ..) = brain(url, dir.path());
        let mut shown = String::new();
        let mut step = |may_answer| {
            b.step(&[Message::new(Role::User, "q")], &[spec()], may_answer, &AtomicBool::new(false), &mut |p| {
                if let Piece::Answer(t) = p {
                    shown.push_str(t)
                }
            })
            .unwrap()
        };
        assert_eq!(step(false), Step::Tool { thought: String::new(), calls: vec![Call { tool: "search".into(), args: json!({"query": "бюджет"}) }] });
        assert_eq!(step(true), Step::Answer { thought: String::new(), text: "Да, [1:00].".into() });
        assert_eq!(step(true), Step::Answer { thought: String::new(), text: "Нет.".into() });
        assert_eq!(shown, "Да, [1:00].Нет.", "answers streamed, calls not");
        let sent = bodies.lock().unwrap();
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[0]["stream"], true);
        assert_eq!(sent[0]["stream_options"]["include_usage"], true, "the tokens used are asked for");
        assert!(sent[1]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("malformed"));
    }

    #[test]
    fn requests_are_counted_with_the_tokens_of_the_response() {
        let (url, _) = sse_server(vec![
            vec![json!({"content": "Да."}), json!({"usage": {"prompt_tokens": 120, "completion_tokens": 7}, "choices": []})],
            vec![json!({"content": "Сводка"})],
            vec![call("search", "{query:")],
        ]);
        let dir = tempfile::tempdir().unwrap();
        let (b, queue, task) = brain(url, dir.path());
        let cancel = AtomicBool::new(false);
        b.step(&[Message::new(Role::User, "q")], &[], true, &cancel, &mut |_| {}).unwrap();
        let stats = queue.get(task).unwrap().unwrap().stats;
        assert_eq!((stats.model_calls, stats.prompt_tokens, stats.completion_tokens), (1, 120, 7));
        assert_eq!(stats.models, ["m"]);
        assert_eq!(b.complete(&[Message::new(Role::User, "q")], &cancel).unwrap(), "Сводка");
        let stats = queue.get(task).unwrap().unwrap().stats;
        assert_eq!((stats.model_calls, stats.prompt_tokens, stats.completion_tokens), (2, 120, 7));
        assert!(b.step(&[Message::new(Role::User, "q")], &[spec()], false, &cancel, &mut |_| {}).is_err());
        assert_eq!(queue.get(task).unwrap().unwrap().stats.model_calls, 4, "the malformed call and its retry (the script is over)");
    }

    #[test]
    fn reasoning_is_asked_for_and_streamed() {
        let on = |kind| request("m", &[], &[], true, 100, Some(kind));
        assert_eq!(on(ProviderKind::OpenRouter)["reasoning"], json!({"effort": "medium"}));
        assert_eq!(on(ProviderKind::OpenAiCompatible)["reasoning_effort"], "medium");
        assert!(request("m", &[], &[], true, 100, None).get("reasoning_effort").is_none(), "off: nothing is sent");

        let (url, bodies) = sse_server(vec![vec![
            json!({"reasoning_content": "Надо "}),
            json!({"reasoning": "поискать."}),
            json!({"content": "Ответ"}),
            json!({"content": " готов."}),
        ]]);
        let dir = tempfile::tempdir().unwrap();
        let (mut b, ..) = brain(url, dir.path());
        b.thinking = true;
        let (mut thinking, mut answer) = (String::new(), String::new());
        let step = b
            .step(&[Message::new(Role::User, "q")], &[], true, &AtomicBool::new(false), &mut |p| match p {
                Piece::Thinking(t) => thinking.push_str(t),
                Piece::Answer(t) => answer.push_str(t),
            })
            .unwrap();
        assert_eq!(step, Step::Answer { thought: String::new(), text: "Ответ готов.".into() }, "the reasoning is not in the answer");
        assert_eq!((thinking.as_str(), answer.as_str()), ("Надо поискать.", "Ответ готов."));
        assert_eq!(bodies.lock().unwrap()[0]["reasoning_effort"], "medium");
    }
}
