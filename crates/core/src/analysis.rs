use crate::agent::AnalysisOptions;
use crate::outline::Chapter;
use crate::retrieval::Chunk;
use crate::transcript::{Reply, Transcript};
use std::path::Path;

pub const TRANSCRIBE_KIND: &str = "transcribe";
pub const DIARIZE_KIND: &str = "diarize";
pub const INDEX_KIND: &str = "index";
pub const TOPICS_KIND: &str = "topics";
pub const PLAN_KIND: &str = "topics-plan";
pub const DIGEST_KIND: &str = "topic-digest";
pub const MERGE_KIND: &str = "topics-merge";
pub const DETAILS_KIND: &str = "details";
pub const DETAIL_KIND: &str = "topic-detail";
pub const ANALYSIS_KIND: &str = "analysis";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Transcribe,
    Diarize,
    Index,
    Topics,
    Details,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Transcribe,
        Stage::Diarize,
        Stage::Index,
        Stage::Topics,
        Stage::Details,
    ];

    pub fn kind(self) -> Option<&'static str> {
        match self {
            Stage::Transcribe => Some(TRANSCRIBE_KIND),
            Stage::Diarize => Some(DIARIZE_KIND),
            Stage::Index => Some(INDEX_KIND),
            Stage::Topics => Some(TOPICS_KIND),
            Stage::Details => Some(DETAILS_KIND),
        }
    }

    pub fn parse(text: &str) -> Option<Stage> {
        match text {
            "transcribe" => Some(Stage::Transcribe),
            "diarize" => Some(Stage::Diarize),
            "index" => Some(Stage::Index),
            "topics" => Some(Stage::Topics),
            "details" => Some(Stage::Details),
            _ => None,
        }
    }

    pub fn chosen(self, options: &AnalysisOptions) -> bool {
        match self {
            Stage::Transcribe => true,
            Stage::Diarize => false,
            Stage::Index => options.index,
            Stage::Topics => options.topics,
            Stage::Details => options.details,
        }
    }

    pub fn reset(self, ai_dir: &Path, id: &str) -> anyhow::Result<()> {
        if !Transcript::exists(ai_dir, id) {
            return Ok(());
        }
        let mut t = Transcript::open(ai_dir, id)?;
        match self {
            Stage::Transcribe => t.clear(),
            Stage::Diarize => t.clear_speakers(),
            Stage::Index => t.set_chunks(&[]),
            Stage::Topics => {
                t.clear_chapters()?;
                t.remove_meta_prefix("outline-")
            }
            Stage::Details => {
                for (i, mut chapter) in t.chapters()?.into_iter().enumerate() {
                    chapter.digest.sections.clear();
                    t.put_chapter(i, &chapter)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Readiness {
    pub replies: bool,
    pub speakers: bool,
    pub index: bool,
    pub topics: bool,
    pub details: bool,
}

pub fn readiness(ai_dir: &Path, id: &str) -> anyhow::Result<Readiness> {
    if !Transcript::exists(ai_dir, id) {
        return Ok(Readiness::default());
    }
    let t = Transcript::open(ai_dir, id)?;
    let dialog: Vec<Reply> = t.dialog()?;
    let chunks: Vec<(i64, Chunk, bool)> = t.chunks()?;
    let chapters: Vec<Chapter> = t.chapters()?;
    Ok(Readiness {
        replies: !dialog.is_empty(),
        speakers: dialog.iter().any(|reply| reply.speaker.is_some()),
        index: !chunks.is_empty() && chunks.iter().all(|(_, _, embedded)| *embedded),
        topics: !chapters.is_empty()
            && chapters
                .iter()
                .all(|chapter| !chapter.digest.title.trim().is_empty()),
        details: !chapters.is_empty()
            && chapters
                .iter()
                .all(|chapter| !chapter.digest.sections.is_empty()),
    })
}
