use std::path::{Path, PathBuf};

const RULES: [(&str, &[&str]); 2] = [
    ("features", &["crate::daemon"]),
    ("infra", &["crate::daemon", "crate::features"]),
];

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        files.push(path);
    }
}

#[test]
fn backend_modules_respect_architecture_boundaries() {
    let root = src_root();
    let mut violations = Vec::new();
    for path in ["features", "infra"] {
        let mut files = Vec::new();
        collect(&root.join(path), &mut files);
        for file in files {
            let source = std::fs::read_to_string(&file).expect("reading backend source");
            let relative = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            for (layer, forbidden) in RULES.iter().filter(|(name, _)| *name == path) {
                for pattern in *forbidden {
                    if source.contains(pattern) {
                        violations.push(format!("{relative}: {pattern} в слое {layer}"));
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "нарушения слоёв backend:\n{}",
        violations.join("\n")
    );
}
