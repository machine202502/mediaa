use crate::engine::{phrases_from_words, shift_phrases, EngineError};
use mediaa_core::agent::{CommandSpec, OutputFormat};
use mediaa_core::transcript::{Phrase, Turn, Word};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub fn expand_home(value: &str) -> String {
    let Some(rest) = value.strip_prefix('~') else {
        return value.to_string();
    };
    let home = directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    if rest.is_empty() {
        return home.display().to_string();
    }
    let rest = rest.trim_start_matches(['/', '\\']);
    home.join(rest).display().to_string()
}

pub struct Prepared {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub output: PathBuf,
    pub output_stem: PathBuf,
}

#[allow(
    clippy::too_many_arguments,
    reason = "one command invocation: program template plus the file, model and time range"
)]
pub fn prepare(
    spec: &CommandSpec,
    input: &Path,
    model: &str,
    language: &str,
    from: f64,
    to: f64,
    inputs: &[PathBuf],
    work_dir: &Path,
) -> Prepared {
    let output = work_dir.join("out.txt");
    let output_stem = work_dir.join("out");
    let mut args = Vec::new();
    for arg in &spec.args {
        if arg == "{inputs}" {
            for path in inputs {
                args.push(path.display().to_string());
            }
            continue;
        }
        let replaced = arg
            .replace("{input}", &input.display().to_string())
            .replace("{output}", &output.display().to_string())
            .replace("{output_stem}", &output_stem.display().to_string())
            .replace("{model}", model)
            .replace("{language}", language)
            .replace("{from}", &format!("{from}"))
            .replace("{to}", &format!("{to}"));
        args.push(expand_home(&replaced));
    }
    Prepared {
        program: PathBuf::from(expand_home(&spec.program.display().to_string())),
        args,
        output,
        output_stem,
    }
}

pub fn run(
    spec: &CommandSpec,
    prepared: &Prepared,
    cancel: &AtomicBool,
) -> Result<String, EngineError> {
    let mut child = Command::new(&prepared.program)
        .args(&prepared.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            EngineError::not_found(format!("{}: {error}", prepared.program.display()))
        })?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EngineError::cancelled("cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                if !status.success() {
                    let detail = if stderr.trim().is_empty() {
                        stdout.trim().to_string()
                    } else {
                        stderr.trim().to_string()
                    };
                    return Err(EngineError::internal(format!(
                        "{} exited with {}: {detail}",
                        prepared.program.display(),
                        status.code().unwrap_or(-1)
                    )));
                }
                if prepared
                    .args
                    .iter()
                    .any(|arg| arg == &prepared.output.display().to_string())
                    || spec.args.iter().any(|arg| arg.contains("{output}"))
                {
                    if let Ok(file) = std::fs::read_to_string(&prepared.output) {
                        return Ok(file);
                    }
                }
                if spec.args.iter().any(|arg| arg.contains("{output_stem}")) {
                    let json = prepared.output_stem.with_extension("json");
                    if let Ok(file) = std::fs::read_to_string(&json) {
                        return Ok(file);
                    }
                }
                return Ok(stdout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(EngineError::internal(error.to_string())),
        }
    }
}

pub fn parse_transcript(
    format: OutputFormat,
    text: &str,
    offset: f64,
) -> Result<Vec<Phrase>, EngineError> {
    let phrases = match format {
        OutputFormat::WhisperJson | OutputFormat::OpenAiJson => parse_json(text)?,
        OutputFormat::Srt => parse_cues(text, true)?,
        OutputFormat::Vtt => parse_cues(text, false)?,
        OutputFormat::Rttm | OutputFormat::Vectors => {
            return Err(EngineError::bad_request(
                "this command does not return a transcript",
            ))
        }
    };
    Ok(shift_phrases(phrases, offset))
}

pub fn parse_rttm(text: &str) -> Result<Vec<Turn>, EngineError> {
    let mut labels = Vec::new();
    let mut turns = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 || !parts[0].eq_ignore_ascii_case("SPEAKER") {
            continue;
        }
        let start: f64 = parts[3]
            .parse()
            .map_err(|_| EngineError::bad_request(format!("bad RTTM start: {line}")))?;
        let duration: f64 = parts[4]
            .parse()
            .map_err(|_| EngineError::bad_request(format!("bad RTTM duration: {line}")))?;
        let label = parts[7];
        let speaker = if let Some(index) = labels.iter().position(|known| known == label) {
            index as u32
        } else {
            labels.push(label.to_string());
            (labels.len() - 1) as u32
        };
        turns.push(Turn {
            start,
            end: start + duration,
            speaker,
        });
    }
    Ok(turns)
}

pub fn parse_vectors(text: &str, expected: usize) -> Result<Vec<Vec<f32>>, EngineError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| EngineError::bad_request(format!("vectors are not JSON: {error}")))?;
    let rows = value
        .as_array()
        .ok_or_else(|| EngineError::bad_request("vectors are not an array"))?;
    if rows.len() != expected {
        return Err(EngineError::bad_request(format!(
            "the command returned {} vectors for {expected} files",
            rows.len()
        )));
    }
    rows.iter()
        .map(|row| {
            let values = row
                .as_array()
                .ok_or_else(|| EngineError::bad_request("a vector is not an array"))?;
            Ok(unit(
                &values
                    .iter()
                    .filter_map(|value| value.as_f64().map(|number| number as f32))
                    .collect::<Vec<_>>(),
            ))
        })
        .collect()
}

fn unit(vector: &[f32]) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if norm <= 1e-8 {
        return Vec::new();
    }
    vector
        .iter()
        .map(|value| (*value as f64 / norm) as f32)
        .collect()
}

fn parse_json(text: &str) -> Result<Vec<Phrase>, EngineError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| EngineError::bad_request(format!("transcript is not JSON: {error}")))?;
    if let Some(words) = value["words"].as_array() {
        let parsed = words
            .iter()
            .filter_map(|word| {
                Some(Word {
                    text: word["word"].as_str()?.trim().to_string(),
                    start: word["start"].as_f64()?,
                    end: word["end"].as_f64()?,
                })
            })
            .filter(|word| !word.text.is_empty())
            .collect();
        return Ok(phrases_from_words(parsed));
    }
    if let Some(segments) = value["segments"].as_array() {
        let mut phrases = Vec::new();
        for segment in segments {
            let words = segment["words"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|word| {
                    Some(Word {
                        text: word["word"].as_str()?.trim().to_string(),
                        start: word["start"].as_f64()?,
                        end: word["end"].as_f64()?,
                    })
                })
                .collect::<Vec<_>>();
            if !words.is_empty() {
                phrases.extend(phrases_from_words(words));
            } else if let Some(text) = segment["text"].as_str() {
                phrases.push(Phrase {
                    start: segment["start"].as_f64().unwrap_or(0.0),
                    end: segment["end"].as_f64().unwrap_or(0.0),
                    words: vec![Word {
                        text: text.trim().to_string(),
                        start: segment["start"].as_f64().unwrap_or(0.0),
                        end: segment["end"].as_f64().unwrap_or(0.0),
                    }],
                });
            }
        }
        return Ok(phrases);
    }
    Err(EngineError::bad_request("transcript JSON has no words"))
}

fn parse_cues(text: &str, srt: bool) -> Result<Vec<Phrase>, EngineError> {
    let mut phrases = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || (!srt && (line.starts_with("WEBVTT") || line.starts_with("NOTE"))) {
            continue;
        }
        if srt && line.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let timing = if line.contains("-->") {
            line
        } else {
            continue;
        };
        let mut parts = timing.split("-->");
        let start = stamp(parts.next().unwrap_or("").trim())?;
        let end = stamp(
            parts
                .next()
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or(""),
        )?;
        let mut words = Vec::new();
        while let Some(next) = lines.peek() {
            if next.trim().is_empty() {
                break;
            }
            let text = lines.next().unwrap().trim();
            if text.contains("-->") {
                break;
            }
            if !text.is_empty() {
                words.push(text.to_string());
            }
        }
        let text = words.join(" ");
        if !text.is_empty() {
            phrases.push(Phrase {
                start,
                end,
                words: vec![Word { text, start, end }],
            });
        }
    }
    Ok(phrases)
}

fn stamp(value: &str) -> Result<f64, EngineError> {
    let value = value.trim().replace(',', ".");
    let mut parts = value.split(':').collect::<Vec<_>>();
    if parts.len() == 2 {
        parts.insert(0, "0");
    }
    if parts.len() != 3 {
        return Err(EngineError::bad_request(format!("bad timestamp {value}")));
    }
    let hours: f64 = parts[0]
        .parse()
        .map_err(|_| EngineError::bad_request(format!("bad timestamp {value}")))?;
    let minutes: f64 = parts[1]
        .parse()
        .map_err(|_| EngineError::bad_request(format!("bad timestamp {value}")))?;
    let seconds: f64 = parts[2]
        .parse()
        .map_err(|_| EngineError::bad_request(format!("bad timestamp {value}")))?;
    Ok(hours * 3600.0 + minutes * 60.0 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::CommandSpec;

    fn stub() -> PathBuf {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("command_stub.exe");
        path
    }

    fn spec(args: &[&str], output: OutputFormat) -> CommandSpec {
        CommandSpec {
            program: stub(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            output,
            whole_file: false,
        }
    }

    #[test]
    fn placeholders_and_each_output_format() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.wav");
        std::fs::write(&input, b"wav").unwrap();
        let prepared = prepare(
            &spec(
                &["-m", "{model}", "-f", "{input}", "{inputs}"],
                OutputFormat::Rttm,
            ),
            &input,
            "model.bin",
            "ru",
            1.0,
            2.0,
            &[dir.path().join("a.wav"), dir.path().join("b.wav")],
            dir.path(),
        );
        assert_eq!(
            prepared.args,
            [
                "-m",
                "model.bin",
                "-f",
                &input.display().to_string(),
                &dir.path().join("a.wav").display().to_string(),
                &dir.path().join("b.wav").display().to_string(),
            ]
        );
        let turns = parse_rttm(
            "SPEAKER f 1 0.5 1.25 <NA> <NA> bob <NA> <NA>\nSPEAKER f 1 2.0 0.5 <NA> <NA> ann <NA> <NA>\nSPEAKER f 1 3.0 0.5 <NA> <NA> bob <NA> <NA>\n",
        )
        .unwrap();
        assert_eq!(turns[0].speaker, 0);
        assert_eq!(turns[1].speaker, 1);
        assert_eq!(turns[2].speaker, 0);
        assert!((turns[0].end - 1.75).abs() < 1e-9);
        let vectors = parse_vectors("[[3.0, 0.0], [0.0, 4.0]]", 2).unwrap();
        assert!((vectors[0][0] - 1.0).abs() < 1e-6);
        assert!(parse_vectors("[[1.0]]", 2).is_err());
        let phrases = parse_transcript(
            OutputFormat::Srt,
            "1\n00:00:01,000 --> 00:00:02,000\nhello\n",
            10.0,
        )
        .unwrap();
        assert!((phrases[0].start - 11.0).abs() < 1e-6);
        let phrases = parse_transcript(
            OutputFormat::OpenAiJson,
            r#"{"words":[{"word":"hi","start":0.2,"end":0.4}]}"#,
            0.0,
        )
        .unwrap();
        assert_eq!(phrases[0].words[0].text, "hi");
    }
}
