//! What the engine knows about models without running them: the format of a model from its file
//! names, the roles it fits, the bytes a download takes, and the Hugging Face repository listing.
//!
//! Format detection and role fitness are pure functions over file names (unit-tested); only
//! `fetch_remote_files` touches the network.

use crate::engine::{EngineError, ModelFile, ModelFormat, ModelInspect};
use crate::hub;
use mediaa_core::agent::ModelRole;
use serde::Deserialize;
use std::time::Duration;

/// The API name of a role (its `ModelRole` name in snake_case).
pub fn role_name(role: ModelRole) -> &'static str {
    match role {
        ModelRole::Embedding => "embedding",
        ModelRole::Reranker => "reranker",
        ModelRole::Agent => "agent",
        ModelRole::Titles => "titles",
        ModelRole::SpeechRecognition => "speech_recognition",
        ModelRole::Diarization => "diarization",
        ModelRole::Speech => "speech",
    }
}

pub fn role_from_name(name: &str) -> Option<ModelRole> {
    ModelRole::ALL.iter().copied().find(|role| role_name(*role) == name.trim())
}

fn is_gguf(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".gguf") && !lower.contains("mmproj")
}

fn is_onnx(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".onnx")
}

fn is_tokens(name: &str) -> bool {
    name.rsplit(['/', '\\']).next().is_some_and(|file| file.eq_ignore_ascii_case("tokens.txt"))
}

/// The format of a model from its file names.
pub fn detect_format(files: &[ModelFile]) -> ModelFormat {
    if files.iter().any(|file| is_gguf(&file.name)) {
        ModelFormat::Gguf
    } else if files.iter().any(|file| is_tokens(&file.name)) && files.iter().any(|file| is_onnx(&file.name)) {
        ModelFormat::SherpaOnnx
    } else if files.iter().any(|file| is_onnx(&file.name)) {
        ModelFormat::Onnx
    } else {
        ModelFormat::Other
    }
}

/// For which roles the model fits, by its format, and why the other roles do not fit it.
pub fn role_fitness(files: &[ModelFile]) -> (Vec<String>, Vec<String>) {
    let format = detect_format(files);
    let mut roles: Vec<String> = Vec::new();
    let mut problems = Vec::new();
    match format {
        ModelFormat::Gguf => roles.extend(["agent", "titles", "embedding", "reranker"].map(str::to_string)),
        ModelFormat::SherpaOnnx => roles.push("speech_recognition".to_string()),
        ModelFormat::Onnx => roles.push("diarization".to_string()),
        ModelFormat::Other => {}
    }
    if format != ModelFormat::Gguf {
        problems.push("no GGUF file: llama.cpp roles need one".into());
    }
    if format != ModelFormat::SherpaOnnx {
        problems.push("no tokens.txt with .onnx files: speech recognition needs a sherpa-onnx model".into());
    }
    if format != ModelFormat::Onnx {
        problems.push("not a speaker embedding model: diarization needs a single .onnx model without tokens.txt".into());
    }
    problems.push("speech synthesis models are not supported yet".into());
    (roles, problems)
}

/// `pattern` with `*` standing for any run of characters (the globs of `hub::select_files`).
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, n));
            p += 1;
        } else if p < pattern.len() && pattern[p] == name[n] {
            p += 1;
            n += 1;
        } else if let Some((star_p, star_n)) = star {
            p = star_p + 1;
            n = star_n + 1;
            // Let the star consume one more character, or the next mismatch would retry the same one.
            star = Some((star_p, n));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// Bytes a download of these files takes: one weights format per repository, as
/// `hub::select_files` chooses it.
fn selected_bytes(files: &[ModelFile]) -> u64 {
    let names: Vec<String> = files.iter().map(|file| file.name.clone()).collect();
    let selection = hub::select_files(&names);
    files
        .iter()
        .filter(|file| selection.allow.as_ref().is_none_or(|allow| allow.iter().any(|pattern| glob_match(pattern, &file.name))))
        .filter(|file| !selection.ignore.iter().any(|pattern| glob_match(pattern, &file.name)))
        .map(|file| file.bytes)
        .sum()
}

/// The inspection answer for a list of files; `only_selected`: count only what a pull will
/// download (a repository), rather than the size of the files given (one requested file).
pub fn inspect_files(spec: &str, mut files: Vec<ModelFile>, only_selected: bool) -> ModelInspect {
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let format = detect_format(&files);
    let (roles, problems) = role_fitness(&files);
    let download_bytes = if only_selected { selected_bytes(&files) } else { files.iter().map(|file| file.bytes).sum() };
    ModelInspect { spec: spec.to_string(), files, format, download_bytes, roles, problems }
}

#[derive(Deserialize)]
struct RepoInfo {
    #[serde(default)]
    siblings: Vec<Sibling>,
}

#[derive(Deserialize)]
struct Sibling {
    rfilename: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    lfs: Option<LfsSize>,
}

#[derive(Deserialize)]
struct LfsSize {
    #[serde(default)]
    size: u64,
}

/// Files of a Hugging Face repository with their sizes, from the API (no model is downloaded).
pub fn fetch_remote_files(repo: &str, hf_token: Option<&str>) -> Result<Vec<ModelFile>, EngineError> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("mediaa/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| EngineError::internal(format!("the HTTP client: {e}")))?;
    let url = format!("https://huggingface.co/api/models/{repo}?blobs=true");
    let mut request = client.get(&url);
    if let Some(token) = hf_token.map(str::trim).filter(|token| !token.is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(|e| EngineError::internal(format!("Hugging Face: {e}")))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(EngineError::not_found(format!("{repo} not found on Hugging Face")));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(EngineError::bad_request(format!("Hugging Face refused {repo} ({status}): the model may be gated; set a Hugging Face token")));
    }
    if !status.is_success() {
        return Err(EngineError::internal(format!("Hugging Face answered {status} for {repo}")));
    }
    let info: RepoInfo = response.json().map_err(|e| EngineError::internal(format!("Hugging Face answered not JSON: {e}")))?;
    Ok(info
        .siblings
        .into_iter()
        .map(|sibling| ModelFile { bytes: sibling.size.max(sibling.lfs.map_or(0, |lfs| lfs.size)), name: sibling.rfilename })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[(&str, u64)]) -> Vec<ModelFile> {
        names.iter().map(|(name, bytes)| ModelFile { name: (*name).into(), bytes: *bytes }).collect()
    }

    #[test]
    fn formats_are_recognised_from_file_names() {
        let gguf = files(&[("README.md", 10), ("model-Q4_K_M.gguf", 100)]);
        assert_eq!(detect_format(&gguf), ModelFormat::Gguf);
        let sherpa = files(&[("tokens.txt", 1), ("model.int8.onnx", 100)]);
        assert_eq!(detect_format(&sherpa), ModelFormat::SherpaOnnx);
        let onnx = files(&[("wespeaker.onnx", 100)]);
        assert_eq!(detect_format(&onnx), ModelFormat::Onnx);
        let other = files(&[("model.safetensors", 100), ("config.json", 1)]);
        assert_eq!(detect_format(&other), ModelFormat::Other);
        assert_eq!(detect_format(&files(&[("mmproj-F16.gguf", 100)])), ModelFormat::Other, "a vision projector is no model");
        assert_eq!(detect_format(&[]), ModelFormat::Other);
    }

    #[test]
    fn roles_follow_the_format_and_problems_say_why_not() {
        let (roles, problems) = role_fitness(&files(&[("model-Q4_K_M.gguf", 100)]));
        assert_eq!(roles, ["agent", "titles", "embedding", "reranker"]);
        assert!(!problems.iter().any(|problem| problem.starts_with("no GGUF")), "llama.cpp roles fit");
        assert!(problems.iter().any(|problem| problem.contains("speech recognition")));

        let (roles, problems) = role_fitness(&files(&[("tokens.txt", 1), ("model.onnx", 100)]));
        assert_eq!(roles, ["speech_recognition"]);
        assert!(problems.iter().any(|problem| problem == "no GGUF file: llama.cpp roles need one"));

        let (roles, _) = role_fitness(&files(&[("speaker.onnx", 100)]));
        assert_eq!(roles, ["diarization"]);

        let (roles, problems) = role_fitness(&files(&[("model.safetensors", 100)]));
        assert!(roles.is_empty());
        assert!(problems.iter().any(|problem| problem.contains("llama.cpp")));
        assert!(problems.iter().any(|problem| problem.contains("sherpa-onnx")));
        assert!(problems.iter().any(|problem| problem.contains("speaker embedding")));
    }

    #[test]
    fn roles_round_trip_their_names() {
        for role in ModelRole::ALL {
            assert_eq!(role_from_name(role_name(role)), Some(role));
        }
        assert_eq!(role_from_name(" speech_recognition "), Some(ModelRole::SpeechRecognition));
        assert_eq!(role_from_name("chat"), None);
    }

    #[test]
    fn inspect_counts_one_weights_format() {
        let repository = files(&[
            ("README.md", 10),
            ("model-Q8_0.gguf", 800),
            ("model-Q4_K_M.gguf", 400),
            ("test_wavs/a.wav", 50),
        ]);
        let response = inspect_files("unsloth/model-GGUF", repository.clone(), true);
        assert_eq!(response.format, ModelFormat::Gguf);
        assert_eq!(response.download_bytes, 400, "one quantisation, no samples");
        assert_eq!(response.files.len(), 4);
        let single = inspect_files("unsloth/model-GGUF/model-Q8_0.gguf", files(&[("model-Q8_0.gguf", 800)]), false);
        assert_eq!(single.download_bytes, 800);
    }

    #[test]
    fn globs_match_the_selection_patterns() {
        assert!(glob_match("*.bin", "pytorch_model.bin"));
        assert!(glob_match("onnx/*", "onnx/model.onnx"));
        assert!(glob_match("big-Q4_K_M-*", "big-Q4_K_M-00001-of-00002.gguf"));
        assert!(glob_match("test_wavs/*", "test_wavs/a.wav"));
        assert!(!glob_match("*.bin", "model.safetensors"));
        assert!(!glob_match("onnx/*", "onnx_data/x"));
        assert!(glob_match("*", "anything/at/all"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        // Backtracking must advance, or a failing star loops forever.
        assert!(!glob_match("*a", "bbb"));
        assert!(glob_match("*a", "bbba"));
    }
}
