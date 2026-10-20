//! A provider's model as the agent's brain: native OpenAI tool calls (`tools`, `tool_choice`),
//! streamed (the answer shows as it is written), arguments checked as JSON with one corrective
//! retry; a reply that is a step written as text (models imitate the history) is accepted too.
//! Each request takes a «provider calls» slot of the budget.

use super::brain::{parse_step, AnswerStream, Brain, Message, Role, Step, ToolSpec};
use crate::providers::chat_completion_stream;
use crate::slots::Slots;
use mediaa_core::agent::{Provider, WorkKind};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

pub struct ProviderBrain {
    pub provider: Provider,
    pub model: String,
    pub slots: Slots,
    /// Longest step (tokens).
    pub max_tokens: u32,
}

/// The request body for a step.
pub fn request(model: &str, messages: &[Message], tools: &[ToolSpec], may_answer: bool, max_tokens: u32) -> Value {
    let messages: Vec<Value> = messages.iter().map(|m| json!({"role": m.role.as_str(), "content": m.content})).collect();
    let mut body = json!({"model": model, "messages": messages, "max_tokens": max_tokens, "temperature": 0.2});
    if !tools.is_empty() {
        let specs: Vec<Value> = tools
            .iter()
            .map(|t| {
                // The step title rides along as an argument.
                let mut params = t.params.clone();
                params["properties"]["title"] = json!({"type": "string", "description": crate::prompts::tool("step_title")});
                let mut required = params["required"].as_array().cloned().unwrap_or_default();
                required.push(json!("title"));
                params["required"] = Value::Array(required);
                json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": params}})
            })
            .collect();
        body["tools"] = Value::Array(specs);
        body["tool_choice"] = json!(if may_answer { "auto" } else { "required" });
    }
    body
}

/// A streamed response put together: the text, the first tool call; the text is passed on as
/// the answer while no call has begun (a step written as JSON text streams its `answer` only).
#[derive(Default)]
struct Collected {
    content: String,
    tool: String,
    arguments: String,
    json_text: Option<AnswerStream>,
}

impl Collected {
    fn add(&mut self, delta: &Value, on_answer: &mut dyn FnMut(&str)) {
        if let Some(calls) = delta["tool_calls"].as_array() {
            for c in calls.iter().filter(|c| c["index"].as_u64().unwrap_or(0) == 0) {
                self.tool.push_str(c["function"]["name"].as_str().unwrap_or_default());
                self.arguments.push_str(c["function"]["arguments"].as_str().unwrap_or_default());
            }
        }
        let Some(piece) = delta["content"].as_str() else { return };
        self.content.push_str(piece);
        if !self.tool.is_empty() {
            return;
        }
        if self.json_text.is_none() && self.content.trim_start().starts_with('{') {
            let mut s = AnswerStream::default();
            let shown = s.push(&self.content);
            self.json_text = Some(s);
            if !shown.is_empty() {
                on_answer(&shown);
            }
            return;
        }
        let text = match &mut self.json_text {
            Some(s) => s.push(piece),
            None if self.content.trim().is_empty() => return,
            None => piece.to_string(),
        };
        if !text.is_empty() {
            on_answer(&text);
        }
    }

    /// The response as a non-streamed one.
    fn response(&self) -> Value {
        let mut message = json!({"content": self.content});
        if !self.tool.is_empty() {
            message["tool_calls"] = json!([{"function": {"name": self.tool, "arguments": self.arguments}}]);
        }
        json!({"choices": [{"message": message}]})
    }
}

/// The step in a chat completion response.
pub fn read_response(body: &Value) -> anyhow::Result<Step> {
    let msg = &body["choices"][0]["message"];
    anyhow::ensure!(msg.is_object(), "no message in the answer: {body}");
    let content = msg["content"].as_str().unwrap_or_default().trim().to_string();
    if let Some(call) = msg["tool_calls"].as_array().and_then(|c| c.first()) {
        let tool = call["function"]["name"].as_str().unwrap_or_default().to_string();
        let raw = call["function"]["arguments"].as_str().unwrap_or("{}");
        let mut args: Value = serde_json::from_str(raw).map_err(|e| anyhow::anyhow!(crate::prompts::render("errors/arguments_not_json.j2", minijinja::context! { tool => &tool, error => e.to_string(), raw })))?;
        let title = args.as_object_mut().and_then(|o| o.remove("title")).and_then(|t| t.as_str().map(str::to_string)).unwrap_or_default();
        return Ok(Step::Tool { thought: content, title, tool, args });
    }
    // A step written as text (the history shows steps as JSON), else a plain answer.
    if content.starts_with('{') {
        if let Ok(step) = parse_step(&content) {
            return Ok(step);
        }
    }
    anyhow::ensure!(!content.is_empty(), "the model returned nothing");
    Ok(Step::Answer { thought: String::new(), text: content })
}

impl Brain for ProviderBrain {
    fn step(&self, messages: &[Message], tools: &[ToolSpec], may_answer: bool, cancel: &AtomicBool, on_answer: &mut dyn FnMut(&str)) -> anyhow::Result<Step> {
        let mut messages = messages.to_vec();
        let mut retried = false;
        loop {
            let lease = self.slots.request(WorkKind::ProviderCalls, true).wait(cancel)?;
            let mut collected = Collected::default();
            let streamed = chat_completion_stream(&self.provider, &request(&self.model, &messages, tools, may_answer, self.max_tokens), &mut |delta| {
                collected.add(delta, on_answer);
            });
            drop(lease);
            match streamed.and_then(|()| read_response(&collected.response())) {
                Ok(step) => return Ok(step),
                // A malformed call: once more, telling the model what was wrong.
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
        let mut text = String::new();
        chat_completion_stream(&self.provider, &request(&self.model, messages, &[], true, self.max_tokens), &mut |delta| {
            text.push_str(delta["content"].as_str().unwrap_or_default());
        })?;
        drop(lease);
        Ok(text.trim().to_string())
    }

    fn context(&self) -> u32 {
        self.provider.context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{Budget, ProviderKind};
    use std::sync::{Arc, Mutex};

    fn spec() -> ToolSpec {
        ToolSpec { name: "search", description: "find".into(), params: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}), example: json!({}) }
    }

    #[test]
    fn requests_carry_tools_and_the_choice() {
        let body = request("m", &[Message::new(Role::User, "q")], &[spec()], false, 100);
        assert_eq!(body["tool_choice"], "required");
        let f = &body["tools"][0]["function"];
        assert_eq!(f["name"], "search");
        assert_eq!(f["parameters"]["required"], json!(["query", "title"]));
        assert_eq!(body["messages"][0], json!({"role": "user", "content": "q"}));
        assert_eq!(request("m", &[], &[spec()], true, 100)["tool_choice"], "auto");
        assert!(request("m", &[], &[], true, 100).get("tools").is_none());
    }

    #[test]
    fn responses_become_steps() {
        let call = json!({"choices": [{"message": {"content": null, "tool_calls": [{"function": {"name": "search", "arguments": "{\"query\":\"бюджет\",\"title\":\"Ищу бюджет\"}"}}]}}]});
        assert_eq!(read_response(&call).unwrap(), Step::Tool { thought: String::new(), title: "Ищу бюджет".into(), tool: "search".into(), args: json!({"query": "бюджет"}) });
        let text = json!({"choices": [{"message": {"content": "Ответ [1:00]."}}]});
        assert_eq!(read_response(&text).unwrap(), Step::Answer { thought: String::new(), text: "Ответ [1:00].".into() });
        let as_text = json!({"choices": [{"message": {"content": "{\"thought\":\"\",\"action\":\"read\",\"title\":\"Читаю\",\"args\":{\"from\":\"1:00\",\"to\":\"2:00\"}}"}}]});
        assert!(matches!(read_response(&as_text).unwrap(), Step::Tool { tool, .. } if tool == "read"));
        let bad = json!({"choices": [{"message": {"tool_calls": [{"function": {"name": "search", "arguments": "{query:"}}]}}]});
        assert!(read_response(&bad).unwrap_err().to_string().contains("not JSON"));
        assert!(read_response(&json!({"error": "x"})).is_err());
    }

    /// An SSE server answering request n with the events of `script[n]` (chunk deltas).
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

", json!({"choices": [{"delta": d}]}))).collect();
                events.push_str("data: [DONE]

");
                let _ = req.respond(tiny_http::Response::from_string(events));
            }
        });
        (format!("http://{addr}/v1"), bodies)
    }

    fn brain(url: String) -> ProviderBrain {
        let mut p = Provider::new(ProviderKind::OpenAiCompatible, "Local");
        p.base_url = url;
        ProviderBrain { provider: p, model: "m".into(), slots: Slots::new(Budget::default()), max_tokens: 100 }
    }

    fn call(name: &str, args: &str) -> Value {
        json!({"tool_calls": [{"index": 0, "function": {"name": name, "arguments": args}}]})
    }

    #[test]
    fn streams_answers_and_retries_a_malformed_call_once() {
        let (url, bodies) = sse_server(vec![
            // A malformed call, then a good one in two pieces.
            vec![call("search", "{query:")],
            vec![call("search", r#"{"query":"бюд"#), call("", r#"жет","title":"Ищу"}"#)],
            // A plain answer in pieces.
            vec![json!({"content": "Да, "}), json!({"content": "[1:00]."})],
            // A step written as JSON text: only its answer streams.
            vec![json!({"content": r#"{"thought":"","action":"answer","#}), json!({"content": r#""answer":"Нет."}"#})],
        ]);
        let b = brain(url);
        let mut shown = String::new();
        let mut step = |may_answer| b.step(&[Message::new(Role::User, "q")], &[spec()], may_answer, &AtomicBool::new(false), &mut |t: &str| shown.push_str(t)).unwrap();
        assert_eq!(step(false), Step::Tool { thought: String::new(), title: "Ищу".into(), tool: "search".into(), args: json!({"query": "бюджет"}) });
        assert_eq!(step(true), Step::Answer { thought: String::new(), text: "Да, [1:00].".into() });
        assert_eq!(step(true), Step::Answer { thought: String::new(), text: "Нет.".into() });
        assert_eq!(shown, "Да, [1:00].Нет.", "answers streamed, calls not");
        let sent = bodies.lock().unwrap();
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[0]["stream"], true);
        assert!(sent[1]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("malformed"));
    }
}
