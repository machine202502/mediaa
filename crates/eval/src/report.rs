use crate::cases::Case;
use crate::score::{RepeatScores, TurnScore};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub level: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub facts_hit: usize,
    pub facts_total: usize,
    pub forbidden_hits: f64,
    pub citation_precision: Option<f64>,
    pub judge: Option<f64>,
    pub steps: f64,
    pub seconds: f64,
    pub forced: usize,
}

impl Row {
    pub fn recall(&self) -> f64 {
        if self.facts_total == 0 {
            1.0
        } else {
            self.facts_hit as f64 / self.facts_total as f64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub label: String,
    pub cases_file: String,
    pub repeat: usize,
    pub judge: String,
    pub rows: Vec<Row>,
    pub model_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub errors: Vec<String>,
}

pub fn row_from_repeats(case: &Case, repeats: &[RepeatScores]) -> Row {
    let n = repeats.len().max(1) as f64;
    let judged: Vec<f64> = repeats.iter().filter_map(|r| r.judge).collect();
    Row {
        id: case.id.clone(),
        level: case.level.clone(),
        kind: case.kind.clone(),
        facts_hit: repeats.iter().map(|r| r.facts_hit).sum(),
        facts_total: repeats.iter().map(|r| r.facts_total).sum(),
        forbidden_hits: repeats.iter().map(|r| r.forbidden_hits as f64).sum::<f64>() / n,
        citation_precision: mean(
            repeats
                .iter()
                .filter_map(|r| r.citation_precision)
                .collect(),
        ),
        judge: if judged.is_empty() {
            None
        } else {
            Some(judged.iter().sum::<f64>() / judged.len() as f64)
        },
        steps: repeats.iter().map(|r| r.steps as f64).sum::<f64>() / n,
        seconds: repeats.iter().map(|r| r.seconds).sum::<f64>() / n,
        forced: repeats.iter().map(|r| r.forced).sum(),
    }
}

pub fn report_markdown(summary: &Summary) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Eval {}\n\n", summary.label));
    out.push_str(&format!(
        "{} cases · repeat {} · judge {}\n\n",
        summary.rows.len(),
        summary.repeat,
        summary.judge
    ));
    out.push_str(&case_table(&summary.rows));
    out.push_str("\n## By level\n\n");
    out.push_str(&group_table(
        "level",
        &grouped(&summary.rows, |r| &r.level),
        Count::Mean,
    ));
    out.push_str("\n## By type\n\n");
    out.push_str(&group_table(
        "type",
        &grouped(&summary.rows, |r| &r.kind),
        Count::Mean,
    ));
    out.push_str("\n## Total\n\n");
    out.push_str(&group_table(
        "total",
        &[("all".into(), summary.rows.iter().collect())],
        Count::Sum,
    ));
    out.push_str(&format!(
        "\nModel calls: {} · tokens: {} in / {} out\n",
        summary.model_calls, summary.prompt_tokens, summary.completion_tokens
    ));
    let forced: Vec<&Row> = summary.rows.iter().filter(|r| r.forced > 0).collect();
    if !forced.is_empty() {
        let list = forced
            .iter()
            .map(|r| format!("{} ({})", r.id, r.forced))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("\nForced answers: {list}\n"));
    }
    if !summary.errors.is_empty() {
        out.push_str("\n## Errors\n\n");
        for error in &summary.errors {
            out.push_str(&format!("- {error}\n"));
        }
    }
    out.push_str(
        "\nFact recall is facts found divided by facts required, over every repeat. \
Citation precision is the mean of the turns that were given an expected interval: a cited time \
counts when it points at a real reply and lies inside that interval, give or take 15 seconds; a \
turn that was given intervals and cited nothing scores 0; a turn that was given no interval is \
not counted and shows —. By level and by type, forbidden hits, steps and time are means per case; \
the total row sums them. The judge column is the mean score, or — when this run did not ask a judge.\n",
    );
    out
}

pub fn compare_markdown(a_path: &Path, a: &Summary, b_path: &Path, b: &Summary) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} → {}\n\n", a.label, b.label));
    out.push_str(&format!(
        "A: `{}`\n\nB: `{}`\n\n",
        a_path.display(),
        b_path.display()
    ));
    out.push_str(&format!(
        "judge {} → {}, repeat {} → {}\n\n",
        a.judge, b.judge, a.repeat, b.repeat
    ));
    out.push_str("| case | fact recall | forbidden hits | citation precision | judge | steps | time | worse |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- | --- | --- |\n");
    let mut worse = Vec::new();
    let mut only_b = Vec::new();
    for row_a in &a.rows {
        let Some(row_b) = b.rows.iter().find(|r| r.id == row_a.id) else {
            out.push_str(&format!(
                "| {} | {} | | | | | | only in A |\n",
                row_a.id,
                recall_cell(row_a)
            ));
            continue;
        };
        let reasons = worse_reasons(row_a, row_b);
        if !reasons.is_empty() {
            worse.push((row_a.id.clone(), reasons.join(", ")));
        }
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            row_a.id,
            diff(&recall_cell(row_a), &recall_cell(row_b)),
            diff(
                &fmt_num(row_a.forbidden_hits),
                &fmt_num(row_b.forbidden_hits)
            ),
            diff(
                &precision_cell(row_a.citation_precision),
                &precision_cell(row_b.citation_precision)
            ),
            diff(&judge_cell(row_a.judge), &judge_cell(row_b.judge)),
            diff(&fmt_num(row_a.steps), &fmt_num(row_b.steps)),
            diff(&clock(row_a.seconds), &clock(row_b.seconds)),
            reasons.join(", "),
        ));
    }
    for row_b in &b.rows {
        if !a.rows.iter().any(|r| r.id == row_b.id) {
            only_b.push(row_b.id.clone());
            out.push_str(&format!(
                "| {} | {} | | | | | | only in B |\n",
                row_b.id,
                recall_cell(row_b)
            ));
        }
    }
    out.push_str("\n## Worse\n\n");
    if worse.is_empty() {
        out.push_str("Nothing got worse.\n");
    } else {
        for (id, why) in &worse {
            out.push_str(&format!("- **{id}** — {why}\n"));
        }
    }
    if !only_b.is_empty() {
        out.push_str("\n## Only in B\n\n");
        for id in &only_b {
            out.push_str(&format!("- {id}\n"));
        }
    }
    let only_a: Vec<&str> = a
        .rows
        .iter()
        .filter(|r| !b.rows.iter().any(|o| o.id == r.id))
        .map(|r| r.id.as_str())
        .collect();
    if !only_a.is_empty() {
        out.push_str("\n## Only in A\n\n");
        for id in only_a {
            out.push_str(&format!("- {id}\n"));
        }
    }
    out.push_str(
        "\nWorse means lower fact recall, more forbidden hits, lower citation precision, \
or a lower judge score when both runs have one. Steps and time are shown and are not counted as worse.\n",
    );
    out
}

fn worse_reasons(a: &Row, b: &Row) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if b.recall() + 1e-9 < a.recall() {
        reasons.push("fact recall");
    }
    if b.forbidden_hits > a.forbidden_hits + 1e-9 {
        reasons.push("forbidden hits");
    }
    if let (Some(before), Some(after)) = (a.citation_precision, b.citation_precision) {
        if after + 1e-9 < before {
            reasons.push("citation precision");
        }
    }
    if let (Some(before), Some(after)) = (a.judge, b.judge) {
        if after + 1e-6 < before {
            reasons.push("judge");
        }
    }
    reasons
}

fn diff(a: &str, b: &str) -> String {
    if a == b {
        a.to_string()
    } else {
        format!("{a} → {b}")
    }
}

#[derive(Clone, Copy)]
enum Count {
    Mean,
    Sum,
}

fn grouped(rows: &[Row], key: impl Fn(&Row) -> &str) -> Vec<(String, Vec<&Row>)> {
    let mut names: Vec<String> = rows.iter().map(&key).map(str::to_string).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            (
                name.clone(),
                rows.iter().filter(|r| key(r) == name).collect(),
            )
        })
        .collect()
}

fn case_table(rows: &[Row]) -> String {
    let mut out = String::from("| case | level | type | fact recall | forbidden hits | citation precision | judge | steps | time |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for row in rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            row.id,
            row.level,
            row.kind,
            recall_cell(row),
            fmt_num(row.forbidden_hits),
            precision_cell(row.citation_precision),
            judge_cell(row.judge),
            fmt_num(row.steps),
            clock(row.seconds),
        ));
    }
    out
}

fn group_table(key: &str, groups: &[(String, Vec<&Row>)], count: Count) -> String {
    let mut out = format!("| {key} | cases | fact recall | forbidden hits | citation precision | judge | steps | time |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for (name, rows) in groups {
        let agg = aggregate(rows, count);
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            name,
            rows.len(),
            recall_pair(agg.recall, agg.facts_hit, agg.facts_total),
            fmt_num(agg.forbidden),
            precision_cell(agg.precision),
            judge_cell(agg.judge),
            fmt_num(agg.steps),
            clock(agg.seconds),
        ));
    }
    out
}

struct Agg {
    facts_hit: usize,
    facts_total: usize,
    recall: f64,
    forbidden: f64,
    precision: Option<f64>,
    judge: Option<f64>,
    steps: f64,
    seconds: f64,
}

fn aggregate(rows: &[&Row], count: Count) -> Agg {
    let n = rows.len().max(1) as f64;
    let facts_hit = rows.iter().map(|r| r.facts_hit).sum();
    let facts_total = rows.iter().map(|r| r.facts_total).sum();
    let fold = |values: Vec<f64>| match count {
        Count::Mean => values.iter().sum::<f64>() / n,
        Count::Sum => values.iter().sum(),
    };
    let judged: Vec<f64> = rows.iter().filter_map(|r| r.judge).collect();
    Agg {
        facts_hit,
        facts_total,
        recall: if facts_total == 0 {
            1.0
        } else {
            facts_hit as f64 / facts_total as f64
        },
        forbidden: fold(rows.iter().map(|r| r.forbidden_hits).collect()),
        precision: if rows.is_empty() {
            Some(1.0)
        } else {
            mean(rows.iter().filter_map(|r| r.citation_precision).collect())
        },
        judge: if judged.is_empty() {
            None
        } else {
            Some(judged.iter().sum::<f64>() / judged.len() as f64)
        },
        steps: fold(rows.iter().map(|r| r.steps).collect()),
        seconds: fold(rows.iter().map(|r| r.seconds).collect()),
    }
}

fn recall_cell(row: &Row) -> String {
    recall_pair(row.recall(), row.facts_hit, row.facts_total)
}

fn recall_pair(recall: f64, hit: usize, total: usize) -> String {
    format!("{} ({hit}/{total})", pct(recall))
}

fn pct(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

fn precision_cell(value: Option<f64>) -> String {
    value.map(pct).unwrap_or_else(|| "—".into())
}

fn mean(values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn judge_cell(score: Option<f64>) -> String {
    match score {
        Some(score) => format!("{score:.0}"),
        None => "—".into(),
    }
}

fn fmt_num(value: f64) -> String {
    if (value - value.round()).abs() < 1e-6 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn clock(seconds: f64) -> String {
    let whole = seconds.round().max(0.0) as u64;
    if whole >= 60 {
        format!("{}m{:02}s", whole / 60, whole % 60)
    } else {
        format!("{whole}s")
    }
}

pub fn save_summary(dir: &Path, summary: &Summary) -> anyhow::Result<()> {
    let text = report_markdown(summary);
    std::fs::write(dir.join("report.md"), &text)?;
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(summary).expect("summary serialises") + "\n",
    )?;
    Ok(())
}

pub fn load_summary(dir: &Path) -> anyhow::Result<Summary> {
    let path = dir.join("summary.json");
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no summary in {} (run mediaa-eval run first)",
            dir.display()
        )
    })?;
    serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
}

pub fn compare_runs(a: &Path, b: &Path) -> anyhow::Result<PathBuf> {
    let left = load_summary(a)?;
    let right = load_summary(b)?;
    let path = b.join("compare.md");
    std::fs::write(&path, compare_markdown(a, &left, b, &right))?;
    Ok(path)
}

pub struct RepeatArtifacts<'a> {
    pub questions: &'a [String],
    pub events: &'a [serde_json::Value],
    pub answers: &'a [String],
    pub shown: &'a [String],
    pub turns: &'a [TurnScore],
    pub stats: &'a StatsFile,
    pub model_steps: &'a [ModelSteps],
}

pub fn write_repeat(dir: &Path, repeat: &RepeatArtifacts) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("questions.txt"), numbered(repeat.questions))?;
    std::fs::write(dir.join("answer.txt"), numbered(repeat.answers))?;
    std::fs::write(dir.join("answer_shown.txt"), numbered(repeat.shown))?;
    let events_text = repeat
        .events
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        dir.join("events.jsonl"),
        if events_text.is_empty() {
            String::new()
        } else {
            format!("{events_text}\n")
        },
    )?;
    let turns: Vec<serde_json::Value> = repeat
        .turns
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let model = repeat.model_steps.get(i).cloned().unwrap_or_default();
            serde_json::json!({
                "seconds": t.seconds,
                "steps": t.steps,
                "forced": t.forced,
                "model_steps": model.steps,
                "calls_per_step": model.calls_per_step,
            })
        })
        .collect();
    let timings = serde_json::json!({
        "seconds": repeat.turns.iter().map(|t| t.seconds).sum::<f64>(),
        "turns": turns,
    });
    std::fs::write(
        dir.join("timings.json"),
        serde_json::to_string_pretty(&timings).expect("timings") + "\n",
    )?;
    std::fs::write(
        dir.join("stats.json"),
        serde_json::to_string_pretty(repeat.stats).expect("stats") + "\n",
    )?;
    std::fs::write(
        dir.join("scores.json"),
        serde_json::to_string_pretty(&scores_json(repeat.turns)).expect("scores") + "\n",
    )?;
    Ok(())
}

fn numbered(texts: &[String]) -> String {
    let mut out = String::new();
    for (i, text) in texts.iter().enumerate() {
        out.push_str(&format!("# {}\n{text}\n\n", i + 1));
    }
    out
}

fn scores_json(turns: &[TurnScore]) -> serde_json::Value {
    let repeat = crate::score::repeat_from_turns(turns);
    serde_json::json!({
        "fact_recall": if repeat.facts_total == 0 { 1.0 } else { repeat.facts_hit as f64 / repeat.facts_total as f64 },
        "facts_hit": repeat.facts_hit,
        "facts_total": repeat.facts_total,
        "forbidden_hits": repeat.forbidden_hits,
        "citation_precision": repeat.citation_precision,
        "steps": repeat.steps,
        "forced": repeat.forced,
        "seconds": repeat.seconds,
        "judge": repeat.judge,
        "turns": turns.iter().map(turn_json).collect::<Vec<_>>(),
    })
}

fn turn_json(turn: &TurnScore) -> serde_json::Value {
    serde_json::json!({
        "question": turn.question,
        "facts_hit": turn.facts_hit,
        "facts_total": turn.facts_total,
        "missed": turn.missed,
        "forbidden": turn.forbidden,
        "citation_precision": turn.precision(),
        "citations": turn.cited,
        "bad_citations": turn.bad_citations,
        "steps": turn.steps,
        "forced": turn.forced,
        "seconds": turn.seconds,
        "judge": turn.judge.as_ref().map(|j| serde_json::json!({"score": j.score, "reason": j.reason})),
        "judge_error": turn.judge_error,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelSteps {
    pub steps: usize,
    pub calls_per_step: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsFile {
    pub model_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub model_ms: u64,
    pub models: Vec<String>,
    pub turns: Vec<TurnStats>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TurnStats {
    pub model_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub model_ms: u64,
    pub models: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{JudgeVerdict, TurnScore};

    #[allow(clippy::too_many_arguments)]
    fn row(
        id: &str,
        level: &str,
        kind: &str,
        hit: usize,
        total: usize,
        forbidden: f64,
        precision: Option<f64>,
        judge: Option<f64>,
        steps: f64,
        seconds: f64,
    ) -> Row {
        Row {
            id: id.into(),
            level: level.into(),
            kind: kind.into(),
            facts_hit: hit,
            facts_total: total,
            forbidden_hits: forbidden,
            citation_precision: precision,
            judge,
            steps,
            seconds,
            forced: 0,
        }
    }

    fn summary(rows: Vec<Row>) -> Summary {
        Summary {
            label: "before".into(),
            cases_file: "cases.yaml".into(),
            repeat: 1,
            judge: "none".into(),
            rows,
            model_calls: 3,
            prompt_tokens: 10,
            completion_tokens: 4,
            errors: vec![],
        }
    }

    #[test]
    fn the_report_has_cases_levels_types_and_totals() {
        let text = report_markdown(&summary(vec![
            row("a", "L1", "fact", 1, 1, 0.0, Some(1.0), None, 2.0, 10.0),
            row(
                "b",
                "L2",
                "time",
                1,
                2,
                2.0,
                Some(0.0),
                Some(40.0),
                4.0,
                90.0,
            ),
        ]));
        assert!(
            text.contains("| a | L1 | fact | 100% (1/1) | 0 | 100% | — | 2 | 10s |"),
            "{text}"
        );
        assert!(
            text.contains("| b | L2 | time | 50% (1/2) | 2 | 0% | 40 | 4 | 1m30s |"),
            "{text}"
        );
        assert!(
            text.contains("## By level")
                && text.contains("| L1 | 1 | 100% (1/1) | 0 | 100% | — | 2 | 10s |"),
            "{text}"
        );
        assert!(
            text.contains("| L2 | 1 | 50% (1/2) | 2 | 0% | 40 | 4 | 1m30s |"),
            "{text}"
        );
        assert!(
            text.contains("## By type")
                && text.contains("| fact | 1 |")
                && text.contains("| time | 1 |"),
            "{text}"
        );
        assert!(
            text.contains("| all | 2 | 67% (2/3) | 2 | 50% | 40 | 6 | 1m40s |"),
            "{text}"
        );
        assert!(
            text.contains("Model calls: 3 · tokens: 10 in / 4 out"),
            "{text}"
        );
        assert!(!text.contains("Forced answers"), "{text}");
    }

    #[test]
    fn compare_lists_what_got_worse_and_ignores_steps() {
        let before = summary(vec![row(
            "a",
            "L1",
            "fact",
            1,
            1,
            0.0,
            Some(1.0),
            None,
            2.0,
            10.0,
        )]);
        let mut after = summary(vec![
            row(
                "a",
                "L1",
                "fact",
                1,
                2,
                2.0,
                Some(0.5),
                Some(10.0),
                8.0,
                90.0,
            ),
            row("new", "L1", "fact", 1, 1, 0.0, Some(1.0), None, 1.0, 5.0),
        ]);
        after.label = "after".into();
        let text = compare_markdown(
            Path::new("target/evals/a"),
            &before,
            Path::new("target/evals/b"),
            &after,
        );
        assert!(
            text.contains(
                "## Worse\n\n- **a** — fact recall, forbidden hits, citation precision\n"
            ),
            "{text}"
        );
        assert!(text.contains("## Only in B\n\n- new\n"), "{text}");
        assert!(
            text.contains("8") && text.contains("1m"),
            "steps and time are shown"
        );
        let same = compare_markdown(Path::new("a"), &before, Path::new("b"), &before);
        assert!(same.contains("Nothing got worse."), "{same}");
    }

    #[test]
    fn a_repeat_folder_keeps_the_question_the_events_and_the_scores() {
        let dir = tempfile::tempdir().unwrap();
        let turn = TurnScore {
            question: "Как зовут?".into(),
            facts_hit: 1,
            facts_total: 1,
            missed: vec![],
            forbidden: vec![],
            cited: vec!["1:40".into()],
            precise: 1,
            bad_citations: vec![],
            wanted_interval: true,
            steps: 3,
            forced: false,
            seconds: 1.5,
            judge: Some(JudgeVerdict {
                score: 70,
                reason: "есть имя".into(),
            }),
            judge_error: None,
        };
        let stats = StatsFile {
            model_calls: 2,
            prompt_tokens: 11,
            completion_tokens: 4,
            model_ms: 9,
            models: vec!["m".into()],
            turns: vec![TurnStats {
                model_calls: 2,
                prompt_tokens: 11,
                completion_tokens: 4,
                model_ms: 9,
                models: vec!["m".into()],
            }],
        };
        let events = vec![serde_json::json!({"turn": 1, "event": "step", "tool": "search"})];
        write_repeat(
            dir.path(),
            &RepeatArtifacts {
                questions: &["Как зовут?".into()],
                events: &events,
                answers: &["@s1 сказал [1:40].".into()],
                shown: &["@Антон сказал [1:40].".into()],
                turns: &[turn],
                stats: &stats,
                model_steps: &[ModelSteps {
                    steps: 3,
                    calls_per_step: vec![1, 0],
                }],
            },
        )
        .unwrap();
        let questions = std::fs::read_to_string(dir.path().join("questions.txt")).unwrap();
        assert!(questions.contains("Как зовут?"), "{questions}");
        let events = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
        assert!(events.contains("\"tool\":\"search\""), "{events}");
        let scores: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("scores.json")).unwrap())
                .unwrap();
        assert_eq!(scores["facts_hit"], 1);
        assert_eq!(scores["turns"][0]["judge"]["score"], 70);
        assert!(std::fs::read_to_string(dir.path().join("answer.txt"))
            .unwrap()
            .contains("@s1"));
        assert!(std::fs::read_to_string(dir.path().join("answer_shown.txt"))
            .unwrap()
            .contains("@Антон"));
        let timings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("timings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(timings["turns"][0]["model_steps"], 3);
        assert_eq!(
            timings["turns"][0]["calls_per_step"],
            serde_json::json!([1, 0])
        );
        let saved =
            stats_from_file(&std::fs::read_to_string(dir.path().join("stats.json")).unwrap());
        assert_eq!(saved.model_calls, 2);
        assert_eq!(saved.prompt_tokens, 11);
    }

    #[test]
    fn compare_runs_writes_the_table_into_the_second_run() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        save_summary(
            a.path(),
            &summary(vec![row(
                "a",
                "L1",
                "fact",
                1,
                1,
                0.0,
                Some(1.0),
                None,
                2.0,
                10.0,
            )]),
        )
        .unwrap();
        save_summary(
            b.path(),
            &summary(vec![row(
                "a",
                "L1",
                "fact",
                0,
                1,
                1.0,
                Some(0.0),
                None,
                2.0,
                10.0,
            )]),
        )
        .unwrap();
        let path = compare_runs(a.path(), b.path()).unwrap();
        assert_eq!(path, b.path().join("compare.md"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("fact recall"), "{text}");
    }

    #[test]
    fn a_case_with_no_intervals_shows_a_dash_and_is_not_counted() {
        let text = report_markdown(&summary(vec![
            row(
                "author-name",
                "L1",
                "fact",
                1,
                1,
                0.0,
                Some(0.5),
                None,
                2.0,
                10.0,
            ),
            row(
                "years-programmer",
                "L1",
                "negative",
                1,
                1,
                0.0,
                None,
                None,
                3.0,
                20.0,
            ),
            row("taxes", "L1", "negative", 0, 1, 0.0, None, None, 5.0, 30.0),
        ]));
        assert!(
            text.contains(
                "| years-programmer | L1 | negative | 100% (1/1) | 0 | — | — | 3 | 20s |"
            ),
            "{text}"
        );
        assert!(
            text.contains("| taxes | L1 | negative | 0% (0/1) | 0 | — | — | 5 | 30s |"),
            "{text}"
        );
        assert!(
            text.contains("| negative | 2 | 50% (1/2) | 0 | — | — | 4 | 25s |"),
            "{text}"
        );
        assert!(
            text.contains("| all | 3 | 67% (2/3) | 0 | 50% | — | 10 | 1m00s |"),
            "{text}"
        );
        let before = summary(vec![row(
            "taxes", "L1", "negative", 1, 1, 0.0, None, None, 1.0, 10.0,
        )]);
        let mut after = summary(vec![row(
            "taxes", "L1", "negative", 0, 1, 0.0, None, None, 1.0, 10.0,
        )]);
        after.label = "after".into();
        let compared = compare_markdown(Path::new("a"), &before, Path::new("b"), &after);
        assert!(
            compared.contains("## Worse\n\n- **taxes** — fact recall\n"),
            "{compared}"
        );
    }

    fn stats_from_file(text: &str) -> StatsFile {
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        StatsFile {
            model_calls: v["model_calls"].as_u64().unwrap(),
            prompt_tokens: v["prompt_tokens"].as_u64().unwrap(),
            completion_tokens: v["completion_tokens"].as_u64().unwrap(),
            model_ms: v["model_ms"].as_u64().unwrap(),
            models: vec![],
            turns: vec![],
        }
    }
}
