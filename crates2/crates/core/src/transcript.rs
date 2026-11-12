use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

fn join(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phrase {
    pub start: f64,
    pub end: f64,
    pub words: Vec<Word>,
}

impl Phrase {
    pub fn text(&self) -> String {
        join(&self.words)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub start: f64,
    pub end: f64,
    pub speaker: Option<u32>,
    pub words: Vec<Word>,
}

impl Reply {
    pub fn text(&self) -> String {
        join(&self.words)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NameSource {
    Manual,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameEvidence {
    pub start: f64,
    pub text: String,
    pub how: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerName {
    pub name: String,
    pub source: NameSource,
    pub evidence: Vec<NameEvidence>,
}

const REPLY_GAP: f64 = 1.5;
const REPLY_MAX: f64 = 60.0;

fn speaker_at(turns: &[Turn], start: f64, end: f64) -> Option<u32> {
    let overlap = |t: &Turn| (end.min(t.end) - start.max(t.start)).max(0.0);
    let distance = |t: &Turn| {
        if end < t.start {
            t.start - end
        } else if start > t.end {
            start - t.end
        } else {
            0.0
        }
    };
    turns
        .iter()
        .max_by(|a, b| {
            overlap(a)
                .total_cmp(&overlap(b))
                .then(distance(b).total_cmp(&distance(a)))
        })
        .map(|t| t.speaker)
}

pub fn replies(phrases: &[Phrase], turns: &[Turn]) -> Vec<Reply> {
    let mut out: Vec<Reply> = Vec::new();
    for phrase in phrases {
        let mut phrase_start = true;
        for w in &phrase.words {
            let speaker = if turns.is_empty() {
                None
            } else {
                speaker_at(turns, w.start, w.end)
            };
            let joins = out.last().is_some_and(|r| {
                r.speaker == speaker
                    && (!phrase_start
                        || (w.start - r.end <= REPLY_GAP && r.end - r.start < REPLY_MAX))
            });
            match out.last_mut() {
                Some(r) if joins => {
                    r.end = w.end;
                    r.words.push(w.clone());
                }
                _ => out.push(Reply {
                    start: w.start,
                    end: w.end,
                    speaker,
                    words: vec![w.clone()],
                }),
            }
            phrase_start = false;
        }
    }
    out
}
