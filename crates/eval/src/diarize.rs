use anyhow::Context;
use mediaa_agent::diarize_quality::{self, Der};
use mediaa_core::agent::AgentSettings;
use mediaa_engine::{Engine, LocalEngine};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub data: PathBuf,
    pub models: Option<PathBuf>,
}

pub fn parse(args: &[String]) -> anyhow::Result<Args> {
    let mut data = None;
    let mut models = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--data" => {
                i += 1;
                data = Some(PathBuf::from(args.get(i).with_context(|| {
                    format!("--data needs a value\n{}", crate::USAGE)
                })?));
            }
            "--models" => {
                i += 1;
                models = Some(PathBuf::from(args.get(i).with_context(|| {
                    format!("--models needs a value\n{}", crate::USAGE)
                })?));
            }
            other => anyhow::bail!("unknown flag {other}\n{}", crate::USAGE),
        }
        i += 1;
    }
    Ok(Args {
        data: data.context("--data is required")?,
        models,
    })
}

pub fn execute(args: &Args) -> anyhow::Result<()> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default()).try_init();
    let root = crate::repo_root();
    let data = crate::locate(&root, &args.data);
    anyhow::ensure!(data.is_dir(), "data folder not found: {}", data.display());
    let settings = AgentSettings::default();
    let models = args
        .models
        .as_ref()
        .map_or_else(mediaa_core::paths::models_dir, |path| {
            crate::locate(&root, path)
        });
    let engine: Arc<dyn Engine> =
        Arc::new(LocalEngine::new(models, settings.budget.clone().into()));
    let reports = diarize_quality::measure_all(engine.as_ref(), &settings, &data)?;
    for report in &reports {
        println!("{report}");
    }
    let scored: Vec<Der> = reports
        .iter()
        .filter_map(|report| report.der)
        .filter(|der| der.reference > 0.0)
        .collect();
    if scored.is_empty() {
        println!("\n{} recordings, no timed reference for DER", reports.len());
    } else {
        let mean = scored.iter().map(|der| der.score).sum::<f64>() / scored.len() as f64;
        println!(
            "\n{} recordings, mean DER {:.1}%",
            reports.len(),
            mean * 100.0
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_required_and_flags_parse() {
        assert_eq!(
            parse(&["--data".into(), "evals/data".into()]).unwrap(),
            Args {
                data: PathBuf::from("evals/data"),
                models: None
            }
        );
        assert_eq!(
            parse(&["--models".into(), "m".into(), "--data".into(), "d".into()]).unwrap(),
            Args {
                data: PathBuf::from("d"),
                models: Some(PathBuf::from("m"))
            }
        );
        assert!(parse(&[]).unwrap_err().to_string().contains("--data"));
        assert!(parse(&["--data".into()]).is_err());
        assert!(parse(&["--wat".into()]).is_err());
    }
}
