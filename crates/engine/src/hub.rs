use hf_hub::progress::{DownloadEvent, Progress, ProgressEvent, ProgressHandler};
use hf_hub::repository::RepoTreeEntry;
use mediaa_core::agent::Provider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const COMPLETE: &str = ".mediaa-complete";

pub fn split_spec(spec: &str) -> (&str, Option<&str>) {
    let spec = spec.trim().trim_matches('/');
    match spec.match_indices('/').nth(1) {
        Some((at, _)) => (&spec[..at], Some(&spec[at + 1..]).filter(|f| !f.is_empty())),
        None => (spec, None),
    }
}

pub fn model_dir(models_root: &Path, repo: &str) -> PathBuf {
    models_root.join(repo.trim().replace('/', "--"))
}

pub fn is_downloaded(models_root: &Path, repo: &str) -> bool {
    model_dir(models_root, repo).join(COMPLETE).is_file()
}

pub fn downloaded_repo(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(COMPLETE))
        .ok()
        .map(|spec| spec.trim().to_string())
        .filter(|spec| !spec.is_empty())
}

const GGUF_QUANTS: [&str; 10] = [
    "Q4_K_M", "Q4_K_S", "Q5_K_M", "Q4_0", "IQ4_XS", "Q5_K_S", "Q6_K", "Q8_0", "Q3_K_M", "BF16",
];

pub fn pick_gguf(files: &[String]) -> Option<&str> {
    let models: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .filter(|f| {
            f.to_ascii_lowercase().ends_with(".gguf") && !f.to_ascii_lowercase().contains("mmproj")
        })
        .filter(|f| !f.contains("-of-") || f.contains("-00001-of-"))
        .collect();
    GGUF_QUANTS
        .iter()
        .find_map(|q| {
            models
                .iter()
                .find(|f| f.to_ascii_uppercase().contains(q))
                .copied()
        })
        .or_else(|| models.first().copied())
}

#[derive(Debug, PartialEq)]
pub struct Selection {
    pub allow: Option<Vec<String>>,
    pub ignore: Vec<String>,
}

pub fn select_files(files: &[String]) -> Selection {
    let mut ignore = vec![".eval_results/*".to_string(), "test_wavs/*".to_string()];
    if let Some(gguf) = pick_gguf(files) {
        let allow = match gguf.find("-00001-of-") {
            Some(at) => format!("{}-*", &gguf[..at]),
            None => gguf.to_string(),
        };
        return Selection {
            allow: Some(vec![allow]),
            ignore,
        };
    }
    if files.iter().any(|f| f.ends_with(".safetensors")) {
        ignore.extend(
            [
                "*.bin",
                "*.pt",
                "*.pth",
                "*.h5",
                "*.msgpack",
                "*.ot",
                "*.onnx",
                "*.onnx_data",
                "onnx/*",
                "openvino/*",
                "coreml/*",
            ]
            .map(String::from),
        );
    }
    Selection {
        allow: None,
        ignore,
    }
}

struct Bytes {
    files: Mutex<HashMap<String, (u64, u64)>>,
    pulse: Option<mediaa_core::pulse::Pulse>,
    on_progress: Box<dyn Fn(u64, u64) + Send + Sync>,
}

impl ProgressHandler for Bytes {
    fn on_progress(&self, event: &ProgressEvent) {
        if let Some(pulse) = &self.pulse {
            pulse.beat();
        }
        let ProgressEvent::Download(event) = event else {
            return;
        };
        let (done, total) = match event {
            DownloadEvent::Progress { files } => {
                let mut map = self.files.lock().expect("progress lock");
                for f in files {
                    map.insert(f.filename.clone(), (f.bytes_completed, f.total_bytes));
                }
                map.values()
                    .fold((0, 0), |(d, t), (fd, ft)| (d + fd, t + ft))
            }
            DownloadEvent::AggregateProgress {
                bytes_completed,
                total_bytes,
                ..
            } => (*bytes_completed, *total_bytes),
            _ => return,
        };
        (self.on_progress)(done, total);
    }
}

pub fn download(
    models_root: &Path,
    repo: &str,
    hf: Option<&Provider>,
    on_progress: impl Fn(u64, u64) + Send + Sync + 'static,
) -> anyhow::Result<PathBuf> {
    download_into(model_dir(models_root, repo), repo, hf, on_progress)
}

fn download_into(
    dir: PathBuf,
    repo: &str,
    hf: Option<&Provider>,
    on_progress: impl Fn(u64, u64) + Send + Sync + 'static,
) -> anyhow::Result<PathBuf> {
    static DOWNLOADS: Mutex<()> = Mutex::new(());
    let _one = DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner());
    if dir.join(COMPLETE).is_file() {
        return Ok(dir);
    }
    let (repo_id, file) = split_spec(repo);
    let (owner, name) = hf_hub::split_id(repo_id);
    anyhow::ensure!(
        !owner.is_empty() && !name.is_empty(),
        "\"{repo}\" is not owner/name"
    );
    let mut builder = hf_hub::HFClient::builder();
    if let Some(token) = hf.map(|p| p.api_key.trim()).filter(|t| !t.is_empty()) {
        builder = builder.token(token);
    }
    let proxy = hf.and_then(Provider::proxy);
    let http = reqwest::Client::builder().user_agent(concat!("mediaa/", env!("CARGO_PKG_VERSION")));
    let http = match proxy {
        Some(url) => http
            .proxy(reqwest::Proxy::all(url).map_err(|e| anyhow::anyhow!("proxy \"{url}\": {e}"))?),
        None => http.no_proxy(),
    };
    builder = builder.client(http.build()?);
    for var in ["HTTPS_PROXY", "HTTP_PROXY"] {
        match proxy {
            Some(url) => std::env::set_var(var, url),
            None => std::env::remove_var(var),
        }
    }
    let client = builder.build_sync()?;
    let model = client.model(owner, name);
    let progress = Progress::new(Bytes {
        files: Mutex::default(),
        pulse: mediaa_core::pulse::current(),
        on_progress: Box::new(on_progress),
    });
    if let Some(file) = file {
        model
            .snapshot_download()
            .maybe_allow_patterns(Some(vec![file.to_string()]))
            .local_dir(dir.clone())
            .progress(progress)
            .send()?;
        anyhow::ensure!(dir.join(file).is_file(), "{repo_id} has no file {file}");
        std::fs::write(dir.join(COMPLETE), repo)?;
        return Ok(dir);
    }
    let files: Vec<String> = model
        .list_tree()
        .recursive(true)
        .send()?
        .into_iter()
        .filter_map(|e| match e {
            RepoTreeEntry::File { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    anyhow::ensure!(!files.is_empty(), "{repo} has no files");
    let pick = select_files(&files);
    model
        .snapshot_download()
        .maybe_allow_patterns(pick.allow)
        .ignore_patterns(pick.ignore)
        .local_dir(dir.clone())
        .progress(progress)
        .send()?;
    std::fs::write(dir.join(COMPLETE), repo)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_weights_format_per_repository() {
        let list = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let st = select_files(&list(&[
            "config.json",
            "model.safetensors",
            "pytorch_model.bin",
            "onnx/model.onnx",
        ]));
        assert!(
            st.allow.is_none()
                && st.ignore.contains(&"*.bin".to_string())
                && st.ignore.contains(&"onnx/*".to_string())
        );
        let onnx = select_files(&list(&["v2_ctc.onnx", "v2_vocab.txt"]));
        assert_eq!(
            onnx,
            Selection {
                allow: None,
                ignore: vec![".eval_results/*".into(), "test_wavs/*".into()]
            }
        );
        let gguf = list(&[
            "Qwen3.5-4B-Q8_0.gguf",
            "Qwen3.5-4B-Q4_K_M.gguf",
            "Qwen3.5-4B-UD-Q2_K_XL.gguf",
            "mmproj-F16.gguf",
            "README.md",
        ]);
        assert_eq!(
            select_files(&gguf).allow,
            Some(vec!["Qwen3.5-4B-Q4_K_M.gguf".into()])
        );
        assert_eq!(
            pick_gguf(&list(&["mmproj-F16.gguf", "m-Q8_0.gguf"])),
            Some("m-Q8_0.gguf")
        );
        assert_eq!(pick_gguf(&list(&["mmproj-F16.gguf"])), None);
        assert_eq!(
            pick_gguf(&list(&["weird-name.gguf"])),
            Some("weird-name.gguf"),
            "any GGUF rather than none"
        );
        let split = list(&[
            "big-Q4_K_M-00001-of-00002.gguf",
            "big-Q4_K_M-00002-of-00002.gguf",
        ]);
        assert_eq!(
            select_files(&split).allow,
            Some(vec!["big-Q4_K_M-*".into()])
        );
    }

    #[test]
    #[ignore]
    fn downloads_a_tiny_model() {
        let dir = tempfile::tempdir().unwrap();
        let repo = "hf-internal-testing/tiny-random-bert";
        let seen = std::sync::Arc::new(Mutex::new(0u64));
        let s = seen.clone();
        let path = download(dir.path(), repo, None, move |done, _| {
            *s.lock().unwrap() = done
        })
        .unwrap();
        assert!(is_downloaded(dir.path(), repo));
        let (files, _) = crate::local::check(&path).unwrap();
        assert!(
            files
                .iter()
                .all(|f| f.extension().unwrap() == "safetensors"),
            "one format only: {files:?}"
        );
        assert!(*seen.lock().unwrap() > 0, "progress reported");
    }

    #[test]
    fn a_repository_or_one_file_of_it() {
        assert_eq!(split_spec(" owner/name "), ("owner/name", None));
        assert_eq!(
            split_spec("csukuangfj/vad/silero_vad.onnx"),
            ("csukuangfj/vad", Some("silero_vad.onnx"))
        );
        assert_eq!(split_spec("a/b/dir/m.onnx/"), ("a/b", Some("dir/m.onnx")));
        assert_eq!(
            model_dir(Path::new("/ai"), "a/b/m.onnx"),
            Path::new("/ai/a--b--m.onnx"),
            "a folder of its own"
        );
    }

    #[test]
    fn model_folders() {
        let root = Path::new("/m/models");
        assert_eq!(
            model_dir(root, " Qwen/Qwen3-0.6B "),
            Path::new("/m/models/Qwen--Qwen3-0.6B")
        );
        assert_eq!(
            model_dir(root, "a/b/file.gguf"),
            Path::new("/m/models/a--b--file.gguf")
        );
        assert!(!is_downloaded(Path::new("/nope"), "a/b"));
        assert_eq!(downloaded_repo(Path::new("/nope")), None);
    }
}
