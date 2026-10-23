//! UI languages. Translations live in `crates/app/locales/<code>.toml` (one file per
//! language, compiled in by `rust-i18n`); adding a language = adding a file + a row in
//! [`LANGUAGES`]. The default is the system language when it is available.

use mediaa_core::settings::{AfterCapture, GifDither, ThemePreference};
use mediaa_core::MediaKind;
use rust_i18n::t;

/// `(code, native name)` of every bundled language.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("ru", "Русский")];

/// Picks the UI language: explicit setting, else the system locale, else English.
pub fn resolve(setting: Option<&str>, system_locale: Option<&str>) -> &'static str {
    let supported = |code: &str| {
        let base = code
            .split(['-', '_', '.'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        LANGUAGES.iter().map(|(c, _)| *c).find(|c| *c == base)
    };
    setting
        .and_then(supported)
        .or_else(|| system_locale.and_then(supported))
        .unwrap_or("en")
}

pub fn apply(setting: Option<&str>) -> &'static str {
    let lang = resolve(setting, sys_locale::get_locale().as_deref());
    rust_i18n::set_locale(lang);
    lang
}

pub fn current() -> String {
    rust_i18n::locale().to_string()
}

pub fn after_capture(a: AfterCapture) -> String {
    match a {
        AfterCapture::Copy => t!("after.copy"),
        AfterCapture::Save => t!("after.save"),
        AfterCapture::CopyAndSave => t!("after.copy_save"),
    }
    .into()
}

pub fn dither(d: GifDither) -> String {
    match d {
        GifDither::None => t!("dither.none").into(),
        other => other.label().to_string(),
    }
}

pub fn media_kind(k: MediaKind) -> String {
    match k {
        MediaKind::Image => t!("kind.image"),
        MediaKind::Video => t!("kind.video"),
        MediaKind::Animation => t!("kind.animation"),
        MediaKind::Audio => t!("kind.audio"),
    }
    .into()
}

pub fn theme(p: ThemePreference) -> String {
    match p {
        ThemePreference::System => t!("theme.system"),
        ThemePreference::Light => t!("theme.light"),
        ThemePreference::Dark => t!("theme.dark"),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    #[test]
    fn resolution_order() {
        assert_eq!(resolve(Some("ru"), Some("en-US")), "ru");
        assert_eq!(resolve(None, Some("ru-RU")), "ru");
        assert_eq!(resolve(None, Some("ru_RU.UTF-8")), "ru");
        assert_eq!(resolve(None, Some("de-DE")), "en");
        assert_eq!(resolve(Some("xx"), Some("ru")), "ru", "unknown setting falls back to system");
        assert_eq!(resolve(None, None), "en");
        assert_eq!(resolve(Some("EN-gb"), None), "en");
    }

    fn crate_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn flatten(prefix: &str, value: &toml::Value, out: &mut BTreeSet<String>) {
        match value {
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                    flatten(&key, v, out);
                }
            }
            _ => {
                out.insert(prefix.to_string());
            }
        }
    }

    fn keys_of(lang: &str) -> BTreeSet<String> {
        let path = crate_dir().join("locales").join(format!("{lang}.toml"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let value: toml::Value = toml::from_str(&text).unwrap();
        let mut keys = BTreeSet::new();
        flatten("", &value, &mut keys);
        keys.remove("_version");
        keys
    }

    fn strings_of(value: &toml::Value, out: &mut Vec<String>) {
        match value {
            toml::Value::String(s) => out.push(s.clone()),
            toml::Value::Table(t) => t.values().for_each(|v| strings_of(v, out)),
            _ => {}
        }
    }

    /// The user sees no engine: the program, its state and its errors are models and tasks.
    #[test]
    fn no_locale_value_mentions_the_engine() {
        for (lang, _) in LANGUAGES {
            let path = crate_dir().join("locales").join(format!("{lang}.toml"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let value: toml::Value = toml::from_str(&text).unwrap();
            let mut values = Vec::new();
            strings_of(&value, &mut values);
            for value in values {
                let lower = value.to_lowercase();
                assert!(!lower.contains("движ"), "{lang}: «{value}»");
                assert!(!lower.contains("engine"), "{lang}: «{value}»");
            }
        }
    }

    #[test]
    fn every_language_has_the_same_keys() {
        let en = keys_of("en");
        for (lang, _) in LANGUAGES {
            let other = keys_of(lang);
            let missing: Vec<_> = en.difference(&other).collect();
            let extra: Vec<_> = other.difference(&en).collect();
            assert!(missing.is_empty() && extra.is_empty(), "{lang}: missing {missing:?}, extra {extra:?}");
        }
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    #[test]
    fn every_key_used_in_code_exists() {
        let en = keys_of("en");
        let mut files = Vec::new();
        rust_files(&crate_dir().join("src"), &mut files);
        let mut missing = BTreeSet::new();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            for (pos, _) in src.match_indices("t!(\"") {
                // Skip `format!(`, `print!(`… — only a standalone `t!` macro counts.
                let prev = src[..pos].chars().next_back();
                if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let key = src[pos + 4..].split('"').next().unwrap();
                if !en.contains(key) {
                    missing.insert(format!("{key} ({})", f.file_name().unwrap().to_string_lossy()));
                }
            }
        }
        assert!(missing.is_empty(), "keys missing from en.toml: {missing:?}");
    }

    #[test]
    fn interpolation_works() {
        // An explicit locale: the global one is shared by the tests running in parallel.
        assert_eq!(t!("lib.count", n = 3, locale = "en"), "3 files");
        assert_eq!(t!("lib.count", n = 3, locale = "ru"), "файлов: 3");
    }
}
