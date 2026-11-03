use crate::formats::MediaKind;
use crate::library::LibraryEntry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySort {
    #[default]
    Newest,
    Oldest,
    Title,
    Largest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryQuery {
    pub search: String,
    pub kind: Option<MediaKind>,
    pub sort: LibrarySort,
    pub offset: u32,
    pub limit: u32,
}

impl Default for LibraryQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            kind: None,
            sort: LibrarySort::Newest,
            offset: 0,
            limit: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryPage {
    pub revision: u64,
    pub total: u32,
    pub entries: Vec<LibraryEntry>,
    pub next_offset: Option<u32>,
}
