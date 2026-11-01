use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub struct CrateRule {
    pub name: &'static str,
    pub dir: &'static str,
    pub normal: &'static [&'static str],
    pub dev: &'static [&'static str],
}

pub const RULES: &[CrateRule] = &[
    CrateRule {
        name: "core",
        dir: "crates/core",
        normal: &[],
        dev: &[],
    },
    CrateRule {
        name: "storage",
        dir: "crates/storage",
        normal: &["core"],
        dev: &[],
    },
    CrateRule {
        name: "render",
        dir: "crates/render",
        normal: &["core"],
        dev: &[],
    },
    CrateRule {
        name: "ffmpeg",
        dir: "crates/ffmpeg",
        normal: &["core"],
        dev: &[],
    },
    CrateRule {
        name: "webp",
        dir: "crates/webp",
        normal: &["core", "ffmpeg"],
        dev: &[],
    },
    CrateRule {
        name: "gif",
        dir: "crates/gif",
        normal: &["core", "ffmpeg"],
        dev: &[],
    },
    CrateRule {
        name: "media",
        dir: "crates/media",
        normal: &["core", "ffmpeg", "webp", "gif"],
        dev: &["storage", "render"],
    },
    CrateRule {
        name: "platform",
        dir: "crates/platform",
        normal: &["core"],
        dev: &["media", "storage"],
    },
    CrateRule {
        name: "engine",
        dir: "crates/engine",
        normal: &["core"],
        dev: &[],
    },
    CrateRule {
        name: "queue",
        dir: "crates/queue",
        normal: &[],
        dev: &[],
    },
    CrateRule {
        name: "agent",
        dir: "crates/agent",
        normal: &["core", "engine", "media", "queue"],
        dev: &["engine"],
    },
    CrateRule {
        name: "eval",
        dir: "crates/eval",
        normal: &["core", "engine", "agent", "queue"],
        dev: &[],
    },
    CrateRule {
        name: "app",
        dir: "crates/app",
        normal: &[
            "core", "storage", "render", "media", "ffmpeg", "platform", "engine", "agent", "queue",
        ],
        dev: &["engine"],
    },
    CrateRule {
        name: "arch",
        dir: "crates/arch",
        normal: &[],
        dev: &[],
    },
];

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn rule(name: &str) -> Option<&'static CrateRule> {
    RULES.iter().find(|rule| rule.name == name)
}

pub fn workspace_manifest() -> PathBuf {
    workspace_root().join("Cargo.toml")
}

pub fn manifest_path(rule: &CrateRule) -> PathBuf {
    workspace_root().join(rule.dir).join("Cargo.toml")
}

pub fn workspace_members() -> Vec<String> {
    let text = fs::read_to_string(workspace_manifest()).expect("reading workspace manifest");
    let value: toml::Table = toml::from_str(&text).expect("parsing workspace manifest");
    value
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(|members| members.as_array())
        .expect("workspace.members")
        .iter()
        .map(|member| member.as_str().expect("member path").to_string())
        .collect()
}

pub fn internal_name(package: &str) -> Option<String> {
    if package == "mediaa" {
        return Some("app".to_string());
    }
    package.strip_prefix("mediaa-").map(str::to_string)
}

#[derive(Default)]
pub struct CrateDeps {
    pub normal: BTreeSet<String>,
    pub dev: BTreeSet<String>,
    pub build: BTreeSet<String>,
}

pub fn read_deps(rule: &CrateRule) -> CrateDeps {
    let path = manifest_path(rule);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let value: toml::Table =
        toml::from_str(&text).unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()));
    let mut deps = CrateDeps::default();
    collect_table(&value, "dependencies", &mut deps.normal);
    collect_table(&value, "dev-dependencies", &mut deps.dev);
    collect_table(&value, "build-dependencies", &mut deps.build);
    if let Some(targets) = value.get("target").and_then(|target| target.as_table()) {
        for target in targets.values() {
            let Some(table) = target.as_table() else {
                continue;
            };
            collect_table(table, "dependencies", &mut deps.normal);
            collect_table(table, "dev-dependencies", &mut deps.dev);
            collect_table(table, "build-dependencies", &mut deps.build);
        }
    }
    deps
}

fn collect_table(value: &toml::Table, key: &str, into: &mut BTreeSet<String>) {
    let Some(table) = value.get(key).and_then(|table| table.as_table()) else {
        return;
    };
    for package in table.keys() {
        if let Some(name) = internal_name(package) {
            into.insert(name);
        }
    }
}

pub fn allowed(rule: &CrateRule, deps: &CrateDeps) -> Vec<String> {
    let mut problems = Vec::new();
    for dependency in deps.normal.iter() {
        if !rule.normal.contains(&dependency.as_str()) {
            problems.push(format!(
                "{}: лишняя обычная зависимость `mediaa-{dependency}`; разрешено: {}",
                crate_label(rule),
                list(rule.normal)
            ));
        }
    }
    for dependency in rule.normal {
        if !deps.normal.contains(*dependency) {
            problems.push(format!(
                "{}: пропала обычная зависимость `mediaa-{dependency}`; правило устарело — обнови его",
                crate_label(rule)
            ));
        }
    }
    for dependency in deps.dev.iter() {
        if !rule.dev.contains(&dependency.as_str()) {
            problems.push(format!(
                "{}: лишняя dev-зависимость `mediaa-{dependency}`; разрешено: {}",
                crate_label(rule),
                list(rule.dev)
            ));
        }
    }
    for dependency in rule.dev {
        if !deps.dev.contains(*dependency) {
            problems.push(format!(
                "{}: пропала dev-зависимость `mediaa-{dependency}`; правило устарело — обнови его",
                crate_label(rule)
            ));
        }
    }
    problems
}

pub fn crate_label(rule: &CrateRule) -> String {
    format!("mediaa-{}", rule.name)
}

fn list(items: &[&str]) -> String {
    if items.is_empty() {
        return "ничего".to_string();
    }
    items
        .iter()
        .map(|item| format!("`mediaa-{item}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone)]
pub struct FeatureUse {
    pub crate_name: String,
    pub section: String,
    pub dependency: String,
}

pub fn feature_users(feature: &str) -> Vec<FeatureUse> {
    let mut uses = Vec::new();
    for rule in RULES {
        let path = manifest_path(rule);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        let value: toml::Table = toml::from_str(&text)
            .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()));
        collect_feature(&value, "dependencies", "normal", feature, rule, &mut uses);
        collect_feature(&value, "dev-dependencies", "dev", feature, rule, &mut uses);
        collect_feature(
            &value,
            "build-dependencies",
            "build",
            feature,
            rule,
            &mut uses,
        );
        let Some(targets) = value.get("target").and_then(|target| target.as_table()) else {
            continue;
        };
        for target in targets.values() {
            let Some(table) = target.as_table() else {
                continue;
            };
            collect_feature(
                table,
                "dependencies",
                "target normal",
                feature,
                rule,
                &mut uses,
            );
            collect_feature(
                table,
                "dev-dependencies",
                "target dev",
                feature,
                rule,
                &mut uses,
            );
            collect_feature(
                table,
                "build-dependencies",
                "target build",
                feature,
                rule,
                &mut uses,
            );
        }
    }
    uses
}

fn collect_feature(
    value: &toml::Table,
    key: &str,
    section: &str,
    feature: &str,
    rule: &CrateRule,
    into: &mut Vec<FeatureUse>,
) {
    let Some(table) = value.get(key).and_then(|table| table.as_table()) else {
        return;
    };
    for (dependency, spec) in table {
        let Some(spec) = spec.as_table() else {
            continue;
        };
        let Some(features) = spec
            .get("features")
            .and_then(|features| features.as_array())
        else {
            continue;
        };
        if features.iter().any(|item| item.as_str() == Some(feature)) {
            into.push(FeatureUse {
                crate_name: rule.name.to_string(),
                section: section.to_string(),
                dependency: dependency.clone(),
            });
        }
    }
}

const TEST_FILES: [&str; 3] = ["ui_tests.rs", "tests.rs", "test_support.rs"];

pub fn source_violations(pattern: &str) -> Vec<(String, usize, String)> {
    let mut violations = Vec::new();
    let mut files = Vec::new();
    for rule in RULES {
        collect_sources(&workspace_root().join(rule.dir).join("src"), &mut files);
    }
    for path in files {
        let relative = path
            .strip_prefix(workspace_root())
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.contains("/tests/") {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            if TEST_FILES.contains(&name) {
                continue;
            }
        }
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        for (number, line) in mask_test_code(&source).iter().enumerate() {
            if line.contains(pattern) {
                violations.push((relative.clone(), number + 1, line.clone()));
            }
        }
    }
    violations
}

pub fn source_mentions(pattern: &str) -> usize {
    let mut files = Vec::new();
    for rule in RULES {
        collect_sources(&workspace_root().join(rule.dir).join("src"), &mut files);
    }
    files
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .map(|source| source.matches(pattern).count())
        .sum()
}

fn collect_sources(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, files);
            continue;
        }
        if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

pub fn mask_test_code(source: &str) -> Vec<String> {
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
            for ch in lines[end].chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            let trimmed = lines[end].trim_start();
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
