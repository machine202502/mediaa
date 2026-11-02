use eframe::egui;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, ViewportId};
use std::collections::VecDeque;
use std::path::PathBuf;

const OVERLAY_VIEWPORT: &str = "mediaa-overlay";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Root,
    Overlay,
}

impl Target {
    fn of(viewport: ViewportId) -> Self {
        let overlay = ViewportId::from_hash_of(OVERLAY_VIEWPORT);
        if viewport == overlay {
            Self::Overlay
        } else {
            Self::Root
        }
    }
}

struct Command {
    target: Target,
    events: Vec<Event>,
}

#[derive(serde::Deserialize)]
struct Line {
    viewport: Option<String>,
    pointer: Option<[f32; 2]>,
    button: Option<String>,
    pressed: Option<bool>,
    key: Option<String>,
    text: Option<String>,
}

pub(crate) struct DevInput {
    path: PathBuf,
    lines_seen: usize,
    pending: VecDeque<Command>,
}

impl DevInput {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            lines_seen: 0,
            pending: VecDeque::new(),
        }
    }

    pub(crate) fn append(&mut self, raw: &mut RawInput) {
        self.read();
        raw.focused = true;
        raw.events
            .retain(|event| !matches!(event, Event::WindowFocused(false)));
        let current = Target::of(raw.viewport_id);
        let mut remaining = VecDeque::new();
        while let Some(command) = self.pending.pop_front() {
            if command.target == current {
                raw.focused = true;
                raw.events.extend(command.events);
            } else {
                remaining.push_back(command);
            }
        }
        self.pending = remaining;
    }

    fn read(&mut self) {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return;
        };
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        if lines.len() < self.lines_seen {
            self.lines_seen = 0;
        }
        for line in lines.iter().skip(self.lines_seen) {
            if let Some(command) = parse(line) {
                self.pending.push_back(command);
            }
        }
        self.lines_seen = lines.len();
    }
}

fn parse(line: &str) -> Option<Command> {
    let parsed: Line = serde_json::from_str(line).ok()?;
    let target = match parsed.viewport.as_deref() {
        Some("root") => Target::Root,
        _ => Target::Overlay,
    };
    let mut events = Vec::new();
    let position = parsed.pointer.map(|[x, y]| Pos2::new(x, y));
    if let Some(pos) = position {
        events.push(Event::PointerMoved(pos));
    }
    if let (Some(pos), Some(button), Some(pressed)) =
        (position, parsed.button.as_deref(), parsed.pressed)
    {
        events.push(Event::PointerButton {
            pos,
            button: pointer_button(button),
            pressed,
            modifiers: Modifiers::default(),
        });
    }
    if let Some(name) = parsed.key.as_deref() {
        if let Some(key) = key(name) {
            events.push(Event::Key {
                key,
                physical_key: None,
                pressed: parsed.pressed.unwrap_or(true),
                repeat: false,
                modifiers: Modifiers::default(),
            });
        }
    }
    if let Some(text) = parsed.text {
        events.push(Event::Text(text));
    }
    if events.is_empty() {
        return None;
    }
    Some(Command { target, events })
}

fn pointer_button(name: &str) -> PointerButton {
    match name {
        "right" => PointerButton::Secondary,
        "middle" => PointerButton::Middle,
        _ => PointerButton::Primary,
    }
}

fn key(name: &str) -> Option<Key> {
    Key::from_name(name)
}

pub(crate) fn configured() -> Option<PathBuf> {
    crate::dev_mode::mode()
        .input
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
}
