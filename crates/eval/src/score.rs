//! Mechanical checks, and the shape of the judge's answer. No model is called here.

use crate::cases::{Fact, TurnSpec};
use mediaa_agent::assistant::{bad_citations, citations, Recording};
use serde_json::{json, Value};

/// How far outside an expected interval a cited time may sit and still count (seconds).
/// The same window the agent uses when it checks that a citation points at a real reply.
pub const SLOP_SECONDS: f64 = 15.0;

#[derive(Debug, Clone, PartialEq)]
pub struct JudgeVerdict {
    pub score: u32,
    pub reason: String,
}

/// What one answer scored. `steps`, `forced` and `seconds` are filled by the runner.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnScore {
    pub question: String,
    pub facts_hit: usize,
    pub facts_total: usize,
    pub missed: Vec<String>,
    pub forbidden: Vec<String>,
    pub cited: Vec<String>,
    pub precise: usize,
    pub bad_citations: Vec<String>,
    pub wanted_interval: bool,
    pub steps: usize,
    pub forced: bool,
    pub seconds: f64,
    pub judge: Option<JudgeVerdict>,
    pub judge_error: Option<String>,
}

impl TurnScore {
    /// Share of cited times that point at a real reply and sit inside an expected interval.
    /// `None` when the turn was given no interval: it is not counted. No citations where an
    /// interval was expected scores 0.
    pub fn precision(&self) -> Option<f64> {
        if !self.wanted_interval {
            None
        } else if self.cited.is_empty() {
            Some(0.0)
        } else {
            Some(self.precise as f64 / self.cited.len() as f64)
        }
    }
}

/// One repeat of a case, summed across its turns. Citation precision is the mean of the turns.
#[derive(Debug, Clone, PartialEq)]
pub struct RepeatScores {
    pub facts_hit: usize,
    pub facts_total: usize,
    pub forbidden_hits: usize,
    /// Mean of the turns that were given an interval. `None` when none of them were.
    pub citation_precision: Option<f64>,
    pub judge: Option<f64>,
    pub steps: usize,
    pub seconds: f64,
    pub forced: usize,
}

pub fn score_turn(
    spec: &TurnSpec,
    answer: &str,
    recording: &Recording,
    steps: usize,
    forced: bool,
    seconds: f64,
) -> TurnScore {
    let mut missed = Vec::new();
    let mut facts_hit = 0;
    for fact in &spec.facts {
        if fact_found(answer, fact) {
            facts_hit += 1;
        } else {
            missed.push(fact.text.clone());
        }
    }
    let cites = citations(answer);
    let bad = bad_citations(answer, recording);
    let mut remaining = bad.clone();
    let mut precise = 0;
    let mut cited = Vec::new();
    for (written, t) in &cites {
        cited.push(written.clone());
        if let Some(i) = remaining.iter().position(|b| b == written) {
            remaining.remove(i);
            continue;
        }
        let inside = spec
            .intervals
            .iter()
            .any(|(start, end)| *t >= start - SLOP_SECONDS && *t <= end + SLOP_SECONDS);
        if inside {
            precise += 1;
        }
    }
    TurnScore {
        question: spec.question.clone(),
        facts_hit,
        facts_total: spec.facts.len(),
        missed,
        forbidden: forbidden_found(answer, &spec.forbidden),
        cited,
        precise,
        bad_citations: bad,
        wanted_interval: !spec.intervals.is_empty(),
        steps,
        forced,
        seconds,
        judge: None,
        judge_error: None,
    }
}

pub fn repeat_from_turns(turns: &[TurnScore]) -> RepeatScores {
    let judged: Vec<f64> = turns
        .iter()
        .filter_map(|t| t.judge.as_ref().map(|j| f64::from(j.score)))
        .collect();
    RepeatScores {
        facts_hit: turns.iter().map(|t| t.facts_hit).sum(),
        facts_total: turns.iter().map(|t| t.facts_total).sum(),
        forbidden_hits: turns.iter().map(|t| t.forbidden.len()).sum(),
        citation_precision: if turns.is_empty() {
            Some(1.0)
        } else {
            let counted: Vec<f64> = turns.iter().filter_map(TurnScore::precision).collect();
            if counted.is_empty() {
                None
            } else {
                Some(counted.iter().sum::<f64>() / counted.len() as f64)
            }
        },
        judge: if judged.is_empty() {
            None
        } else {
            Some(judged.iter().sum::<f64>() / judged.len() as f64)
        },
        steps: turns.iter().map(|t| t.steps).sum(),
        seconds: turns.iter().map(|t| t.seconds).sum(),
        forced: turns.iter().filter(|t| t.forced).count(),
    }
}

fn fact_found(answer: &str, fact: &Fact) -> bool {
    let answer = answer.to_lowercase();
    fact.any
        .iter()
        .any(|part| !part.is_empty() && answer.contains(&part.to_lowercase()))
}

fn forbidden_found(answer: &str, forbidden: &[String]) -> Vec<String> {
    let answer = answer.to_lowercase();
    forbidden
        .iter()
        .filter(|part| !part.is_empty() && answer.contains(&part.to_lowercase()))
        .cloned()
        .collect()
}

/// JSON Schema of the judge's answer. The grammar forces this shape; the prompt's wording is separate.
pub fn judge_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "score": {"type": "integer", "minimum": 0, "maximum": 100},
            "reason": {"type": "string", "maxLength": 400}
        },
        "required": ["score", "reason"],
        "additionalProperties": false
    })
}

pub fn parse_judge(raw: &str) -> anyhow::Result<JudgeVerdict> {
    let raw = raw.trim();
    let value: Value = if let Ok(value) = serde_json::from_str(raw) {
        value
    } else {
        let start = raw
            .find('{')
            .ok_or_else(|| anyhow::anyhow!("judge did not return JSON"))?;
        let end = raw
            .rfind('}')
            .ok_or_else(|| anyhow::anyhow!("judge did not return JSON"))?;
        anyhow::ensure!(end >= start, "judge did not return JSON: {raw}");
        serde_json::from_str(&raw[start..=end])?
    };
    let score = value
        .get("score")
        .and_then(Value::as_f64)
        .or_else(|| value.get("score").and_then(Value::as_u64).map(|n| n as f64));
    let score = score
        .filter(|n| n.is_finite())
        .ok_or_else(|| anyhow::anyhow!("judge score is missing: {raw}"))?;
    let reason = value
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(JudgeVerdict {
        score: score.round().clamp(0.0, 100.0) as u32,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cases::{Fact, TurnSpec};
    use mediaa_core::transcript::Reply;

    fn spec(facts: Vec<Fact>, intervals: Vec<(f64, f64)>, forbidden: &[&str]) -> TurnSpec {
        TurnSpec {
            question: "Q".into(),
            reference: "R".into(),
            facts,
            intervals,
            forbidden: forbidden.iter().copied().map(str::to_string).collect(),
        }
    }

    fn fact(text: &str, any: &[&str]) -> Fact {
        Fact {
            text: text.into(),
            any: any.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// A reply from `start` to `end` seconds. Citations near it are real.
    fn recording(start: f64, end: f64) -> Recording {
        Recording {
            duration: end + 30.0,
            replies: vec![Reply {
                start,
                end,
                speaker: None,
                words: vec![],
            }],
            ..Recording::default()
        }
    }

    fn scored(spec: &TurnSpec, answer: &str, recording: &Recording) -> TurnScore {
        score_turn(spec, answer, recording, 4, true, 12.0)
    }

    #[test]
    fn facts_are_case_insensitive_and_any_alternative_passes() {
        let spec = spec(
            vec![fact("120 000 рублей", &["120 000", "120000", "120 тысяч"])],
            vec![(100.0, 110.0)],
            &[],
        );
        let hit = scored(
            &spec,
            "Стоит до 120000 рублей [1:40].",
            &recording(100.0, 110.0),
        );
        assert_eq!(hit.facts_hit, 1);
        let miss = scored(&spec, "Дорого [1:40].", &recording(100.0, 110.0));
        assert_eq!(miss.missed, ["120 000 рублей"]);
        assert!(
            (miss.precision().unwrap() - 1.0).abs() < 1e-9,
            "1:40 is inside the interval and on a reply"
        );
    }

    #[test]
    fn forbidden_hits_and_a_negative_case() {
        let spec = spec(
            vec![fact("не сказано", &["не сказано", "не говорит"])],
            vec![],
            &["4%", "Процент"],
        );
        let clean = scored(&spec, "В записи этого НЕ СКАЗАНО.", &recording(0.0, 10.0));
        assert_eq!((clean.facts_hit, clean.forbidden.len()), (1, 0));
        assert!(clean.precision().is_none(), "no interval is not counted");
        let cited = scored(&spec, "В записи этого не сказано [1:40].", &recording(0.0, 10.0));
        assert!(cited.precision().is_none(), "citations without an interval are not 0");
        let dirty = scored(
            &spec,
            "Не сказано, налог 4% и ещё процент.",
            &recording(0.0, 10.0),
        );
        assert_eq!(dirty.forbidden, ["4%", "Процент"]);
    }

    #[test]
    fn citations_must_be_real_and_inside_the_interval() {
        // Reply 1:40–1:50. Interval 1:40–1:42, so ±15 s ends at 1:57.
        let turn = spec(vec![], vec![(100.0, 102.0)], &[]);
        let rec = recording(100.0, 110.0);
        let edge = scored(&turn, "Сказано [1:57].", &rec);
        assert_eq!(
            (edge.cited.len(), edge.precise, edge.bad_citations.len()),
            (1, 1, 0)
        );
        let late = scored(&turn, "Сказано [1:58].", &rec);
        assert_eq!(
            late.precise, 0,
            "1:58 is on a reply but outside the interval"
        );
        assert!((late.precision().unwrap() - 0.0).abs() < 1e-9);
        let wide = spec(vec![], vec![(0.0, 20.0)], &[]);
        let invented = scored(&wide, "Сказано [0:10].", &rec);
        assert_eq!(invented.bad_citations, ["0:10"]);
        assert_eq!(
            invented.precise, 0,
            "a time inside the interval with no reply does not count"
        );
        let silent = scored(&turn, "Без времени.", &rec);
        assert!(
            (silent.precision().unwrap() - 0.0).abs() < 1e-9,
            "an expected interval and no citation scores 0"
        );
    }

    #[test]
    fn a_follow_up_averages_turn_precision_and_sums_facts() {
        let mut first = scored(
            &spec(vec![fact("a", &["a"])], vec![(100.0, 110.0)], &[]),
            "a [1:40]",
            &recording(100.0, 110.0),
        );
        let mut second = scored(
            &spec(
                vec![fact("b", &["b"]), fact("c", &["c"])],
                vec![(100.0, 110.0)],
                &["нет"],
            ),
            "нет",
            &recording(100.0, 110.0),
        );
        first.judge = Some(JudgeVerdict {
            score: 80,
            reason: "ok".into(),
        });
        second.judge = Some(JudgeVerdict {
            score: 40,
            reason: "thin".into(),
        });
        second.forced = false;
        let repeat = repeat_from_turns(&[first, second]);
        assert_eq!(
            (repeat.facts_hit, repeat.facts_total, repeat.forbidden_hits),
            (1, 3, 1)
        );
        assert!(
            (repeat.citation_precision.unwrap() - 0.5).abs() < 1e-9,
            "one turn cited well, one cited nothing"
        );
        assert_eq!(repeat.judge, Some(60.0));
        assert_eq!((repeat.steps, repeat.forced), (8, 1));
    }

    #[test]
    fn a_turn_without_an_interval_is_left_out_of_the_mean() {
        let counted = scored(
            &spec(vec![], vec![(100.0, 110.0)], &[]),
            "Сказано [1:40].",
            &recording(100.0, 110.0),
        );
        let skipped = scored(&spec(vec![], vec![], &[]), "Сказано [1:40].", &recording(100.0, 110.0));
        let repeat = repeat_from_turns(&[counted, skipped.clone()]);
        assert!((repeat.citation_precision.unwrap() - 1.0).abs() < 1e-9);
        assert!(repeat_from_turns(&[skipped]).citation_precision.is_none());
    }

    #[test]
    fn the_judge_grammar_is_accepted_and_the_score_is_clamped() {
        mediaa_engine::llm::check_schema(&judge_schema().to_string()).unwrap();
        let ok = parse_judge(r#"{"score":80,"reason":"полно"}"#).unwrap();
        assert_eq!((ok.score, ok.reason.as_str()), (80, "полно"));
        assert_eq!(
            parse_judge("noise {\"score\": 150.2, \"reason\": \"x\"} tail")
                .unwrap()
                .score,
            100
        );
        assert_eq!(parse_judge(r#"{"score":-3,"reason":""}"#).unwrap().score, 0);
        assert!(parse_judge("no json").is_err());
    }
}
