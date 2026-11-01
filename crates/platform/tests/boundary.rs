use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap()
}

fn collect_rs(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates = root.join("crates");
    for entry in fs::read_dir(crates).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "platform" {
            continue;
        }
        collect_rs(&entry.path().join("src"), &mut files);
    }
    files
}

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().contains("tests"))
}

fn starts_block_item(item: &str) -> bool {
    let mut rest = item.trim_start();
    loop {
        let next = [
            "pub(crate) ",
            "pub(super) ",
            "pub(in ",
            "pub ",
            "unsafe ",
            "async ",
            "const ",
            "extern \"C\" ",
        ]
        .into_iter()
        .find_map(|prefix| rest.strip_prefix(prefix));
        match next {
            Some(next) => rest = next,
            None => break,
        }
    }
    [
        "mod ", "fn ", "struct ", "enum ", "impl ", "trait ", "type ",
    ]
    .iter()
    .any(|prefix| rest.starts_with(prefix))
}

fn without_tests(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if !line.trim_start().starts_with("#[cfg(test)]") {
            out.push_str(line);
            out.push('\n');
            index += 1;
            continue;
        }

        let mut item = index + 1;
        while item < lines.len()
            && (lines[item].trim().is_empty() || lines[item].trim_start().starts_with("#["))
        {
            item += 1;
        }
        if item >= lines.len() {
            break;
        }

        if !starts_block_item(lines[item]) {
            index = item + 1;
            continue;
        }

        let mut depth = 0i32;
        let mut opened = false;
        while item < lines.len() {
            for character in lines[item].chars() {
                match character {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            let ended_with_semicolon = !opened && lines[item].trim_end().ends_with(';');
            item += 1;
            if (opened && depth <= 0) || ended_with_semicolon {
                break;
            }
        }
        index = item;
    }
    out
}

fn forbidden_in_line(line: &str) -> Option<&'static str> {
    [
        "cfg(windows",
        "cfg(target_os",
        "cfg(unix",
        "cfg(any(windows",
        "windows_sys",
        "windows_capture",
        "xcap",
        "cpal",
        "xcb",
        "x11rb",
        "objc2",
        "std::os::",
        "RawWindowHandle::",
        "mediaa_platform::wgc",
        "mediaa_platform::x11",
        "mediaa_platform::stream",
        "CpalSource",
        "CpalTarget",
    ]
    .into_iter()
    .find(|token| line.contains(token))
}

#[test]
fn no_os_specific_code_outside_platform() {
    let root = workspace_root();
    let mut failures = Vec::new();
    for path in source_files(&root) {
        if is_test_file(&path) {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let text = without_tests(&text);
        for (index, line) in text.lines().enumerate() {
            if let Some(token) = forbidden_in_line(line) {
                failures.push(format!(
                    "{}:{}: {token} in `{}`",
                    path.display(),
                    index + 1,
                    line.trim()
                ));
            }
            if line.contains("windows_subsystem")
                && path.file_name().is_none_or(|name| name != "main.rs")
            {
                failures.push(format!(
                    "{}:{}: windows_subsystem outside the app binary",
                    path.display(),
                    index + 1
                ));
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
fn build_scripts_are_documented_exceptions() {
    let root = workspace_root();
    let allowed = [
        root.join("crates/app/build.rs"),
        root.join("crates/ffmpeg/build.rs"),
    ];
    let mut failures = Vec::new();
    for entry in fs::read_dir(root.join("crates")).unwrap() {
        let entry = entry.unwrap();
        let build = entry.path().join("build.rs");
        if !build.is_file() || allowed.contains(&build) {
            continue;
        }
        let text = fs::read_to_string(&build).unwrap();
        for (index, line) in text.lines().enumerate() {
            if ["CARGO_CFG_TARGET_OS", "std::os::", "windows_subsystem"]
                .iter()
                .any(|token| line.contains(token))
            {
                failures.push(format!(
                    "{}:{}: OS-specific build logic `{}`",
                    build.display(),
                    index + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "OS-specific build logic outside documented build scripts:\n{}",
        failures.join("\n")
    );
}

#[test]
fn manifests_do_not_grow_os_branches() {
    let root = workspace_root();
    let allowed = [
        root.join("crates/platform/Cargo.toml"),
        root.join("crates/app/Cargo.toml"),
    ];
    let mut failures = Vec::new();
    for entry in fs::read_dir(root.join("crates")).unwrap() {
        let entry = entry.unwrap();
        let manifest = entry.path().join("Cargo.toml");
        if allowed.contains(&manifest) {
            continue;
        }
        let text = fs::read_to_string(&manifest).unwrap();
        if text
            .lines()
            .any(|line| line.trim_start().starts_with("[target."))
        {
            failures.push(format!(
                "{}: target-specific dependencies outside platform/app",
                manifest.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "target-specific manifests escaped mediaa-platform:\n{}",
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
