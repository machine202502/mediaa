use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap()
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates = root.join("crates");
    for entry in fs::read_dir(crates).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "platform" {
            continue;
        }
        let src = entry.path().join("src");
        collect(&src, &mut files);
    }
    files
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}

fn without_tests(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("#[cfg(test)]") {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn no_os_specific_code_outside_platform() {
    let root = workspace_root();
    let forbidden = [
        "cfg(windows",
        "cfg(target_os",
        "cfg(any(windows",
        "windows_sys",
        "windows_capture",
        "xcap",
        "cpal",
        "x11rb",
        "objc2",
        "mediaa_platform::wgc",
        "mediaa_platform::x11",
        "mediaa_platform::stream",
        "CpalSource",
        "CpalTarget",
    ];
    let mut failures = Vec::new();
    for path in source_files(&root) {
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains("tests"))
        {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let text = without_tests(&text);
        for (index, line) in text.lines().enumerate() {
            for token in forbidden {
                if line.contains(token) {
                    failures.push(format!(
                        "{}:{}: {token} in `{}`",
                        path.display(),
                        index + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "OS-specific code escaped mediaa-platform:\n{}",
        failures.join("\n")
    );
}

#[test]
fn media_does_not_depend_on_platform() {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("crates/media/Cargo.toml")).unwrap();
    assert!(
        !manifest.contains("mediaa-platform"),
        "mediaa-media must stay independent of mediaa-platform"
    );
}
