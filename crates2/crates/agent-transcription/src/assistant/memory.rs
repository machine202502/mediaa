use super::brain::{Brain, Message, Role};
use super::Turn;
use crate::prompts;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;

const HISTORY_SHARE: f32 = 0.3;
const KEEP_TURNS: usize = 3;
const MAX_FACTS: usize = 30;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub facts: Vec<String>,
    pub summary: String,
    pub summarized: usize,
}

impl Memory {
    pub fn remember(&mut self, fact: &str) -> bool {
        let fact = fact.trim();
        if fact.is_empty()
            || self
                .facts
                .iter()
                .any(|f| f.to_lowercase() == fact.to_lowercase())
        {
            return false;
        }
        self.facts.push(fact.to_string());
        if self.facts.len() > MAX_FACTS {
            self.facts.remove(0);
        }
        true
    }
}

fn chars(turns: &[Turn]) -> usize {
    turns
        .iter()
        .map(|t| t.question.chars().count() + t.answer.chars().count())
        .sum()
}

pub fn to_fold(history: &[Turn], memory: &Memory, context: u32) -> usize {
    let budget = (context as f32 * HISTORY_SHARE) as usize * 3;
    let mut from = memory.summarized.min(history.len());
    let last = history.len().saturating_sub(KEEP_TURNS);
    while from < last && chars(&history[from..]) > budget {
        from += 1;
    }
    from - memory.summarized.min(history.len())
}

pub fn fold(
    brain: &dyn Brain,
    history: &[Turn],
    memory: &mut Memory,
    cancel: &AtomicBool,
) -> anyhow::Result<bool> {
    let n = to_fold(history, memory, brain.context());
    if n == 0 {
        return Ok(false);
    }
    let from = memory.summarized.min(history.len());
    let task = prompts::render(
        "memory_fold_task.j2",
        minijinja::context! { summary => &memory.summary, exchanges => &history[from..from + n] },
    );
    let messages = [
        Message::new(Role::System, prompts::render("memory_fold_system.j2", ())),
        Message::new(Role::User, task),
    ];
    memory.summary = brain.complete(&messages, cancel)?;
    memory.summarized = from + n;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(q: &str, a_len: usize) -> Turn {
        Turn {
            question: q.into(),
            answer: "а".repeat(a_len),
        }
    }

    #[test]
    fn facts_are_kept_once_and_bounded() {
        let mut m = Memory::default();
        assert!(m.remember(" Дану зовут Дарья "));
        assert!(!m.remember("дану зовут дарья"), "case does not make it new");
        assert!(!m.remember("  "));
        for i in 0..MAX_FACTS + 5 {
            m.remember(&format!("факт {i}"));
        }
        assert_eq!(m.facts.len(), MAX_FACTS);
        assert!(
            !m.facts.contains(&"Дану зовут Дарья".to_string()),
            "the oldest went"
        );
    }

    #[test]
    fn the_oldest_exchanges_fold_and_the_last_three_stay() {
        let history: Vec<Turn> = (0..8).map(|i| turn(&format!("вопрос {i}"), 1000)).collect();
        let m = Memory::default();
        let n = to_fold(&history, &m, 4000);
        assert_eq!(n, 5, "8 × ~1000 characters: 5 go, 3 stay (they fit)");
        assert_eq!(
            to_fold(&history[..3], &m, 4000),
            0,
            "short history: nothing"
        );
        let m = Memory {
            summarized: 5,
            ..Memory::default()
        };
        assert_eq!(to_fold(&history, &m, 4000), 0);
        let long: Vec<Turn> = (0..4).map(|i| turn(&format!("q{i}"), 5000)).collect();
        assert_eq!(to_fold(&long, &Memory::default(), 4000), 1);
    }
}
