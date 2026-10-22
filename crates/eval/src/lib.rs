//! Quality runs of the MediaA agent: ask as the app does, score without a model, write a report.

mod cases;
mod report;
mod run;
mod score;

use anyhow::Context;
use std::path::{Path, PathBuf};

/// Who scores an answer besides the mechanical checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judge {
    /// Mechanical checks only.
    None,
    /// The agent's own local model.
    Agent,
    /// An engine model spec (`owner/repo` or a path).
    Spec(String),
}

impl Judge {
    pub fn as_str(&self) -> &str {
        match self {
            Judge::None => "none",
            Judge::Agent => "agent",
            Judge::Spec(spec) => spec,
        }
    }
}

/// `mediaa-eval run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub cases: PathBuf,
    pub library: PathBuf,
    pub file: PathBuf,
    pub models: PathBuf,
    pub repeat: usize,
    pub judge: Judge,
    pub case: Option<String>,
    pub label: Option<String>,
}

const USAGE: &str = "\
usage:
  mediaa-eval run --cases FILE --library DIR --file PATH --models DIR [--repeat N] [--judge none|agent|<model>] [--case ID] [--label TEXT]
  mediaa-eval compare <run A> <run B>";

/// The workspace root: the directory that has `Cargo.toml` and `crates/`.
pub fn repo_root() -> PathBuf {
    let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = start.clone();
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return dir;
        }
        if !dir.pop() {
            return start;
        }
    }
}

/// An absolute path, or a path that already exists, stays. Anything else is under `root`.
pub fn locate(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() || path.exists() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn flag_value(args: &[String], i: &mut usize) -> anyhow::Result<String> {
    let flag = args[*i].as_str();
    *i += 1;
    args.get(*i)
        .cloned()
        .with_context(|| format!("{flag} needs a value"))
}

pub fn parse_run(args: &[String]) -> anyhow::Result<RunArgs> {
    let mut cases = None;
    let mut library = None;
    let mut file = None;
    let mut models = None;
    let mut repeat = 1usize;
    let mut judge = Judge::None;
    let mut case = None;
    let mut label = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cases" => cases = Some(PathBuf::from(flag_value(args, &mut i)?)),
            "--library" => library = Some(PathBuf::from(flag_value(args, &mut i)?)),
            "--file" => file = Some(PathBuf::from(flag_value(args, &mut i)?)),
            "--models" => models = Some(PathBuf::from(flag_value(args, &mut i)?)),
            "--repeat" => {
                let text = flag_value(args, &mut i)?;
                let n: usize = text
                    .parse()
                    .with_context(|| format!("--repeat {text:?} is not a number"))?;
                anyhow::ensure!(n >= 1, "--repeat must be at least 1");
                repeat = n;
            }
            "--judge" => {
                let text = flag_value(args, &mut i)?;
                judge = match text.to_ascii_lowercase().as_str() {
                    "none" => Judge::None,
                    "agent" => Judge::Agent,
                    _ => Judge::Spec(text),
                };
            }
            "--case" => case = Some(flag_value(args, &mut i)?),
            "--label" => label = Some(flag_value(args, &mut i)?),
            other => anyhow::bail!("unknown flag {other}\n{USAGE}"),
        }
        i += 1;
    }
    Ok(RunArgs {
        cases: cases.context("--cases is required")?,
        library: library.context("--library is required")?,
        file: file.context("--file is required")?,
        models: models.context("--models is required")?,
        repeat,
        judge,
        case,
        label,
    })
}

pub fn main_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<()> {
    let args: Vec<String> = args.into_iter().collect();
    match args.first().map(String::as_str) {
        Some("run") => {
            let parsed = parse_run(&args[1..])?;
            let dir = run::execute(&parsed)?;
            println!("{}", dir.display());
            Ok(())
        }
        Some("compare") => {
            anyhow::ensure!(
                args.len() == 3,
                "compare needs two run directories\n{USAGE}"
            );
            let path = report::compare_runs(Path::new(&args[1]), Path::new(&args[2]))?;
            println!("{}", path.display());
            Ok(())
        }
        _ => anyhow::bail!("{USAGE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_flags(extra: &[&str]) -> Vec<String> {
        let mut args = vec![
            "--cases".into(),
            "cases.yaml".into(),
            "--library".into(),
            "lib".into(),
            "--file".into(),
            "rec.mp4".into(),
            "--models".into(),
            "models".into(),
        ];
        args.extend(extra.iter().map(|s| (*s).to_string()));
        args
    }

    #[test]
    fn flags_parse_repeat_judge_case_and_label() {
        let args = parse_run(&run_flags(&[
            "--repeat", "2", "--judge", "AGENT", "--case", "taxes", "--label", "before",
        ]))
        .unwrap();
        assert_eq!(args.repeat, 2);
        assert!(matches!(args.judge, Judge::Agent));
        assert_eq!(args.case.as_deref(), Some("taxes"));
        assert_eq!(args.label.as_deref(), Some("before"));
        assert_eq!(args.judge.as_str(), "agent");
        let spec = parse_run(&run_flags(&["--judge", "unsloth/Qwen3.5-4B-GGUF"])).unwrap();
        assert!(matches!(spec.judge, Judge::Spec(s) if s == "unsloth/Qwen3.5-4B-GGUF"));
        let defaults = parse_run(&run_flags(&[])).unwrap();
        assert_eq!(defaults.repeat, 1);
        assert!(matches!(defaults.judge, Judge::None));
        assert!(parse_run(&["--cases".into(), "cases.yaml".into()]).is_err());
        assert!(parse_run(&run_flags(&["--repeat", "0"])).is_err());
    }

    #[test]
    fn compare_needs_two_runs_that_have_summaries() {
        let missing = main_args(["compare".into(), "no-such-run".into()]).unwrap_err();
        assert!(format!("{missing:#}").contains("two"), "{missing:#}");
        let absent =
            main_args(["compare".into(), "no-such-a".into(), "no-such-b".into()]).unwrap_err();
        assert!(format!("{absent:#}").contains("summary"), "{absent:#}");
        assert!(main_args(std::iter::empty::<String>())
            .unwrap_err()
            .to_string()
            .contains("usage"));
    }

    #[test]
    fn the_repo_root_holds_this_crate_and_missing_paths_land_there() {
        let root = repo_root();
        assert!(
            root.join("crates").join("eval").is_dir(),
            "{}",
            root.display()
        );
        let missing = Path::new("does-not-exist-eval.yaml");
        assert_eq!(locate(&root, missing), root.join(missing));
        let here = std::env::temp_dir();
        assert_eq!(locate(&root, &here), here);
    }
}
