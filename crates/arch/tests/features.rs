use mediaa_arch::{feature_users, source_mentions, source_violations};

#[test]
fn testing_feature_is_dev_only() {
    let uses = feature_users("testing");
    assert!(
        !uses.is_empty(),
        "правило устарело: feature `testing` больше никто не включает — удали проверку"
    );
    let mut problems = Vec::new();
    for usage in &uses {
        if usage.section != "dev" && usage.section != "target dev" {
            problems.push(format!(
                "mediaa-{}: зависимость `{}` включает feature `testing` в разделе «{}»; \
                 тестовые заглушки допустимы только в dev-dependencies",
                usage.crate_name, usage.dependency, usage.section
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "feature `testing` попала в production-сборку:\n{}",
        problems.join("\n")
    );
}

#[test]
fn testing_module_is_test_only() {
    let pattern = "mediaa_engine::testing";
    assert!(
        source_mentions(pattern) > 0,
        "правило устарело: `{pattern}` больше нигде не встречается — удали проверку"
    );
    let violations = source_violations(pattern);
    assert!(
        violations.is_empty(),
        "`{pattern}` используется вне тестового кода:\n{}",
        violations
            .iter()
            .map(|(file, line, text)| format!("{file}:{line}: {}", text.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
