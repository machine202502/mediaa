use crate::theme;
use eframe::egui::{self, Color32, FontId, TextFormat};
use mediaa_core::speakers::{self, Piece, Who};
use mediaa_core::transcript::Transcript;
use rust_i18n::t;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Roster {
    pub speakers: Vec<Who>,
}

impl Roster {
    pub fn recording(names: &BTreeMap<u32, String>, ids: impl IntoIterator<Item = u32>) -> Self {
        let mut ids: Vec<u32> = ids.into_iter().collect();
        ids.extend(names.keys().copied());
        ids.sort_unstable();
        ids.dedup();
        Self {
            speakers: ids
                .into_iter()
                .map(|index| Who {
                    index,
                    recording: None,
                    name: names.get(&index).cloned().unwrap_or_default(),
                    title: String::new(),
                })
                .collect(),
        }
    }

    pub fn of_file(ai: &Path, file_id: &str) -> Self {
        if file_id.is_empty() || file_id.starts_with("folder-") {
            return Self::default();
        }
        let Ok(t) = Transcript::open(ai, file_id) else {
            return Self::default();
        };
        let names = t.speaker_names().unwrap_or_default();
        let ids = t.speaker_ids().unwrap_or_default();
        Self::recording(&names, ids)
    }

    pub fn of_files(ai: &Path, files: &[PathBuf]) -> Self {
        let mut speakers = Vec::new();
        for (i, path) in files.iter().enumerate() {
            let Ok(id) = mediaa_core::transcript::file_id(path) else {
                continue;
            };
            let Ok(t) = Transcript::open(ai, &id) else {
                continue;
            };
            let names = t.speaker_names().unwrap_or_default();
            let mut ids = t.speaker_ids().unwrap_or_default();
            ids.extend(names.keys().copied());
            ids.sort_unstable();
            ids.dedup();
            let title = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let recording = (i as u32) + 1;
            for index in ids {
                speakers.push(Who {
                    index,
                    recording: Some(recording),
                    name: names.get(&index).cloned().unwrap_or_default(),
                    title: title.clone(),
                });
            }
        }
        Self { speakers }
    }

    fn unnamed(index: u32) -> String {
        t!("studio.speaker", n = index + 1).into()
    }

    pub fn row_label(who: &Who) -> String {
        let name = if who.name.trim().is_empty() {
            Self::unnamed(who.index)
        } else {
            who.name.trim().to_string()
        };
        if who.recording.is_some() && !who.title.is_empty() {
            format!("{name} ({})", who.title)
        } else {
            name
        }
    }

    pub fn name_of(&self, label: &str) -> Option<String> {
        let parsed = speakers::parse_label(label)?;
        let who = self
            .speakers
            .iter()
            .find(|w| w.index == parsed.index && w.recording == parsed.recording);
        Some(match who {
            Some(w) => Self::row_label(w),
            None => Self::unnamed(parsed.index),
        })
    }

    pub fn to_model(&self, text: &str) -> String {
        speakers::to_model(text, &self.speakers)
    }

    pub fn render(&self, text: &str) -> String {
        self.runs(text).into_iter().map(|(s, _)| s).collect()
    }

    pub fn runs(&self, text: &str) -> Vec<(String, bool)> {
        speakers::pieces(text, &self.speakers, Self::unnamed)
            .into_iter()
            .map(|p| match p {
                Piece::Text(s) => (s.to_string(), false),
                Piece::Mention { text, .. } => (text, true),
            })
            .collect()
    }
}

fn mention_format(base: &TextFormat, p: &theme::Palette) -> TextFormat {
    TextFormat {
        font_id: base.font_id.clone(),
        italics: base.italics,
        color: p.accent,
        background: p.accent_soft,
        underline: egui::Stroke::NONE,
        ..Default::default()
    }
}

pub fn append(
    job: &mut egui::text::LayoutJob,
    text: &str,
    roster: &Roster,
    base: TextFormat,
    p: &theme::Palette,
) {
    for (piece, mention) in roster.runs(text) {
        let format = if mention {
            mention_format(&base, p)
        } else {
            base.clone()
        };
        job.append(&piece, 0.0, format);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn galley(
    ui: &egui::Ui,
    text: &str,
    roster: &Roster,
    font: FontId,
    color: Color32,
    max_width: f32,
    truncate: bool,
    italics: bool,
    p: theme::Palette,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: max_width.max(0.0),
            max_rows: if truncate { 1 } else { usize::MAX },
            break_anywhere: truncate,
            overflow_character: truncate.then_some('…'),
        },
        ..Default::default()
    };
    append(
        &mut job,
        text,
        roster,
        TextFormat {
            font_id: font,
            color,
            italics,
            ..Default::default()
        },
        &p,
    );
    ui.fonts_mut(|f| f.layout_job(job))
}

pub fn show(
    ui: &mut egui::Ui,
    text: &str,
    roster: &Roster,
    font: FontId,
    color: Color32,
    italics: bool,
    p: theme::Palette,
) -> egui::Response {
    let galley = galley(
        ui,
        text,
        roster,
        font,
        color,
        ui.available_width(),
        false,
        italics,
        p,
    );
    ui.add(egui::Label::new(galley).wrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(index: u32, name: &str) -> Who {
        Who {
            index,
            recording: None,
            name: name.into(),
            title: String::new(),
        }
    }

    #[test]
    fn labels_render_as_the_current_name_and_a_rename_is_just_another_roster() {
        let named = Roster {
            speakers: vec![who(0, "Антон"), who(1, "")],
        };
        assert_eq!(
            named.runs("сказал @s1 и @s2"),
            vec![
                ("сказал ".into(), false),
                ("@Антон".into(), true),
                (" и ".into(), false),
                (format!("@{}", t!("studio.speaker", n = 2)), true)
            ]
        );
        let renamed = Roster {
            speakers: vec![who(0, "Олег")],
        };
        assert_eq!(renamed.render("@s1"), "@Олег");
        assert_eq!(renamed.render("привет @Антон"), "привет @Антон");
    }

    #[test]
    fn a_folder_row_names_the_recording() {
        let who = Who {
            index: 1,
            recording: Some(3),
            name: "Анна".into(),
            title: "Планёрка".into(),
        };
        let roster = Roster {
            speakers: vec![
                who.clone(),
                Who {
                    index: 1,
                    recording: Some(4),
                    name: "Анна".into(),
                    title: "Стендап".into(),
                },
            ],
        };
        assert_eq!(Roster::row_label(&who), "Анна (Планёрка)");
        assert_eq!(roster.render("см. @s2#3"), "см. @Анна (Планёрка)");
        assert_eq!(roster.to_model("@s2#3"), "@s2#3");
        assert_eq!(roster.to_model("@Анна"), "@Анна");
    }
}
