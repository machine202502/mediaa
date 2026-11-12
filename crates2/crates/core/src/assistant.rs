use crate::chat::{Timeline, STEP_RUNNING};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRef {
    pub kind: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Found,
    Nothing,
    Repeated,
    Failed,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Found => "found",
            StepStatus::Nothing => "nothing",
            StepStatus::Repeated => "repeated",
            StepStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub title: String,
    pub tool: String,
    pub args: Value,
    pub result: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaitReason {
    Loading {
        model: String,
    },
    Memory {
        model: String,
    },
    Downloading {
        model: String,
        done: u64,
        total: u64,
    },
    Busy {
        tasks: Vec<TaskRef>,
    },
    ProviderSlot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Waiting { reason: WaitReason },
    StepStarted { title: String, tool: String },
    Step(StepRecord),
    Thinking(String),
    AnswerText(String),
    AnswerDiscarded,
    Say(String),
    SteerTaken { text: String },
    Scanning { done: usize, total: usize },
    Metrics(Metrics),
    Done,
    Stopped,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub context_tokens: u32,
    pub context: u32,
    pub speed: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Running,
    Done,
    Stopped,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Live {
    pub timeline: Timeline,
    pub waiting: Option<WaitReason>,
    pub scanning: Option<(usize, usize)>,
    pub metrics: Option<Metrics>,
    pub steps: Vec<StepRecord>,
    pub stored: usize,
    pub steers_taken: usize,
    pub state: State,
    pub continuing: bool,
    closed: bool,
}

impl Default for Live {
    fn default() -> Live {
        Live::new()
    }
}

impl Live {
    pub fn new() -> Live {
        Live {
            timeline: Timeline::default(),
            waiting: None,
            scanning: None,
            metrics: None,
            steps: Vec::new(),
            stored: 0,
            steers_taken: 0,
            state: State::Running,
            continuing: false,
            closed: false,
        }
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    pub fn apply(&mut self, event: &Event) {
        if !matches!(self.state, State::Running) {
            return;
        }
        if self.closed && !matches!(event, Event::Done | Event::Stopped | Event::Failed(_)) {
            return;
        }
        if !matches!(event, Event::Waiting { .. } | Event::Metrics(_)) {
            self.waiting = None;
        }
        match event {
            Event::Waiting { reason } => self.waiting = Some(reason.clone()),
            Event::StepStarted { title, tool } => {
                self.scanning = None;
                self.timeline.step(title, tool, STEP_RUNNING);
            }
            Event::Step(record) => {
                self.scanning = None;
                self.timeline.finish_step(record.status.as_str());
                self.steps.push(record.clone());
            }
            Event::Thinking(text) => self.timeline.thinking(text),
            Event::AnswerText(text) => self.timeline.answer(text),
            Event::AnswerDiscarded => self.timeline.discard_answer(),
            Event::Say(text) => {
                self.timeline.section(text);
                self.stored += 1;
            }
            Event::SteerTaken { text } => {
                self.timeline.steer(text);
                self.steers_taken += 1;
            }
            Event::Scanning { done, total } => self.scanning = Some((*done, *total)),
            Event::Metrics(metrics) => self.metrics = Some(*metrics),
            Event::Done => self.state = State::Done,
            Event::Stopped => {
                self.timeline.stop();
                self.state = State::Stopped;
            }
            Event::Failed(error) => {
                self.timeline.stop();
                self.state = State::Failed(error.clone());
            }
        }
    }

    pub fn details(&self, forced: bool, stopped: bool) -> Value {
        let mut details =
            json!({ "steps": self.steps, "forced": forced, "timeline": self.timeline.to_value() });
        if let Some(metrics) = self.metrics {
            details["metrics"] = json!(metrics);
        }
        if stopped {
            details["stopped"] = json!(true);
        }
        details
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_reduced_from_events_once() {
        let mut live = Live::new();
        live.apply(&Event::Thinking("plan".into()));
        live.apply(&Event::AnswerText("hello".into()));
        live.apply(&Event::Say("section".into()));
        live.apply(&Event::SteerTaken {
            text: "next".into(),
        });
        live.apply(&Event::Done);
        let details = live.details(false, false);
        assert_eq!(details["timeline"].as_array().map(Vec::len), Some(3));
        assert_eq!(details["steps"].as_array().map(Vec::len), Some(0));
        assert_eq!(live.state, State::Done);
    }
}
