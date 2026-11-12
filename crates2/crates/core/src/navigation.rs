use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Screenshot,
    Record,
    RecordAudio,
    Library,
    Transcription,
    Convert,
    Settings,
    About,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::Screenshot,
        Page::Record,
        Page::RecordAudio,
        Page::Library,
        Page::Transcription,
        Page::Convert,
        Page::Settings,
        Page::About,
    ];

    pub fn from_arg(text: &str) -> Option<Page> {
        match text {
            "screenshot" => Some(Page::Screenshot),
            "record" => Some(Page::Record),
            "audio" | "record_audio" => Some(Page::RecordAudio),
            "library" => Some(Page::Library),
            "transcription" => Some(Page::Transcription),
            "convert" => Some(Page::Convert),
            "settings" => Some(Page::Settings),
            "about" => Some(Page::About),
            _ => None,
        }
    }

    pub fn to_arg(self) -> &'static str {
        match self {
            Page::Screenshot => "screenshot",
            Page::Record => "record",
            Page::RecordAudio => "record_audio",
            Page::Library => "library",
            Page::Transcription => "transcription",
            Page::Convert => "convert",
            Page::Settings => "settings",
            Page::About => "about",
        }
    }
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

    #[test]
    fn page_args_round_trip() {
        for page in Page::ALL {
            assert_eq!(Page::from_arg(page.to_arg()), Some(page));
        }
        assert_eq!(Page::from_arg("audio"), Some(Page::RecordAudio));
        assert_eq!(Page::from_arg("nope"), None);
    }

    fn page_key(page: Page) -> &'static str {
        match page {
            Page::Screenshot => "screenshot",
            Page::Transcription => "transcription",
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
