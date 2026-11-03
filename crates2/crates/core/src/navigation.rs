use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Screenshot,
    Record,
    RecordAudio,
    Library,
    Convert,
    Settings,
    About,
}

impl Page {
    pub const ALL: [Page; 7] = [
        Page::Screenshot,
        Page::Record,
        Page::RecordAudio,
        Page::Library,
        Page::Convert,
        Page::Settings,
        Page::About,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_survive_serialization() {
        for page in Page::ALL {
            let text = format!("page = \"{}\"", page_key(page));
            let back: Wrap = toml::from_str(&text).unwrap();
            assert_eq!(back.page, page);
        }
    }

    fn page_key(page: Page) -> &'static str {
        match page {
            Page::Screenshot => "screenshot",
            Page::Record => "record",
            Page::RecordAudio => "record_audio",
            Page::Library => "library",
            Page::Convert => "convert",
            Page::Settings => "settings",
            Page::About => "about",
        }
    }

    #[derive(Deserialize)]
    struct Wrap {
        page: Page,
    }
}
