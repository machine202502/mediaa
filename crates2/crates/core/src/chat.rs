use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub type ChatId = i64;

pub const OPEN_CHATS: &str = "chat-tabs";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub fn memory_key(chat: ChatId) -> String {
    format!("chat-memory:{chat}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatInfo {
    pub id: ChatId,
    pub title: String,
    pub created_ms: i64,
    pub messages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub author: Author,
    pub text: String,
    pub details: serde_json::Value,
}

impl ChatMessage {
    pub fn timeline(&self) -> Timeline {
        Timeline::from_details(&self.details)
    }

    pub fn stopped(&self) -> bool {
        self.details["stopped"].as_bool() == Some(true)
    }
}

pub const STEP_RUNNING: &str = "running";
pub const STEP_STOPPED: &str = "stopped";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineEntry {
    Step {
        title: String,
        tool: String,
        status: String,
    },
    Thinking {
        text: String,
    },
    Answer {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        section: bool,
    },
    Steer {
        text: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub entries: Vec<TimelineEntry>,
}

impl Timeline {
    pub fn from_details(details: &serde_json::Value) -> Timeline {
        let entries = serde_json::from_value(details["timeline"].clone()).unwrap_or_default();
        Timeline { entries }
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(&self.entries).unwrap_or(serde_json::Value::Null)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn step(&mut self, title: &str, tool: &str, status: &str) {
        self.entries.push(TimelineEntry::Step {
            title: title.to_string(),
            tool: tool.to_string(),
            status: status.to_string(),
        });
    }

    pub fn set_step_title(&mut self, index: usize, title: &str) -> bool {
        let Some(slot) = self
            .entries
            .iter_mut()
            .filter_map(|entry| match entry {
                TimelineEntry::Step { title, .. } => Some(title),
                _ => None,
            })
            .nth(index)
        else {
            return false;
        };
        *slot = title.to_string();
        true
    }

    pub fn finish_step(&mut self, status: &str) {
        let running = self.entries.iter().position(
            |e| matches!(e, TimelineEntry::Step { status, .. } if status == STEP_RUNNING),
        );
        let last = self
            .entries
            .iter()
            .rposition(|e| matches!(e, TimelineEntry::Step { .. }));
        let Some(i) = running.or(last) else { return };
        if let TimelineEntry::Step { status: s, .. } = &mut self.entries[i] {
            *s = status.to_string();
        }
    }

    pub fn thinking(&mut self, piece: &str) {
        match self.entries.last_mut() {
            Some(TimelineEntry::Thinking { text }) => text.push_str(piece),
            _ => self.entries.push(TimelineEntry::Thinking {
                text: piece.to_string(),
            }),
        }
    }

    pub fn answer(&mut self, piece: &str) {
        match self.entries.last_mut() {
            Some(TimelineEntry::Answer {
                text,
                section: false,
            }) => text.push_str(piece),
            _ => self.entries.push(TimelineEntry::Answer {
                text: piece.to_string(),
                section: false,
            }),
        }
    }

    pub fn section(&mut self, text: &str) {
        self.discard_answer();
        self.entries.push(TimelineEntry::Answer {
            text: text.to_string(),
            section: true,
        });
    }

    pub fn steer(&mut self, text: &str) {
        self.entries.push(TimelineEntry::Steer {
            text: text.to_string(),
        });
    }

    pub fn discard_answer(&mut self) {
        if matches!(
            self.entries.last(),
            Some(TimelineEntry::Answer { section: false, .. })
        ) {
            self.entries.pop();
        }
    }

    pub fn finish_answer(&mut self, text: &str) {
        if text.trim().is_empty() {
            self.discard_answer();
            return;
        }
        match self.entries.last_mut() {
            Some(TimelineEntry::Answer {
                text: t,
                section: false,
            }) => *t = text.to_string(),
            _ => self.entries.push(TimelineEntry::Answer {
                text: text.to_string(),
                section: false,
            }),
        }
    }

    pub fn stop(&mut self) {
        for e in &mut self.entries {
            if let TimelineEntry::Step { status, .. } = e {
                if status == STEP_RUNNING {
                    *status = STEP_STOPPED.to_string();
                }
            }
        }
    }

    pub fn draft(&self) -> &str {
        match self.entries.last() {
            Some(TimelineEntry::Answer {
                text,
                section: false,
            }) => text,
            _ => "",
        }
    }
}
