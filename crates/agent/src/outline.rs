//! The `outline` queue task: chapter digests of a recording (`mediaa_core::outline`) written by
//! the agent's model window by window as constrained JSON. Queued after indexing; resumable
//! chapter by chapter.

use crate::assistant::{Message, Role};
use crate::prompts;
use mediaa_engine::{ChatMessage, ChatRequest, Device, Engine, Priority};
use crate::slots::Slots;
use crate::worker::{Checkpoints, TaskHandler};
use mediaa_core::agent::{AgentSettings, ModelRole, WorkKind};
use mediaa_core::library::human_duration;
use mediaa_core::outline::{self, Chapter, Digest, Window, DIGEST_SCHEMA};
use mediaa_core::queue::{Queue, Task};
use mediaa_core::transcript::{self, Transcript};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const KIND: &str = "outline";

/// Context of the model for one window (~1200 words of Russian ≈ 3000 tokens, plus the answer).
const OUTLINE_CTX: u32 = 8192;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
}

/// Queues the outline of `path` (after its indexing).
pub fn enqueue(ai_dir: &Path, path: &Path) -> anyhow::Result<()> {
    let title = path.file_name().unwrap_or_default().to_string_lossy();
    Queue::open(ai_dir)?.enqueue_once(KIND, &title, &serde_json::to_value(Input { path: path.to_path_buf() })?, 0)?;
    Ok(())
}

/// The request for window `i` of `n`.
pub fn prompt(window: &Window, i: usize, n: usize, previous: Option<&str>) -> String {
    let (start, end, language) = (human_duration(window.start), human_duration(window.end), outline::language(&window.text));
    prompts::render("outline/part.j2", minijinja::context! { number => i + 1, parts => n, start, end, language, previous, text => &window.text })
}

/// Asks the engine's `model` for the digest of a window.
#[allow(clippy::too_many_arguments)]
pub fn digest(engine: &dyn Engine, model: &str, device: Device, hf_token: Option<&str>, window: &Window, i: usize, n: usize, previous: Option<&str>, cancel: &AtomicBool) -> anyhow::Result<Digest> {
    let messages = [Message::new(Role::System, prompts::render("outline/system.j2", ())), Message::new(Role::User, prompt(window, i, n, previous))];
    let request = ChatRequest {
        model: model.to_string(),
        messages: messages.iter().map(|m| ChatMessage::new(m.role.into(), m.content.clone())).collect(),
        max_tokens: 800,
        temperature: 0.0,
        json_schema: Some(serde_json::from_str(DIGEST_SCHEMA)?),
        no_repeats: true,
        n_ctx: Some(OUTLINE_CTX),
        device: Some(device),
        priority: Some(Priority::Background),
        hf_token: hf_token.map(str::to_string),
    };
    let text = engine.chat(&request, cancel, &mut |_| true)?.content;
    let mut d: Digest = serde_json::from_str(text.trim()).map_err(|e| anyhow::anyhow!("the model's digest is not valid: {e}"))?;
    d.title = d.title.trim().trim_end_matches('.').to_string();
    Ok(d)
}

pub struct Outline {
    ai_dir: PathBuf,
    engine: Arc<dyn Engine>,
    slots: Slots,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Outline {
    pub fn new(ai_dir: &Path, engine: Arc<dyn Engine>, slots: Slots, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Outline {
        Outline { ai_dir: ai_dir.to_path_buf(), engine, slots, settings }
    }
}

impl TaskHandler for Outline {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(Transcript::exists(&self.ai_dir, &id), "{} has no transcript", input.path.display());
        let t = Transcript::open(&self.ai_dir, &id)?;
        let next = task.cursor.as_u64().unwrap_or(0) as usize;
        if next == 0 {
            t.clear_chapters()?;
        }
        let settings = (self.settings)();
        // Provider models write outlines once the agent talks to providers (P4).
        let model = settings.models.agent.as_ref().and_then(|source| crate::models::local_spec(source).ok());
        let Some(model) = model else { return Ok(json!({ "chapters": 0, "skipped": "the agent model is not local" })) };
        let names = t.speaker_names()?;
        let dialog = t.dialog()?;
        let language = outline::language(&dialog.iter().take(20).map(|r| r.text()).collect::<Vec<_>>().join(" "));
        let windows = outline::windows(&dialog, outline::WINDOW_WORDS, &names, |s| outline::unnamed_speaker(language, s));
        let n = windows.len();
        let device = settings.devices.get(ModelRole::Agent);
        let hf_token = crate::assistant::hf_token(&settings);
        let cancel = cp.stopping();
        let _lease = self.slots.request(WorkKind::Indexing, false).wait(cancel)?;
        let mut previous = t.chapters()?.last().map(|c| c.digest.title.clone());
        for (i, w) in windows.iter().enumerate().skip(next) {
            let started = std::time::Instant::now();
            let d = digest(self.engine.as_ref(), &model, device, hf_token.as_deref(), w, i, n, previous.as_deref(), cancel)?;
            log::info!("outline of {}: chapter {}/{n} in {:.1} s", input.path.display(), i + 1, started.elapsed().as_secs_f32());
            previous = Some(d.title.clone());
            t.put_chapter(i, &Chapter { start: w.start, end: w.end, digest: d })?;
            cp.save(&json!(i + 1), &Value::Null, (i + 1) as f32 / n.max(1) as f32)?;
        }
        Ok(json!({ "chapters": n }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Models are required (`MEDIAA_TEST_MODELS`, the app's folder by default); the default agent
    /// model is downloaded on first use (~2.5 GB). It digests a short meeting: a title, the
    /// decision and the action items with names.
    #[test]
    #[ignore]
    fn digests_a_meeting() {
        let engine = crate::live::engine();
        let model = match ModelRole::Agent.default_source().unwrap() {
            mediaa_core::agent::ModelSource::HuggingFace { repo } => repo,
            other => panic!("{other:?}"),
        };
        let text = "[00:00] Дана: Давайте по отчёту для заказчика. Олег, где мы?\n\
                    [00:05] Олег: Цифры за квартал собраны, не хватает графиков по продажам.\n\
                    [00:12] Дана: Графики сделает Ира до четверга. Отчёт отправляем в пятницу утром, решено.\n\
                    [00:20] Ира: Хорошо, сделаю. А презентацию для созвона кто готовит?\n\
                    [00:26] Дана: Пока не знаю, обсудим в понедельник.\n\
                    [00:30] Олег: Я ещё проверю цифры по региону Север, там были вопросы.";
        let w = Window { start: 0.0, end: 35.0, text: text.into() };
        let d = digest(engine.as_ref(), &model, Device::Auto, None, &w, 0, 1, None, &AtomicBool::new(false)).unwrap();
        eprintln!("{d:#?}");
        assert!(!d.title.is_empty() && d.title.split_whitespace().count() <= 12, "{}", d.title);
        assert!(!d.summary.is_empty());
        assert!(d.decisions.iter().any(|x| x.to_lowercase().contains("пятниц")), "{:?}", d.decisions);
        assert!(d.actions.iter().any(|x| x.contains("Ира")), "{:?}", d.actions);
    }

    #[test]
    fn prompts_carry_the_part_and_its_neighbour() {
        let w = Window { start: 600.0, end: 1200.0, text: "[10:00] Дана: начнём.".into() };
        let p = prompt(&w, 1, 3, Some("Планы на квартал"));
        assert!(p.starts_with("Part 2 of 3 of the recording (10:00 – 20:00).\nWrite everything in Russian.\n"), "{p}");
        assert!(p.contains("previous part was about: Планы на квартал"));
        assert!(p.contains("[10:00] Дана: начнём."));
        assert!(!prompt(&w, 0, 1, None).contains("previous"));
    }
}
