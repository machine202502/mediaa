use std::path::{Path, PathBuf};

const TEST_FILES: [&str; 3] = ["ui_tests.rs", "tests.rs", "test_support.rs"];

const LAYER_RULES: [(&str, &[&str]); 3] = [
    ("logic", &["crate::ui", "crate::shell", "egui", "eframe"]),
    ("adapters", &["crate::ui", "crate::logic", "egui", "eframe"]),
    (
        "ui",
        &[
            "crate::shell",
            "rfd::",
            "std::fs::",
            "std::thread::",
            "std::process::",
            "mediaa_platform::files::",
            "mediaa_platform::clipboard::",
        ],
    ),
];

const LEGACY: [(&str, &str, &str); 0] = [];

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

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn layer_of(relative: &str) -> Option<&str> {
    let first = relative.split('/').next()?;
    LAYER_RULES
        .iter()
        .map(|(layer, _)| *layer)
        .find(|layer| *layer == first)
}

fn mask_test_code(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut masked: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].trim() != "#[cfg(test)]" {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        let mut depth = 0_i32;
        let mut opened = false;
        while end < lines.len() {
            let line = lines[end];
            for ch in line.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            let trimmed = line.trim_start();
            if !opened && depth < 0 {
                break;
            }
            if !opened && trimmed.ends_with(';') {
                break;
            }
            if opened && depth <= 0 {
                break;
            }
            if !opened && end - index > 200 {
                break;
            }
            end += 1;
        }
        for line in masked
            .iter_mut()
            .take((end + 1).min(lines.len()))
            .skip(index)
        {
            line.clear();
        }
        index = end + 1;
    }
    masked
}

#[test]
fn layers_have_no_forbidden_dependencies() {
    let root = src_root();
    let mut files = Vec::new();
    collect(&root, &mut files);
    assert!(
        !files.is_empty(),
        "проверка слоёв не нашла исходников в {}",
        root.display()
    );

    let mut violations = Vec::new();
    let mut legacy_hits = [false; LEGACY.len()];
    for path in &files {
        let relative = relative(&root, path);
        let Some(layer) = layer_of(&relative) else {
            continue;
        };
        let source = std::fs::read_to_string(path).expect("reading layer source");
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let is_test_file = TEST_FILES.contains(&name.as_ref());
        let full_source = layer == "logic" || layer == "adapters";
        let lines: Vec<(usize, String)> = if full_source {
            source
                .lines()
                .enumerate()
                .map(|(number, line)| (number, line.to_string()))
                .collect()
        } else {
            if is_test_file {
                continue;
            }
            mask_test_code(&source).into_iter().enumerate().collect()
        };
        for (number, line) in lines {
            for (_, patterns) in LAYER_RULES.iter().filter(|(name, _)| *name == layer) {
                for pattern in *patterns {
                    if !line.contains(pattern) {
                        continue;
                    }
                    let mut allowed = false;
                    for (index, (legacy_pattern, legacy_file, _)) in LEGACY.iter().enumerate() {
                        if legacy_pattern == pattern && relative.ends_with(legacy_file) {
                            legacy_hits[index] = true;
                            allowed = true;
                        }
                    }
                    if !allowed {
                        violations.push(format!(
                            "{relative}:{}: {pattern} в слое {layer}",
                            number + 1
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "нарушения слоёв (перенеси код в правильный слой или оформи исключение с этапом):\n{}",
        violations.join("\n")
    );

    for (index, (pattern, file, stage)) in LEGACY.iter().enumerate() {
        assert!(
            legacy_hits[index],
            "исключение {pattern} для {file} устарело: удали его ({stage})"
        );
    }
}
