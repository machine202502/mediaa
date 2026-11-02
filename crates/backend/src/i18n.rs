const LANGUAGES: [&str; 2] = ["en", "ru"];

fn supported(code: &str) -> Option<&'static str> {
    let base = code
        .split(['-', '_', '.'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    LANGUAGES.into_iter().find(|language| *language == base)
}

pub fn resolve(setting: Option<&str>, system_locale: Option<&str>) -> &'static str {
    setting
        .and_then(supported)
        .or_else(|| system_locale.and_then(supported))
        .unwrap_or("en")
}

pub fn apply(setting: Option<&str>) -> &'static str {
    let language = resolve(setting, sys_locale::get_locale().as_deref());
    rust_i18n::set_locale(language);
    language
}

pub fn current() -> String {
    rust_i18n::locale().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_labels_are_localized() {
        let previous = current();
        apply(Some("ru"));
        assert_eq!(t!("tray.show").to_string(), "Открыть MediaA");
        apply(Some("en"));
        assert_eq!(t!("tray.show").to_string(), "Open MediaA");
        rust_i18n::set_locale(&previous);
    }
}
