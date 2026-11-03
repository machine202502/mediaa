#![forbid(unsafe_code)]

rust_i18n::i18n!("locales", fallback = "en");

pub use rust_i18n::t;

pub const LANGUAGES: [(&str, &str); 2] = [("en", "English"), ("ru", "Русский")];

pub fn resolve_language(setting: Option<&str>, system: Option<&str>) -> &'static str {
    match setting.map(str::trim).filter(|value| !value.is_empty()) {
        Some("ru") => "ru",
        Some("en") => "en",
        Some(_) => "en",
        None => match system {
            Some(value) if value.to_ascii_lowercase().starts_with("ru") => "ru",
            _ => "en",
        },
    }
}

pub fn set_language(setting: Option<&str>, system: Option<&str>) -> &'static str {
    let language = resolve_language(setting, system);
    rust_i18n::set_locale(language);
    language
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn keys(value: &toml::Value, prefix: &str, out: &mut BTreeSet<String>) {
        if let toml::Value::Table(table) = value {
            for (key, value) in table {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if value.is_table() {
                    keys(value, &path, out);
                } else {
                    out.insert(path);
                }
            }
        }
    }

    fn locale_keys(text: &str) -> BTreeSet<String> {
        let value: toml::Value = toml::from_str(text).expect("locale is valid toml");
        let mut result = BTreeSet::new();
        keys(&value, "", &mut result);
        result
    }

    #[test]
    fn locale_files_have_the_same_keys() {
        let en = locale_keys(include_str!("../locales/en.toml"));
        let ru = locale_keys(include_str!("../locales/ru.toml"));
        assert_eq!(en, ru);
        assert!(!en.is_empty());
    }

    #[test]
    fn language_resolution_prefers_setting_over_system() {
        assert_eq!(resolve_language(Some("ru"), Some("en-US")), "ru");
        assert_eq!(resolve_language(Some("en"), Some("ru-RU")), "en");
        assert_eq!(resolve_language(None, Some("ru-RU")), "ru");
        assert_eq!(resolve_language(None, Some("en-US")), "en");
        assert_eq!(resolve_language(Some("de"), Some("ru")), "en");
        assert_eq!(resolve_language(Some(""), Some("ru")), "ru");
    }

    #[test]
    fn languages_are_listed_once() {
        let codes: BTreeSet<_> = LANGUAGES.iter().map(|(code, _)| *code).collect();
        assert_eq!(codes, BTreeSet::from(["en", "ru"]));
    }
}
