use anyhow::Context;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub text: String,
    pub any: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnSpec {
    pub question: String,
    pub reference: String,
    pub facts: Vec<Fact>,
    pub intervals: Vec<(f64, f64)>,
    pub forbidden: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub id: String,
    pub level: String,
    pub kind: String,
    pub turns: Vec<TurnSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suite {
    pub recording: String,
    pub cases: Vec<Case>,
}

#[derive(Deserialize)]
struct RawSuite {
    recording: String,
    cases: Vec<RawCase>,
}

#[derive(Deserialize)]
struct RawCase {
    id: String,
    level: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    question: String,
    #[serde(default)]
    answer: String,
    #[serde(default)]
    facts: Vec<RawFact>,
    #[serde(default)]
    intervals: Vec<(String, String)>,
    #[serde(default)]
    forbidden: Vec<String>,
    #[serde(default)]
    turns: Vec<RawTurn>,
}

#[derive(Deserialize)]
struct RawTurn {
    question: String,
    answer: String,
    #[serde(default)]
    facts: Vec<RawFact>,
    #[serde(default)]
    intervals: Vec<(String, String)>,
    #[serde(default)]
    forbidden: Vec<String>,
}

#[derive(Deserialize)]
struct RawFact {
    #[serde(default)]
    text: String,
    #[serde(default)]
    any: Vec<String>,
}

pub fn load_cases(path: &Path) -> anyhow::Result<Suite> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_cases(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn parse_cases(text: &str) -> anyhow::Result<Suite> {
    let raw: RawSuite = serde_yaml::from_str(text)?;
    let mut seen = Vec::new();
    let mut cases = Vec::new();
    for case in raw.cases {
        let id = case.id.trim().to_string();
        anyhow::ensure!(!id.is_empty(), "a case has no id");
        anyhow::ensure!(!seen.contains(&id), "duplicate case id {id}");
        seen.push(id.clone());
        anyhow::ensure!(
            !case.level.trim().is_empty() && !case.kind.trim().is_empty(),
            "case {id} needs a level and a type"
        );
        let shared = clean_forbidden(case.forbidden);
        let turns = if case.turns.is_empty() {
            anyhow::ensure!(
                !case.question.trim().is_empty(),
                "case {id} has no question"
            );
            vec![raw_turn(
                &id,
                &case.question,
                &case.answer,
                &case.facts,
                &case.intervals,
                shared,
            )?]
        } else {
            let mut turns = Vec::new();
            for (i, turn) in case.turns.iter().enumerate() {
                anyhow::ensure!(
                    !turn.question.trim().is_empty(),
                    "case {id} turn {} has no question",
                    i + 1
                );
                let mut forbidden = clean_forbidden(turn.forbidden.clone());
                for item in &shared {
                    if !forbidden.iter().any(|f| f == item) {
                        forbidden.push(item.clone());
                    }
                }
                turns.push(raw_turn(
                    &id,
                    &turn.question,
                    &turn.answer,
                    &turn.facts,
                    &turn.intervals,
                    forbidden,
                )?);
            }
            turns
        };
        cases.push(Case {
            id,
            level: case.level.trim().to_string(),
            kind: case.kind.trim().to_string(),
            turns,
        });
    }
    anyhow::ensure!(!cases.is_empty(), "the file has no cases");
    Ok(Suite {
        recording: raw.recording,
        cases,
    })
}

fn raw_turn(
    id: &str,
    question: &str,
    answer: &str,
    facts: &[RawFact],
    intervals: &[(String, String)],
    forbidden: Vec<String>,
) -> anyhow::Result<TurnSpec> {
    let mut parsed = Vec::new();
    for (start, end) in intervals {
        let start = clock(start).with_context(|| format!("case {id}"))?;
        let end = clock(end).with_context(|| format!("case {id}"))?;
        parsed.push(if start <= end {
            (start, end)
        } else {
            (end, start)
        });
    }
    Ok(TurnSpec {
        question: question.trim().to_string(),
        reference: answer.trim().to_string(),
        facts: facts.iter().map(clean_fact).collect(),
        intervals: parsed,
        forbidden,
    })
}

fn clean_fact(fact: &RawFact) -> Fact {
    let text = fact.text.trim().to_string();
    let mut any: Vec<String> = fact
        .any
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if any.is_empty() && !text.is_empty() {
        any.push(text.clone());
    }
    Fact { text, any }
}

fn clean_forbidden(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn clock(text: &str) -> anyhow::Result<f64> {
    mediaa_agent::tools::parse_time(text.trim())
        .with_context(|| format!("{text:?} is not a time (use m:ss)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
recording: demo
cases:
  - id: author-name
    level: L1
    type: fact
    question: Как зовут?
    answer: Антон.
    facts:
      - { text: Антон Назаров, any: [Назаров] }
    intervals: [["11:15", "12:21"], ["25:13", "26:34"]]
  - id: taxes
    level: L1
    type: negative
    question: Налоги?
    answer: Нет.
    facts:
      - { text: не сказано, any: ["не сказано"] }
    intervals: []
    forbidden: ["4%", "процент"]
  - id: brother
    level: L1
    type: followup
    forbidden: ["выдумка"]
    turns:
      - question: Что с братом?
        answer: Бан.
        facts:
          - { text: час, any: ["час"] }
        intervals: [["16:03", "17:11"]]
      - question: Как зовут?
        answer: Дмитрий.
        facts:
          - { text: Дмитрий }
        intervals: [["17:11", "16:03"]]
"#;

    #[test]
    fn yaml_cases_turns_and_times() {
        let suite = parse_cases(SAMPLE).unwrap();
        assert_eq!(suite.recording, "demo");
        assert_eq!(suite.cases.len(), 3);
        let author = &suite.cases[0];
        assert_eq!(author.kind, "fact");
        assert_eq!(author.turns.len(), 1);
        assert_eq!(author.turns[0].facts[0].any, ["Назаров"]);
        assert_eq!(
            author.turns[0].intervals,
            vec![(675.0, 741.0), (1513.0, 1594.0)]
        );
        let taxes = &suite.cases[1];
        assert!(taxes.turns[0].intervals.is_empty());
        assert_eq!(taxes.turns[0].forbidden, ["4%", "процент"]);
        let brother = &suite.cases[2];
        assert_eq!(brother.turns.len(), 2);
        assert_eq!(
            brother.turns[1].facts[0].any,
            ["Дмитрий"],
            "an empty any falls back to the fact text"
        );
        assert_eq!(
            brother.turns[1].intervals,
            vec![(963.0, 1031.0)],
            "a reversed pair is ordered"
        );
        assert_eq!(
            brother.turns[0].forbidden,
            ["выдумка"],
            "a case-level forbidden list applies to every turn"
        );
    }

    #[test]
    fn a_duplicate_id_or_a_missing_question_is_an_error() {
        assert!(parse_cases("recording: x\ncases:\n  - {id: a, level: L1, type: fact, question: q}\n  - {id: a, level: L1, type: fact, question: q}\n").is_err());
        assert!(parse_cases("recording: x\ncases:\n  - {id: a, level: L1, type: fact}\n").is_err());
        assert!(parse_cases("recording: x\ncases: []\n").is_err());
    }

    #[test]
    fn the_local_hh_file_parses_when_it_is_present() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../evals/local/hh-experience/cases.yaml");
        if !path.is_file() {
            return;
        }
        let suite = load_cases(&path).unwrap();
        let author = suite.cases.iter().find(|c| c.id == "author-name").unwrap();
        assert_eq!(author.turns[0].intervals[0].0, 675.0);
        assert_eq!(author.turns[0].intervals.len(), 2);
        assert!(author.turns[0].facts[0].any.iter().any(|s| s == "Назаров"));
        let brother = suite.cases.iter().find(|c| c.id == "brother").unwrap();
        assert_eq!(brother.kind, "followup");
        assert_eq!(brother.turns.len(), 2);
        let taxes = suite.cases.iter().find(|c| c.id == "taxes").unwrap();
        assert!(taxes.turns[0].forbidden.iter().any(|s| s == "процент"));
        assert!(taxes.turns[0].intervals.is_empty());
        let tips = suite.cases.iter().find(|c| c.id == "tips").unwrap();
        assert!(tips.turns[0].forbidden.iter().any(|s| s == "150 000"));
        assert!(tips.turns[0].facts.len() >= 6);
    }
}
