//! The analysis of a recording as a graph of queue tasks: which stages there are, what each
//! needs, how it is reset and when it counts as ready. One table ([`Stage`]) is the source of
//! truth for the pipeline, the restart and (later) the interface.
//!
//! The analysis is a composite task `analysis` (titled with the file name, scope = the file id) with one
//! child per chosen stage, linked by dependencies. The data lives in the recording's transcript,
//! so a stage continues after a crash and readiness is read from the data itself, not from marks.

use mediaa_core::agent::AnalysisOptions;
use mediaa_core::outline::Chapter;
use mediaa_core::queue::{Child, Filter, KindGroup, NewTask, Queue, Task, TaskId};
use mediaa_core::retrieval::Chunk;
use mediaa_core::transcript::{self, Reply, Transcript};
use serde_json::{json, Value};
use std::path::Path;

/// The kind of the composite task of one analysis.
pub const ANALYSIS_KIND: &str = "analysis";

/// The stages of the analysis, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Transcribe,
    Diarize,
    Index,
    Topics,
    Details,
}

impl Stage {
    pub const ALL: [Stage; 5] = [Stage::Transcribe, Stage::Diarize, Stage::Index, Stage::Topics, Stage::Details];

    /// The queue kind of the stage (the details are a composite of `topic-detail` tasks).
    pub fn kind(self) -> Option<&'static str> {
        match self {
            Stage::Transcribe => Some(crate::transcribe::KIND),
            Stage::Diarize => Some(crate::diarize::KIND),
            Stage::Index => Some(crate::index::KIND),
            Stage::Topics => Some(crate::outline::TOPICS_KIND),
            Stage::Details => Some(crate::outline::DETAILS_KIND),
        }
    }

    /// Whether the options chose the stage (Transcribe is always part of an analysis).
    fn chosen(self, options: &AnalysisOptions) -> bool {
        match self {
            Stage::Transcribe => true,
            Stage::Diarize => options.diarize,
            Stage::Index => options.index,
            Stage::Topics => options.topics,
            Stage::Details => options.details,
        }
    }

    /// The data the stage builds, forgotten when it is rebuilt.
    fn reset(self, ai_dir: &Path, id: &str) -> anyhow::Result<()> {
        if !Transcript::exists(ai_dir, id) {
            return Ok(());
        }
        let mut t = Transcript::open(ai_dir, id)?;
        match self {
            Stage::Transcribe => t.clear(),
            Stage::Diarize => t.clear_speakers(),
            Stage::Index => t.set_chunks(&[]),
            // The chapters go with the `outline-` keys, so the details go too.
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

/// What of a recording is ready, from its data (§0: no stage marks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Readiness {
    pub replies: bool,
    pub speakers: bool,
    pub index: bool,
    pub topics: bool,
    pub details: bool,
}

/// The stages of a new analysis: from `from` on, the chosen ones, with the dependants of a
/// skipped stage skipped too. Stages before `from` keep their data and are not run.
fn stages(from: Stage, options: &AnalysisOptions) -> Vec<Stage> {
    let mut active: Vec<Stage> = Vec::new();
    for stage in Stage::ALL.into_iter().filter(|stage| *stage >= from) {
        let wanted = stage == from || stage.chosen(options);
        // The topics need the index, the details need the topics. An earlier stage (before
        // `from`) is not re-run but its data is kept, so it is enough.
        let prerequisite = match stage {
            Stage::Topics => Some(Stage::Index),
            Stage::Details => Some(Stage::Topics),
            _ => None,
        };
        let ready = prerequisite.is_none_or(|needed| needed < from || active.contains(&needed));
        if wanted && ready && stage.kind().is_some() {
            active.push(stage);
        }
    }
    active
}

fn stage_input(stage: Stage, path: &Path, options: &AnalysisOptions) -> Value {
    match stage {
        Stage::Diarize => json!({ "path": path, "speakers": options.speakers }),
        _ => json!({ "path": path }),
    }
}

/// Enqueues the composite analysis and its stages in one transaction; returns the composite's id.
fn enqueue_analysis(ai_dir: &Path, path: &Path, options: &AnalysisOptions, from: Stage) -> anyhow::Result<TaskId> {
    let id = transcript::file_id(path)?;
    let mut stages = stages(from, options);
    anyhow::ensure!(!stages.is_empty(), "no analysis stage to run from {from:?}");
    // The merge creates the details when the topics run in this analysis; only a restart from
    // the details themselves builds the `details` composite here, from the existing chapters.
    if stages.contains(&Stage::Topics) {
        stages.retain(|stage| *stage != Stage::Details);
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let minutes = mediaa_media::info::probe(path).ok().and_then(|info| info.duration_secs()).unwrap_or(0.0) / 60.0;
    let root = NewTask::new(ANALYSIS_KIND, &name, &json!({ "path": path })).composite().scope(&id);
    let mut children = Vec::with_capacity(stages.len());
    for (i, stage) in stages.iter().enumerate() {
        let child = match stage {
            Stage::Topics => Child::new(crate::outline::topics_task(path, options).scope(&id).units(minutes).priority(10))
                .with_children(vec![Child::new(crate::outline::plan_task(path, options).scope(&id).priority(10))]),
            Stage::Details => {
                let t = Transcript::open(ai_dir, &id)?;
                let chapters = t.chapters()?;
                anyhow::ensure!(!chapters.is_empty(), "{} has no topics to detail", path.display());
                let replies = t.dialog()?;
                Child::new(crate::outline::details_task(path).scope(&id).priority(10))
                    .with_children(crate::outline::detail_tasks(path, options, &chapters, &replies))
            }
            _ => {
                let kind = stage.kind().expect("only stages with a task are in the chain");
                Child::new(NewTask::new(kind, &name, &stage_input(*stage, path, options)).scope(&id).units(minutes).priority(10))
            }
        };
        children.push(if i == 0 { child } else { child.after(&[i - 1]) });
    }
    Ok(Queue::open(ai_dir)?.enqueue_tree(root, &children)?.0)
}

/// Starts the analysis of `path`: the composite task and the chosen stages, with the stages the
/// options turn off left out. Earlier data (a previous transcription) is not touched.
pub fn start(ai_dir: &Path, path: &Path, options: AnalysisOptions) -> anyhow::Result<TaskId> {
    enqueue_analysis(ai_dir, path, &options, Stage::Transcribe)
}

/// Rebuilds `path` from `from`: cancels the unfinished work of the recording, forgets the data
/// of `from` and every later stage that will run, and queues a new analysis of those stages.
pub fn restart(ai_dir: &Path, path: &Path, from: Stage, options: AnalysisOptions) -> anyhow::Result<TaskId> {
    let id = transcript::file_id(path)?;
    let chain = stages(from, &options);
    anyhow::ensure!(!chain.is_empty(), "no analysis stage to run from {from:?}");
    let queue = Queue::open(ai_dir)?;
    // Only the analysis: a chat answer about the same recording keeps running.
    queue.cancel_analysis(&id)?;
    for stage in chain {
        stage.reset(ai_dir, &id)?;
    }
    enqueue_analysis(ai_dir, path, &options, from)
}

/// The unfinished analysis of the recording with `id`, if there is one.
pub fn active(ai_dir: &Path, id: &str) -> anyhow::Result<Option<Task>> {
    let queue = Queue::open(ai_dir)?;
    let filter = Filter { scope: Some(id.to_string()), group: KindGroup::Analysis };
    Ok(queue
        .top_level(&filter)?
        .into_iter()
        .filter(|t| t.kind == ANALYSIS_KIND && !t.status.is_finished())
        .max_by_key(|t| t.id))
}

/// What of the recording with `id` is ready, read from its transcript's data.
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
        // Placeholders with empty titles are the plan, not ready topics.
        topics: !chapters.is_empty() && chapters.iter().all(|chapter| !chapter.digest.title.trim().is_empty()),
        details: !chapters.is_empty() && chapters.iter().all(|chapter| !chapter.digest.sections.is_empty()),
    })
}

/// Raises the priority of the unfinished `topic-detail` of topic `topic` of the recording, so the
/// interface (P5) gets the breakdown it is waiting for first; returns how many tasks were raised.
pub fn prioritize_detail(ai_dir: &Path, file_id: &str, topic: usize) -> anyhow::Result<usize> {
    let queue = Queue::open(ai_dir)?;
    let mut raised = 0;
    for task in queue.list()? {
        if task.kind != crate::outline::DETAIL_KIND || task.status.is_finished() || task.scope.as_deref() != Some(file_id) {
            continue;
        }
        let Ok(input) = serde_json::from_value::<crate::outline::TopicInput>(task.input.clone()) else { continue };
        if input.topic != topic {
            continue;
        }
        queue.set_priority(task.id, crate::outline::DETAIL_PRIORITY)?;
        raised += 1;
    }
    Ok(raised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{AgentSettings, ModelChoices, ModelSource};
    use mediaa_core::outline::Digest;
    use mediaa_core::queue::{Queue, TaskStatus};
    use mediaa_core::transcript::Word;
    use mediaa_engine::Engine;
    use mediaa_engine::testing::FakeEngine;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    /// A short silent PCM WAV the media decoder can read.
    fn wav(path: &Path, seconds: f64) {
        let rate = 8_000u32;
        let samples = (seconds * f64::from(rate)) as u32;
        let data = samples * 2;
        let mut bytes = Vec::with_capacity(44 + data as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        bytes.resize(44 + data as usize, 0);
        std::fs::write(path, bytes).unwrap();
    }

    fn settings(models: ModelChoices) -> AgentSettings {
        AgentSettings { models, ..AgentSettings::default() }
    }

    fn handlers(ai: &Path, settings: &AgentSettings) -> Vec<Box<dyn crate::worker::TaskHandler>> {
        let slots = crate::slots::Slots::new(settings.budget.clone());
        let s = settings.clone();
        let settings: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(move || s.clone());
        let mut handlers: Vec<Box<dyn crate::worker::TaskHandler>> = vec![
            Box::new(crate::transcribe::Transcribe::new(ai, slots.clone(), settings.clone())),
            Box::new(crate::diarize::Diarize::new(ai, slots.clone(), settings.clone())),
            Box::new(crate::index::Index::new(ai, slots.clone(), settings.clone())),
        ];
        handlers.extend(crate::outline::handlers(ai, slots, settings));
        handlers
    }

    /// Runs the queue to the end; the fake answers the outline with a valid digest and a section
    /// per topic.
    fn drain(q: &Queue, engine: &Arc<dyn Engine>, handlers: &[Box<dyn crate::worker::TaskHandler>]) {
        let stop = AtomicBool::new(false);
        while crate::worker::run_next(q, engine, handlers, &stop).unwrap() {}
    }

    const DIGEST: &str = r#"{"title":"Тема","summary":"сводка","decisions":[],"actions":[],"questions":[],"topics":[],"quotes":[]}"#;
    const SECTIONS: &str = r#"{"sections":[{"title":"Итоги","kind":"list","items":[{"text":"привет"}]}]}"#;

    fn id(path: &Path) -> String {
        transcript::file_id(path).unwrap()
    }

    #[test]
    fn the_whole_analysis_runs_in_order_through_the_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let settings = settings(ModelChoices::default());
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        fake.chat_answers(&[DIGEST, SECTIONS]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let task = start(&ai, &media, AnalysisOptions::default()).unwrap();
        let handlers = handlers(&ai, &settings);
        drain(&queue, &engine, &handlers);

        let analysis = queue.get(task).unwrap().unwrap();
        assert_eq!(analysis.status, TaskStatus::Done, "{:?}", analysis.error);
        let stages: Vec<String> = queue.children(task).unwrap().into_iter().map(|t| t.kind).collect();
        assert_eq!(stages, ["transcribe", "diarize", "index", "topics"], "the dependencies hold the order");
        let tree = queue.subtree(task).unwrap();
        assert!(tree.iter().all(|t| t.status == TaskStatus::Done), "{tree:?}");
        let kinds: Vec<&str> = tree.iter().map(|t| t.kind.as_str()).collect();
        for kind in ["topics-plan", "topic-digest", "topics-merge", "details", "topic-detail"] {
            assert!(kinds.contains(&kind), "{kind} is in the tree: {kinds:?}");
        }
        assert!(!fake.vads().is_empty() && !fake.recognizes().is_empty());
        assert!(!fake.diarizes().is_empty());
        assert!(fake.embeddings_count() > 0);
        assert!(!fake.chats().is_empty());

        let ready = readiness(&ai, &id(&media)).unwrap();
        assert_eq!(ready, Readiness { replies: true, speakers: true, index: true, topics: true, details: true });
    }

    #[test]
    fn disabled_stages_are_not_queued_and_dependencies_follow() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        let engine: Arc<dyn Engine> = Arc::new(fake);
        let handlers = handlers(&ai, &settings(ModelChoices::default()));

        // Without the index there are no topics either (details need topics).
        let options = AnalysisOptions { diarize: false, speakers: Some(2), index: false, topics: true, details: true };
        let task = start(&ai, &media, options).unwrap();
        assert_eq!(queue.children(task).unwrap().len(), 1);
        assert_eq!(queue.children(task).unwrap()[0].kind, "transcribe");
        drain(&queue, &engine, &handlers);
        assert_eq!(queue.get(task).unwrap().unwrap().status, TaskStatus::Done);
        let ready = readiness(&ai, &id(&media)).unwrap();
        assert_eq!(ready, Readiness { replies: true, speakers: false, index: false, topics: false, details: false });
    }

    #[test]
    fn diarization_failure_does_not_stop_the_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let models = ModelChoices { diarization: Some(ModelSource::HuggingFace { repo: "fails/model".into() }), ..ModelChoices::default() };
        let settings = settings(models);
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        fake.chat_answers(&[DIGEST, SECTIONS]);
        let engine: Arc<dyn Engine> = Arc::new(fake);
        let task = start(&ai, &media, AnalysisOptions::default()).unwrap();
        drain(&queue, &engine, &handlers(&ai, &settings));

        let children = queue.children(task).unwrap();
        assert!(children.iter().all(|t| t.status == TaskStatus::Done), "{children:?}");
        let diarize = children.iter().find(|t| t.kind == crate::diarize::KIND).unwrap();
        assert!(diarize.result.as_ref().unwrap()["warning"].as_str().is_some(), "{:?}", diarize.result);
        let ready = readiness(&ai, &id(&media)).unwrap();
        assert!(ready.replies && ready.index && ready.topics);
        assert!(!ready.speakers, "the replies stay without speakers");
    }

    #[test]
    fn restart_from_the_index_cancels_the_analysis_and_keeps_the_words() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let settings = settings(ModelChoices::default());
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        fake.chat_answers(&[DIGEST, SECTIONS, DIGEST, SECTIONS]);
        let engine: Arc<dyn Engine> = Arc::new(fake);
        let handlers = handlers(&ai, &settings);
        start(&ai, &media, AnalysisOptions::default()).unwrap();
        drain(&queue, &engine, &handlers);
        let t = Transcript::open(&ai, &id(&media)).unwrap();
        let replies = t.dialog().unwrap();
        t.rename_speaker(0, "Дана").unwrap();
        let names = t.speaker_names().unwrap();
        drop(t);
        let speakers = replies.iter().filter_map(|r| r.speaker).collect::<Vec<_>>();

        // An analysis that has not finished yet: the restart must cancel it.
        let running = start(&ai, &media, AnalysisOptions::default()).unwrap();
        let restarted = restart(&ai, &media, Stage::Index, AnalysisOptions::default()).unwrap();
        assert_eq!(queue.get(running).unwrap().unwrap().status, TaskStatus::Cancelled, "running analysis cancelled");
        let kinds: Vec<String> = queue.children(restarted).unwrap().into_iter().map(|t| t.kind).collect();
        assert_eq!(kinds, ["index", "topics"], "only the index and what follows it");

        drain(&queue, &engine, &handlers);
        assert_eq!(queue.get(restarted).unwrap().unwrap().status, TaskStatus::Done);
        let t = Transcript::open(&ai, &id(&media)).unwrap();
        assert_eq!(t.dialog().unwrap().iter().filter_map(|r| r.speaker).collect::<Vec<_>>(), speakers, "speakers kept");
        assert_eq!(t.speaker_names().unwrap(), names, "names kept");
        assert!(!t.chunks().unwrap().is_empty());
        assert!(t.chunks().unwrap().iter().all(|(_, _, embedded)| *embedded));
        assert!(!t.chapters().unwrap().is_empty());
        let ready = readiness(&ai, &id(&media)).unwrap();
        assert_eq!(ready, Readiness { replies: true, speakers: true, index: true, topics: true, details: true });
    }

    #[test]
    fn a_restart_from_the_details_rebuilds_them_and_keeps_a_chat_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let settings = settings(ModelChoices::default());
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        fake.chat_answers(&[DIGEST, SECTIONS, SECTIONS]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let handlers = handlers(&ai, &settings);
        start(&ai, &media, AnalysisOptions::default()).unwrap();
        drain(&queue, &engine, &handlers);
        let t = Transcript::open(&ai, &id(&media)).unwrap();
        assert!(t.chapters().unwrap().iter().all(|c| !c.digest.sections.is_empty()));

        // A chat answer about the same recording is not the analysis: the restart leaves it.
        let answer = queue.begin_inline("answer", "тест", &json!({}), None, Some(&id(&media))).unwrap();
        let restarted = restart(&ai, &media, Stage::Details, AnalysisOptions::default()).unwrap();
        assert_eq!(queue.get(answer).unwrap().unwrap().status, TaskStatus::Running, "the chat answer survives");
        let kinds: Vec<String> = queue.children(restarted).unwrap().into_iter().map(|t| t.kind).collect();
        assert_eq!(kinds, ["details"], "only the details");
        let details = queue.children(restarted).unwrap()[0].id;
        assert!(!queue.children(details).unwrap().is_empty(), "one topic-detail per topic");
        let t = Transcript::open(&ai, &id(&media)).unwrap();
        assert!(t.chapters().unwrap().iter().all(|c| c.digest.sections.is_empty()), "the old details are dropped");

        drain(&queue, &engine, &handlers);
        assert_eq!(queue.get(restarted).unwrap().unwrap().status, TaskStatus::Done);
        let ready = readiness(&ai, &id(&media)).unwrap();
        assert!(ready.details, "the details are built again");
    }

    #[test]
    fn every_composite_ends_done_after_a_restart_from_the_topics_and_from_the_details() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let settings = settings(ModelChoices::default());
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        fake.chat_answers(&[DIGEST, SECTIONS, DIGEST, SECTIONS, SECTIONS]);
        let engine: Arc<dyn Engine> = Arc::new(fake);
        let handlers = handlers(&ai, &settings);
        start(&ai, &media, AnalysisOptions::default()).unwrap();
        drain(&queue, &engine, &handlers);
        for from in [Stage::Topics, Stage::Details] {
            let root = restart(&ai, &media, from, AnalysisOptions::default()).unwrap();
            let before = queue.subtree(root).unwrap();
            assert!(
                before.iter().filter(|t| t.kind == "details" || t.kind == "topics" || t.kind == ANALYSIS_KIND).all(|t| t.status == TaskStatus::Running),
                "{from:?}: a stage that can run makes its composites go: {:?}",
                before.iter().map(|t| (&t.kind, t.status)).collect::<Vec<_>>()
            );
            // The program is closed and opened again while the work is half done.
            let first = crate::worker::run_next(&queue, &engine, &handlers, &AtomicBool::new(false)).unwrap();
            assert!(first);
            queue.requeue_running().unwrap();
            drain(&queue, &engine, &handlers);
            for t in queue.subtree(root).unwrap() {
                assert_eq!(t.status, TaskStatus::Done, "{from:?}: {} {}", t.kind, t.title);
            }
        }
    }

    #[test]
    fn the_details_of_one_topic_can_go_first() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let settings = settings(ModelChoices::default());
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        fake.chat_answers(&[DIGEST, SECTIONS]);
        let engine: Arc<dyn Engine> = Arc::new(fake);
        let handlers = handlers(&ai, &settings);
        start(&ai, &media, AnalysisOptions::default()).unwrap();
        drain(&queue, &engine, &handlers);
        // The details again, this time unfinished: their tasks wait in the queue.
        restart(&ai, &media, Stage::Details, AnalysisOptions::default()).unwrap();
        let id = id(&media);
        let target = queue
            .list()
            .unwrap()
            .into_iter()
            .find(|t| t.kind == "topic-detail" && t.status == TaskStatus::Queued)
            .unwrap();
        assert_eq!(target.priority, 10, "the usual priority of a topic task");
        assert_eq!(prioritize_detail(&ai, &id, 0).unwrap(), 1);
        assert_eq!(queue.get(target.id).unwrap().unwrap().priority, crate::outline::DETAIL_PRIORITY);
        assert_eq!(prioritize_detail(&ai, &id, 5).unwrap(), 0, "no such topic");
        assert_eq!(prioritize_detail(&ai, "another recording", 0).unwrap(), 0);
    }

    #[test]
    fn the_topics_reset_forgets_the_details_too() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 1.0);
        let id = id(&media);
        let t = Transcript::open(&ai, &id).unwrap();
        t.set_meta("outline-plan", "{}").unwrap();
        t.put_chapter(
            0,
            &Chapter {
                start: 0.0,
                end: 1.0,
                digest: Digest { title: "Тема".into(), sections: vec![mediaa_core::outline::Section { title: "Итоги".into(), ..Default::default() }], ..Digest::default() },
            },
        )
        .unwrap();
        t.set_meta(crate::outline::TOPICS_KEY, "[]").unwrap();
        drop(t);
        assert!(readiness(&ai, &id).unwrap().details);
        Stage::Topics.reset(&ai, &id).unwrap();
        let t = Transcript::open(&ai, &id).unwrap();
        assert!(t.chapters().unwrap().is_empty(), "the chapters go");
        assert!(t.meta(crate::outline::TOPICS_KEY).unwrap().is_none(), "the outline keys go");
        assert!(!readiness(&ai, &id).unwrap().details);
    }

    #[test]
    fn readiness_follows_the_data() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 1.0);
        let id = id(&media);
        assert_eq!(readiness(&ai, &id).unwrap(), Readiness::default(), "no transcript, nothing ready");
        let mut t = Transcript::open(&ai, &id).unwrap();
        let word = Word { text: "привет".into(), start: 0.0, end: 0.5 };
        t.set_replies(&[Reply { start: 0.0, end: 0.5, speaker: None, words: vec![word] }]).unwrap();
        assert_eq!(readiness(&ai, &id).unwrap(), Readiness { replies: true, ..Readiness::default() });
        let chunk = Chunk { start: 0.0, end: 0.5, speakers: vec![], text: "привет".into() };
        t.set_chunks(std::slice::from_ref(&chunk)).unwrap();
        assert!(!readiness(&ai, &id).unwrap().index, "a chunk without an embedding is not a search index");
        let (chunk_id, _, _) = t.chunks().unwrap()[0];
        t.set_embedding(chunk_id, &[1.0, 0.0]).unwrap();
        assert!(readiness(&ai, &id).unwrap().index);
        t.put_chapter(0, &Chapter { start: 0.0, end: 0.5, digest: Digest::default() }).unwrap();
        assert!(!readiness(&ai, &id).unwrap().topics, "a placeholder is not a topic");
        t.put_chapter(0, &Chapter { start: 0.0, end: 0.5, digest: Digest { title: "Тема".into(), ..Digest::default() } }).unwrap();
        assert!(readiness(&ai, &id).unwrap().topics);
        assert!(!readiness(&ai, &id).unwrap().details, "details are not built yet");
        let sections = vec![mediaa_core::outline::Section { title: "Итоги".into(), ..Default::default() }];
        t.put_chapter(0, &Chapter { start: 0.0, end: 0.5, digest: Digest { title: "Тема".into(), sections, ..Digest::default() } }).unwrap();
        assert!(readiness(&ai, &id).unwrap().details, "a chapter with sections is broken down");
    }

    #[test]
    fn analysis_options_are_saved_with_the_settings() {
        let json = serde_json::to_string(&AgentSettings::default()).unwrap();
        let back: AgentSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.analysis, AnalysisOptions::default());
        assert!(back.analysis.diarize && back.analysis.index && back.analysis.topics && back.analysis.details);
        assert_eq!(back.analysis.speakers, None);
        // A settings file from before the field gets the defaults.
        let old: AgentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.analysis, AnalysisOptions::default());
        let models = ModelChoices { diarization: None, ..ModelChoices::default() };
        let resolved = AnalysisOptions::default().resolved(&models);
        assert!(!resolved.diarize && resolved.speakers.is_none(), "no diarization model, no diarization");
        let resolved = AnalysisOptions { index: false, topics: true, details: true, ..AnalysisOptions::default() }.resolved(&ModelChoices::default());
        assert!(!resolved.topics && !resolved.details, "the topics need the index");
    }

    #[test]
    fn an_active_analysis_is_the_unfinished_composite() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 1.0);
        assert!(active(&ai, &id(&media)).unwrap().is_none());
        let task = start(&ai, &media, AnalysisOptions::default()).unwrap();
        assert_eq!(active(&ai, &id(&media)).unwrap().map(|t| t.id), Some(task));
    }

    /// The analysis graph, stopped once the topics composite has digests and the details
    /// composite has its `topic-detail` children. Later stages are still queued.
    fn analysis_graph() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        wav(&media, 3.0);
        let settings = settings(ModelChoices::default());
        let queue = Queue::open(&ai).unwrap();
        let fake = FakeEngine::new();
        fake.chat_answers(&[DIGEST, DIGEST, DIGEST, SECTIONS, SECTIONS]);
        let engine: Arc<dyn Engine> = Arc::new(fake);
        let handlers = handlers(&ai, &settings);
        start(&ai, &media, AnalysisOptions::default()).unwrap();
        let stop = AtomicBool::new(false);
        let mut saw_detail = false;
        for _ in 0..40 {
            if queue.list().unwrap().iter().any(|t| t.kind == "topic-detail") {
                saw_detail = true;
                break;
            }
            assert!(crate::worker::run_next(&queue, &engine, &handlers, &stop).unwrap(), "the graph ended before the details");
        }
        assert!(saw_detail, "no topic-detail was queued");
        let kinds: std::collections::HashSet<String> = queue.list().unwrap().into_iter().map(|t| t.kind).collect();
        for kind in ["analysis", "transcribe", "diarize", "index", "topics", "topics-plan", "topic-digest", "topics-merge", "details", "topic-detail"] {
            assert!(kinds.contains(kind), "{kind} missing from {kinds:?}");
        }
        drop(queue);
        tmp
    }

    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let dest = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), &dest).unwrap();
            }
        }
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            (self.0 >> 33) as usize % n
        }
    }

    fn assert_rule(q: &Queue, seed: u64, step: usize) {
        use mediaa_core::queue::{composite_status, ChildState};
        use std::collections::{HashMap, HashSet};
        let tasks = q.list().unwrap();
        let by_id: HashMap<_, _> = tasks.iter().map(|t| (t.id, t)).collect();
        for task in &tasks {
            if matches!(task.status, TaskStatus::Queued | TaskStatus::Waiting | TaskStatus::Running | TaskStatus::Paused) {
                let mut stack = q.needs(task.id).unwrap();
                let mut seen = HashSet::new();
                while let Some(dep) = stack.pop() {
                    if !seen.insert(dep) {
                        continue;
                    }
                    let Some(other) = by_id.get(&dep) else { continue };
                    assert!(
                        !matches!(other.status, TaskStatus::Failed | TaskStatus::Cancelled),
                        "seed {seed} step {step}: {} {} {:?} waits on {} {} {:?}",
                        task.id, task.kind, task.status, other.id, other.kind, other.status
                    );
                    stack.extend(q.needs(dep).unwrap());
                }
            }
            let children: Vec<_> = tasks.iter().filter(|child| child.parent == Some(task.id)).collect();
            if children.is_empty() || task.inline {
                continue;
            }
            let states: Vec<ChildState> = children
                .iter()
                .map(|child| {
                    let blocked = q.needs(child.id).unwrap().iter().any(|dep| by_id.get(dep).is_some_and(|other| other.status != TaskStatus::Done));
                    ChildState::of(child.status, blocked)
                })
                .collect();
            let expected = composite_status(&states).unwrap();
            assert_eq!(task.status, expected, "seed {seed} step {step}: {} {} {:?}", task.kind, task.title, states);
            if matches!(task.status, TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Paused) {
                assert!(task.error.is_none(), "seed {seed} step {step}: {} still shows {:?}", task.kind, task.error);
            }
        }
    }

    struct FinishKind(&'static str);

    impl crate::worker::TaskHandler for FinishKind {
        fn kind(&self) -> &'static str {
            self.0
        }
        fn run(&self, _task: &mediaa_core::queue::Task, _cp: &mut crate::worker::Checkpoints) -> anyhow::Result<serde_json::Value> {
            Ok(json!({}))
        }
    }

    fn drain_with_a_fake_handler(q: &Queue, seed: u64) {
        let handlers: Vec<Box<dyn crate::worker::TaskHandler>> = ["transcribe", "diarize", "index", "topics-plan", "topic-digest", "topics-merge", "topic-detail"]
            .into_iter()
            .map(|kind| Box::new(FinishKind(kind)) as Box<dyn crate::worker::TaskHandler>)
            .collect();
        let engine: Arc<dyn Engine> = Arc::new(FakeEngine::new());
        let stop = AtomicBool::new(false);
        for step in 0..400 {
            let tasks = q.list().unwrap();
            if tasks.iter().all(|t| t.status.is_finished()) {
                return;
            }
            for task in tasks.iter().filter(|t| t.status == TaskStatus::Paused) {
                q.resume(task.id).unwrap();
            }
            if !crate::worker::run_next(q, &engine, &handlers, &stop).unwrap() {
                let left: Vec<_> = q.list().unwrap().into_iter().filter(|t| !t.status.is_finished()).map(|t| format!("{} {} {:?}", t.id, t.kind, t.status)).collect();
                panic!("seed {seed} step {step} stuck with {left:?}");
            }
        }
        panic!("seed {seed} did not finish");
    }

    #[test]
    fn restarting_a_failed_detail_runs_every_ancestor() {
        let graph = analysis_graph();
        let q = Queue::open(&graph.path().join(".ai")).unwrap();
        let mut details: Vec<_> = q.list().unwrap().into_iter().filter(|t| t.kind == "topic-detail").map(|t| t.id).collect();
        details.sort_unstable();
        let failed = details.pop().unwrap();
        for id in &details {
            q.complete(*id, &json!({})).unwrap();
        }
        q.fail(failed, "model crashed").unwrap();
        let mut ancestor = q.get(failed).unwrap().unwrap().parent;
        let mut depth = 0;
        while let Some(id) = ancestor {
            let t = q.get(id).unwrap().unwrap();
            assert_eq!(t.status, TaskStatus::Failed, "{} {}", t.kind, t.title);
            assert!(t.error.is_some(), "{} has no error", t.kind);
            ancestor = t.parent;
            depth += 1;
        }
        assert!(depth >= 3, "details, topics and the analysis");
        q.restart(failed).unwrap();
        let restarted = q.get(failed).unwrap().unwrap();
        assert_eq!(restarted.status, TaskStatus::Queued);
        assert!(restarted.error.is_none());
        ancestor = restarted.parent;
        while let Some(id) = ancestor {
            let t = q.get(id).unwrap().unwrap();
            assert_eq!(t.status, TaskStatus::Running, "{} {}", t.kind, t.title);
            assert!(t.error.is_none(), "{} kept {:?}", t.kind, t.error);
            ancestor = t.parent;
        }
    }

    #[test]
    fn random_queue_ops_keep_the_analysis_graph_safe() {
        let graph = analysis_graph();
        let source = graph.path().join(".ai");
        for seed in 1..=200 {
            let dir = tempfile::tempdir().unwrap();
            let ai = dir.path().join(".ai");
            copy_dir(&source, &ai);
            let q = Queue::open(&ai).unwrap();
            assert_rule(&q, seed, 0);
            let mut rng = Rng(seed);
            for step in 1..=24 {
                let tasks = q.list().unwrap();
                if tasks.is_empty() {
                    break;
                }
                let id = tasks[rng.below(tasks.len())].id;
                match rng.below(6) {
                    0 => q.cancel(id).unwrap(),
                    1 => q.pause(id).unwrap(),
                    2 => q.resume(id).unwrap(),
                    3 => q.restart(id).unwrap(),
                    4 => q.fail(id, "boom").unwrap(),
                    _ => q.complete(id, &json!({})).unwrap(),
                }
                assert_rule(&q, seed, step);
            }
            drain_with_a_fake_handler(&q, seed);
            assert!(q.list().unwrap().iter().all(|t| t.status.is_finished()), "seed {seed}");
        }
    }
}
