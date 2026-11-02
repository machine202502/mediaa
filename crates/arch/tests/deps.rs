use mediaa_arch::{allowed, read_deps, workspace_members, RULES};
use std::collections::BTreeSet;

#[test]
fn workspace_members_are_described() {
    let members: BTreeSet<String> = workspace_members()
        .into_iter()
        .map(|member| member.replace('\\', "/"))
        .collect();
    let described: BTreeSet<String> = RULES.iter().map(|rule| rule.dir.to_string()).collect();

    let mut problems = Vec::new();
    for member in members.difference(&described) {
        problems.push(format!(
            "{member}: крейт есть в workspace, но его нет в mediaa_arch::RULES — опиши зависимости"
        ));
    }
    for directory in described.difference(&members) {
        problems.push(format!(
            "{directory}: правило есть, а крейта нет в workspace.members — удали правило"
        ));
    }
    assert!(
        problems.is_empty(),
        "состав workspace не совпадает с правилами:\n{}",
        problems.join("\n")
    );
}

#[test]
fn crate_dependencies_match_allowlist() {
    let mut problems = Vec::new();
    for rule in RULES {
        let deps = read_deps(rule);
        problems.extend(allowed(rule, &deps));
    }
    assert!(
        problems.is_empty(),
        "зависимости не соответствуют архитектуре:\n{}\n\
         если зависимость нужна осознанно — добавь её в mediaa_arch::RULES с этапом",
        problems.join("\n")
    );
}

#[test]
fn no_crate_depends_on_app() {
    let mut problems = Vec::new();
    for rule in RULES {
        if rule.name == "app" || rule.name == "app-agent" {
            continue;
        }
        let deps = read_deps(rule);
        for section in [&deps.normal, &deps.dev, &deps.build] {
            if section.contains("app") {
                problems.push(format!(
                    "mediaa-{}: зависит от `mediaa` (app) — приложение может зависеть на крейты, но не наоборот",
                    rule.name
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn build_dependencies_stay_external() {
    let mut problems = Vec::new();
    for rule in RULES {
        let deps = read_deps(rule);
        for dependency in &deps.build {
            problems.push(format!(
                "mediaa-{}: build-зависимость `mediaa-{dependency}` — build-скрипт не должен тянуть крейты приложения",
                rule.name
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
