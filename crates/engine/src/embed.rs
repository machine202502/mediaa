//! Text embeddings and reranking through llama.cpp, on the GPU when there is one. Embedding
//! models (EmbeddingGemma, Qwen3-Embedding, BGE-M3, e5…): queries and passages get the prompts
//! their family was trained with; vectors are L2-normalised, so a dot product is the cosine.
//! Rerankers (cross-encoders: BGE reranker, Qwen3-Reranker) score a query against a passage.

use crate::llm::backend;
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use std::num::NonZeroU32;
use std::path::Path;

/// Longest text embedded at once (tokens); longer ones are cut.
const MAX_TOKENS: u32 = 2048;

/// What goes before a query and before a passage for a model family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompts {
    pub query: &'static str,
    pub passage: &'static str,
}

/// Prompts by the model's architecture (GGUF `general.architecture`) and name.
pub fn prompts(architecture: &str, name: &str) -> Prompts {
    let name = name.to_ascii_lowercase();
    if architecture.contains("gemma") && name.contains("embedding") || architecture == "gemma-embedding" {
        Prompts { query: "task: search result | query: ", passage: "title: none | text: " }
    } else if name.contains("qwen3") && name.contains("embedding") {
        Prompts { query: "Instruct: Given a question, find the parts of a conversation that answer it\nQuery: ", passage: "" }
    } else if name.contains("e5") {
        Prompts { query: "query: ", passage: "passage: " }
    } else {
        Prompts { query: "", passage: "" }
    }
}

/// `v` scaled to length 1 (unchanged when zero).
pub fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

/// Dot product (cosine of normalised vectors).
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// A loaded embedding model.
pub struct Embedder {
    model: LlamaModel,
    prompts: Prompts,
    n_ctx: u32,
    gpu: bool,
}

impl Embedder {
    pub fn load(path: &Path, gpu: bool) -> anyhow::Result<Embedder> {
        anyhow::ensure!(path.is_file(), "{} not found", path.display());
        let params = LlamaModelParams::default().with_n_gpu_layers(if gpu { 999 } else { 0 });
        let model = LlamaModel::load_from_file(backend()?, path, &params).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let architecture = model.meta_val_str("general.architecture").unwrap_or_default();
        let name = format!("{} {}", model.meta_val_str("general.name").unwrap_or_default(), path.file_name().unwrap_or_default().to_string_lossy());
        let n_ctx = MAX_TOKENS.min(model.n_ctx_train().max(128));
        log::info!(
            "embedding model {}: architecture {architecture}, n_ctx_train {}, n_ctx {n_ctx}, {} layers on the GPU",
            path.display(),
            model.n_ctx_train(),
            if gpu { "all" } else { "none" }
        );
        Ok(Embedder { prompts: prompts(&architecture, &name), model, n_ctx, gpu })
    }

    pub fn on_gpu(&self) -> bool {
        self.gpu
    }

    /// Length of the vectors.
    pub fn dimensions(&self) -> usize {
        self.model.n_embd_out().max(0) as usize
    }

    pub fn query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.embed(&format!("{}{text}", self.prompts.query))
    }

    pub fn passage(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.embed(&format!("{}{text}", self.prompts.passage))
    }

    fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut tokens = self.model.vocab().tokenize(text.as_bytes(), true, false);
        tokens.truncate(self.n_ctx as usize);
        anyhow::ensure!(!tokens.is_empty(), "nothing to embed");
        // Non-causal models see the whole text in one micro-batch.
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.n_ctx))
            .with_n_batch(self.n_ctx)
            .with_n_ubatch(self.n_ctx)
            .with_embeddings(true);
        log::debug!(
            "embed: {} tokens ({} chars), context n_ctx {}, n_batch {}, n_ubatch {}",
            tokens.len(),
            text.chars().count(),
            self.n_ctx,
            self.n_ctx,
            self.n_ctx
        );
        let mut ctx = self.model.new_context(backend()?, params)?;
        log::debug!("embed: context created");
        let mut batch = LlamaBatch::new(tokens.len(), 1);
        batch.add_sequence(&tokens, 0, true)?;
        // The decode is one long native call; a beat before and after it shows the watchdog
        // that the call is alive (see `mediaa_core::pulse`).
        mediaa_core::pulse::beat();
        ctx.decode(&mut batch)?;
        mediaa_core::pulse::beat();
        log::debug!("embed: decoded");
        let v = ctx.embeddings_seq_ith(0).map_err(|e| anyhow::anyhow!("not an embedding model: {e}"))?;
        Ok(normalized(v.to_vec()))
    }
}

/// How a query and a document go into a cross-encoder.
#[derive(Debug, Clone, PartialEq)]
enum RerankFormat {
    /// The model's own `rerank` template with `{query}` and `{document}` (Qwen3-Reranker).
    Template(String),
    /// `[BOS] query [EOS] [SEP] document [EOS]` (BERT / XLM-R rerankers such as BGE).
    Pair,
}

/// A loaded reranker (cross-encoder): scores how well a passage answers a query.
pub struct Reranker {
    model: LlamaModel,
    format: RerankFormat,
    n_ctx: u32,
    gpu: bool,
}

/// The rerank prompt of a template.
pub fn rerank_prompt(template: &str, query: &str, document: &str) -> String {
    template.replace("{query}", query).replace("{document}", document)
}

impl Reranker {
    pub fn load(path: &Path, gpu: bool) -> anyhow::Result<Reranker> {
        anyhow::ensure!(path.is_file(), "{} not found", path.display());
        let params = LlamaModelParams::default().with_n_gpu_layers(if gpu { 999 } else { 0 });
        let model = LlamaModel::load_from_file(backend()?, path, &params).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let format = match model.chat_template(Some("rerank")) {
            Ok(t) => RerankFormat::Template(t.as_c_str().to_string_lossy().into_owned()),
            Err(_) => RerankFormat::Pair,
        };
        let n_ctx = MAX_TOKENS.min(model.n_ctx_train().max(128));
        Ok(Reranker { model, format, n_ctx, gpu })
    }

    pub fn on_gpu(&self) -> bool {
        self.gpu
    }

    /// Relevance of `document` to `query` (higher is better; not bounded).
    pub fn score(&self, query: &str, document: &str) -> anyhow::Result<f32> {
        let vocab = self.model.vocab();
        let mut tokens = match &self.format {
            RerankFormat::Template(t) => vocab.tokenize(rerank_prompt(t, query, document).as_bytes(), true, true),
            RerankFormat::Pair => {
                let mut t = vec![vocab.bos()];
                t.extend(vocab.tokenize(query.as_bytes(), false, false));
                t.extend([vocab.eos(), vocab.sep()]);
                t.extend(vocab.tokenize(document.as_bytes(), false, false));
                t.push(vocab.eos());
                t
            }
        };
        tokens.truncate(self.n_ctx as usize);
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.n_ctx))
            .with_n_batch(self.n_ctx)
            .with_n_ubatch(self.n_ctx)
            .with_embeddings(true)
            .with_pooling_type(LlamaPoolingType::Rank);
        let mut ctx = self.model.new_context(backend()?, params)?;
        let mut batch = LlamaBatch::new(tokens.len(), 1);
        batch.add_sequence(&tokens, 0, true)?;
        mediaa_core::pulse::beat();
        ctx.decode(&mut batch)?;
        mediaa_core::pulse::beat();
        let out = ctx.embeddings_seq_ith(0).map_err(|e| anyhow::anyhow!("not a reranker: {e}"))?;
        out.first().copied().ok_or_else(|| anyhow::anyhow!("not a reranker: no score"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BGE reranker (~440 MB) from `MEDIAA_TEST_AI` or downloaded: the passage that answers
    /// scores above one that only shares words with the question.
    #[test]
    #[ignore]
    fn reranker_prefers_the_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = std::env::var_os("MEDIAA_TEST_AI").map_or(tmp.path().to_path_buf(), std::path::PathBuf::from);
        let Some(source @ mediaa_core::agent::ModelSource::HuggingFace { .. }) = mediaa_core::agent::ModelRole::Reranker.default_source() else { panic!() };
        let mediaa_core::agent::ModelSource::HuggingFace { repo } = &source else { unreachable!() };
        crate::hub::download(&ai, repo, None, |_, _| {}).unwrap();
        let r = Reranker::load(&crate::runtime::gguf_path(&ai, &source).unwrap(), true).unwrap();
        let q = "когда нужно сдать отчёт?";
        let answer = r.score(q, "Дана: отчёт отправляем заказчику в пятницу утром.").unwrap();
        let words = r.score(q, "Олег: отчёт о поездке на дачу я так и не написал, там было не до того.").unwrap();
        let off = r.score(q, "Погода была отличная, сажали картошку.").unwrap();
        eprintln!("{answer} {words} {off}");
        assert!(answer > words && words > off, "{answer} {words} {off}");
        assert_eq!(rerank_prompt("<q>{query}</q><d>{document}</d>", "a", "b"), "<q>a</q><d>b</d>");
    }

    #[test]
    fn prompts_by_family() {
        assert_eq!(prompts("gemma-embedding", "embeddinggemma-300M-Q8_0.gguf").query, "task: search result | query: ");
        assert_eq!(prompts("qwen3", "Qwen3-Embedding-0.6B").passage, "");
        assert_eq!(prompts("bert", "multilingual-e5-small").passage, "passage: ");
        assert_eq!(prompts("xlm-roberta", "bge-m3"), Prompts { query: "", passage: "" });
    }

    #[test]
    fn unit_vectors() {
        let v = normalized(vec![3.0, 4.0]);
        assert!((similarity(&v, &v) - 1.0).abs() < 1e-6);
        assert_eq!(normalized(vec![0.0, 0.0]), vec![0.0, 0.0]);
        assert!(similarity(&normalized(vec![1.0, 0.0]), &normalized(vec![0.0, 1.0])).abs() < 1e-6);
    }

    /// EmbeddingGemma (~330 MB) from `MEDIAA_TEST_AI` or downloaded: a question is closer to the
    /// passage that answers it than to an unrelated one, in Russian.
    #[test]
    #[ignore]
    fn related_texts_are_closer() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = std::env::var_os("MEDIAA_TEST_AI").map_or(tmp.path().to_path_buf(), std::path::PathBuf::from);
        let Some(mediaa_core::agent::ModelSource::HuggingFace { repo }) = mediaa_core::agent::ModelRole::Embedding.default_source() else { panic!() };
        crate::hub::download(&ai, &repo, None, |_, _| {}).unwrap();
        let path = crate::runtime::gguf_path(&ai, &mediaa_core::agent::ModelSource::HuggingFace { repo }).unwrap();
        for gpu in [false, true] {
            let e = Embedder::load(&path, gpu).unwrap();
            let q = e.query("когда сдаём отчёт?").unwrap();
            let yes = e.passage("Дана: отчёт нужно отправить заказчику до пятницы, иначе сдвинем релиз.").unwrap();
            let no = e.passage("Олег: вчера на даче сажали картошку, погода была отличная.").unwrap();
            assert_eq!(q.len(), e.dimensions());
            let (s_yes, s_no) = (similarity(&q, &yes), similarity(&q, &no));
            eprintln!("gpu={gpu}: {s_yes} vs {s_no}");
            assert!(s_yes > s_no + 0.05, "{s_yes} vs {s_no}");
        }
    }

    /// The models folder for the live tests: `MEDIAA_TEST_AI`, else the app's `MEDIAA_TEST_MODELS`
    /// (what `scripts/live-test.sh` sets), else a temporary folder it downloads into.
    fn live_models_root() -> std::path::PathBuf {
        std::env::var_os("MEDIAA_TEST_AI")
            .or_else(|| std::env::var_os("MEDIAA_TEST_MODELS"))
            .map_or_else(|| tempfile::tempdir().unwrap().keep(), std::path::PathBuf::from)
    }

    /// Runs `f` on its own thread and fails (instead of hanging the test run) when it does not
    /// return in `seconds`. A stuck native call keeps its thread, but the test itself ends.
    fn within<R: Send + 'static>(seconds: u64, what: &str, f: impl FnOnce() -> R + Send + 'static) -> R {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(seconds)) {
            Ok(value) => value,
            Err(_) => panic!("{what}: no result in {seconds} s, the call hangs"),
        }
    }

    /// A chunk of a spoken dialog of a little over 1200 characters (the size the first live
    /// index call embedded when it hung).
    const LONG_RUSSIAN_CHUNK: &str = "Дана: смотри, у нас на этой неделе получается довольно плотно по задачам, потому что \
релиз сдвинулся, и теперь всё, что мы планировали на следующую неделю, надо успеть сделать до пятницы. \
Олег: я понимаю, но у меня ещё висит отчёт по поездке, который я обещал закончить, и там надо переписать \
примерно половину, потому что заказчик поменял требования к структуре. Дана: а насколько это критично? \
Олег: если честно, не очень критично, но если не сделать сейчас, потом придётся возвращаться и всё равно \
переделывать, так что лучше сразу закрыть. Дана: хорошо, тогда давай так: ты заканчиваешь отчёт до среды, \
а я беру на себя интеграцию с новым сервисом, потому что там нужно постоянно общаться с их командой, \
а у меня как раз есть контакт. Олег: договорились. Ещё вопрос по тестам: те, что падали на прошлой неделе, \
они падали из-за окружения или из-за кода? Дана: часть из-за окружения, там не хватало переменной, \
а часть реально из-за кода, потому что мы поменяли формат и забыли обновить ожидания в трёх местах. \
Олег: то есть если я сейчас запущу полный прогон, он должен быть зелёным? Дана: должен, но я бы всё равно \
посмотрела на медленные тесты, потому что один из них стал идти заметно дольше, и мне кажется, что там \
появилось лишнее ожидание. Олег: ок, тогда я сначала посмотрю на этот тест, а потом уже возьмусь за отчёт. \
Дана: только не забудь, что в четверг у нас встреча с командой поддержки, и надо будет показать, как \
работает поиск по записям, потому что они жалуются, что не могут быстро найти нужный фрагмент разговора. \
Олег: да, я помню, подготовлю короткую демонстрацию на реальной записи, чтобы было видно, как находятся \
цитаты и как показываются темы. Дана: отлично, тогда на этом и закончим.";

    /// The live hang: the first embedding call of a long Russian chunk never returned on the GPU.
    /// `scripts/live-test.sh <log> mediaa-engine a_long_russian_chunk_is_embedded_without_hanging`
    #[test]
    #[ignore]
    fn a_long_russian_chunk_is_embedded_without_hanging() {
        use std::time::{Duration, Instant};
        let root = live_models_root();
        let Some(source) = mediaa_core::agent::ModelRole::Embedding.default_source() else { panic!() };
        let mediaa_core::agent::ModelSource::HuggingFace { repo } = &source else { panic!() };
        crate::hub::download(&root, repo, None, |_, _| {}).unwrap();
        let path = crate::runtime::gguf_path(&root, &source).unwrap();
        assert!(LONG_RUSSIAN_CHUNK.chars().count() > 1200, "{} chars", LONG_RUSSIAN_CHUNK.chars().count());

        // The model alone first (CPU, then the GPU), then the same call through the runtime the
        // app uses: whichever level hangs shows in the log.
        for gpu in [false, true] {
            let at = Instant::now();
            let path = path.clone();
            let (dims, norm) = within(60, &format!("Embedder::load + passage on gpu={gpu}"), move || {
                let embedder = Embedder::load(&path, gpu).unwrap();
                let v = embedder.passage(LONG_RUSSIAN_CHUNK).unwrap();
                (v.len(), v.iter().map(|x| x * x).sum::<f32>())
            });
            eprintln!("Embedder gpu={gpu}: {dims} dims, norm {norm:.4}, {:.1} s", at.elapsed().as_secs_f32());
            assert!(norm > 0.99 && norm < 1.01, "a unit vector, not {norm}");
        }
        for device in [mediaa_core::agent::Device::Cpu, mediaa_core::agent::Device::Auto] {
            let at = Instant::now();
            let (root, source) = (root.clone(), source.clone());
            let dims = within(60, &format!("Runtime::with_model on {device:?}"), move || {
                let runtime = crate::runtime::Runtime::new(&root, mediaa_core::agent::Budget::default(), Duration::from_secs(60));
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let how = crate::runtime::Use { kind: mediaa_core::agent::WorkKind::Indexing, device, n_ctx: 2048, interactive: false };
                runtime
                    .with_model::<Embedder, _>(&source, how, &cancel, |embedder, _| embedder.passage(LONG_RUSSIAN_CHUNK))
                    .unwrap()
                    .len()
            });
            eprintln!("Runtime {device:?}: {dims} dims, {:.1} s", at.elapsed().as_secs_f32());
        }

        // The index stage: one runtime, chunk after chunk, a lease per call. The keeper sweeps
        // between them; if a sweep ever fails to return, the run hangs here instead of finishing.
        let at = Instant::now();
        let (root, source) = (root.clone(), source.clone());
        let chunks = within(180, "21 chunks through one runtime", move || {
            let runtime = crate::runtime::Runtime::new(&root, mediaa_core::agent::Budget::default(), Duration::from_secs(60));
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let how = crate::runtime::Use { kind: mediaa_core::agent::WorkKind::Indexing, device: mediaa_core::agent::Device::Auto, n_ctx: 2048, interactive: false };
            let mut done = 0;
            for _ in 0..21 {
                let v = runtime.with_model::<Embedder, _>(&source, how, &cancel, |embedder, _| embedder.passage(LONG_RUSSIAN_CHUNK)).unwrap();
                if v.len() == 768 {
                    done += 1;
                }
            }
            done
        });
        eprintln!("Runtime: {chunks} chunks, {:.1} s", at.elapsed().as_secs_f32());
        assert_eq!(chunks, 21, "every chunk of the index stage got an embedding");
    }
}
