# MediaA Agent — architecture and plan

Related: open tasks and ideas — [todos.md](todos.md); finished and rejected — [done.md](done.md);
working context (build, tests, workflow) — [dev-context.md](dev-context.md).

The Agent section analyses audio and video in catalogs: transcription, speaker naming, search
and a chat with one agent that answers questions about a recording. From the earlier
media-analysis project only the UX is taken (studio layout, transport, panes); the agent, its
tools, memory and indexing are designed from scratch for this app; they work with small local
models as well as large local or remote ones (see 7).

## 1. What the user sees

| Place | Behaviour |
|---|---|
| Agent → Catalogs | Tree of audio / video (with sound). A file opens as a studio tab: Dialog · Video · Chat panes, transport bar. |
| Studio → «Transcribe» | Starts transcription; diarization is an option (only offered when a diarization model is set). Progress in the Queue and in the Dialog pane. |
| Dialog pane | Replies with time, speaker, words highlighted while playing; click → seek. Speakers can be renamed (names are per file). |
| Chat pane | Several chats per file (tabs, «+», close, history, delete), each with its own context. Markdown answers. The agent's reply appears at once as «thinking…» with live step titles; details expand. Long answers arrive as several messages. |
| Agent → Queue | Every background job (transcription, indexing, titles…) and what it waits for (a model, memory, a task slot) — round-robin order, progress, checkpoints, pause/resume. |
| Agent → Budget | Memory budget for models (GPU VRAM, RAM), concurrency limits per kind of work (agent answers, transcription, indexing, web search), detected GPUs. |
| Agent → Models | Model per role + preferred device (Auto / GPU / CPU). |

## 2. Runtimes

* **LLMs** (agent, sub-agent, titles): llama.cpp through `llama-cpp-2`, GGUF models. GPU via the
  **Vulkan** backend (NVIDIA and AMD, one build, no CUDA/ROCm installs), CPU fallback.
  Constrained JSON output (GBNF from a JSON schema) for tool calls — small models never emit
  broken calls.
* **Speech, diarization**: sherpa-onnx (official `sherpa-onnx` crate, static, ONNX Runtime
  inside, CPU): Silero VAD, GigaAM (NeMo CTC / transducer in the sherpa-onnx layout),
  pyannote segmentation + speaker embeddings. Text embeddings (P3) go the same way.
* Model files: Hugging Face downloads pick *one* runnable file set (a GGUF quantisation such as
  `Q4_K_M`, or the ONNX files) — defaults move to GGUF / ONNX repositories.

## 3. Budget and the model scheduler (`mediaa_agent::scheduler`)

Resources: a **GPU memory pool** (the user's cap, e.g. 8 GB), a **RAM pool** for CPU models,
and **slots per kind of work** (e.g. web search ≤ 2 at once, transcription ≤ 1).

* A job asks for a *lease* on a model: `{model, size estimate, device preference}`.
* Placement: preferred GPU → GPU pool if it fits (evicting *idle* models first); otherwise CPU
  pool (Auto / GPU fall back to CPU, «CPU only» never uses the GPU). If neither fits the job
  waits in line.
* A model stays loaded only while leased; with no lease and no waiting job for it, it is
  **unloaded at once**.
* **Round robin**: waiting jobs form one FIFO line. Jobs run in *steps* (a chunk of audio, one
  agent turn…) and checkpoint after each (the queue's cursor). When others wait for memory, a
  lease older than its *quantum* is asked to yield: the job checkpoints, releases the model,
  goes to the back of the line; the next job's model gets the memory. A big model can never
  hold memory forever; a job never loses work (it continues from its cursor).
* Interactive work (an answer in an open chat) is placed before background jobs, still under
  the same budget.
* The Queue page shows, per job: running / waiting for «model X (3.2 GB)» / waiting for a slot.

## 4. Media pipeline (queue tasks, all resumable)

1. **Audio**: decode to 16 kHz mono, voice-activity chunks (Silero VAD ONNX) of ≤ 30 s.
2. **Transcribe** chunk by chunk (cursor = next chunk), words with timestamps.
3. **Diarize** (optional): speaker embeddings per voiced region, clustering; merge with words.
4. **Speakers**: anonymous labels, then names proposed from the transcript by the agent model
   (people addressing each other); the user can rename; names are stored per file.
5. **Index** (see 6).

Everything for a file lives in `.ai/media/<file-id>.db` (segments, words, speakers, chunks,
embeddings, chats).

## 5. Indexing (retrieval for small models)

* Segments → **chunks** of ~60–90 s of speech with ~15 % overlap, cut at sentence ends, each
  carrying its time range and speakers.
* Embeddings of chunks (e5: `passage:` / `query:` prefixes), cosine search in SQLite (brute
  force is fast for one file; an ANN index only if needed).
* **Hybrid search**: embedding similarity + keyword (SQLite FTS5) merged by reciprocal rank
  fusion — names, numbers and rare terms are found even when embeddings miss them.
* **Outline**: chapter cards (title, 1–2 sentence summary, time range) built by the small model
  window by window — the agent's map of a long recording.
* *Focused scan* (successor of media-analysis' «make index»): for questions that need the whole
  recording («every time X is mentioned», «all decisions»), the model checks every chunk
  against an instruction and keeps matching facts with times (map → collect). It is the only
  reliable way for a small model to answer exhaustive questions, so it is kept as a tool; its
  results are cached per instruction.

## 6. The agent

One agent, a tool-calling loop (plan → act → observe) that stays reliable down to 1–8B models
and uses the room larger models give (see 7):

* **Structured steps**: each model turn is a JSON object (constrained by grammar): either tool
  calls or a message to the user. No free-form parsing.
* **Tools** (small, typed, with clear errors the model can recover from):
  * recording: `outline`, `search` (hybrid), `read` (time range, bounded), `scan` (focused
    scan), `speakers`, `rename_speakers`;
  * exact work the model must not do in its head: `calculator` (expressions),
    `time` (timecode ↔ seconds, durations between times);
  * outside: `web_search`, `web_read` (page → text) — limited by the budget's web-search slots;
  * memory: `remember` (fact → chat memory), `recall` (search memory).
  * `say` — sends a message to the user while working (long answers come as several
    messages).
* **Grounding**: answers cite times (`[12:30]`), the UI turns them into seek links.
* **Step titles**: each tool call gets a short title from the titles model («Ищу, где говорили
  о бюджете») shown under «thinking…»; full calls / results in the details.
* **Limits**: max steps per answer, per-tool call caps, total tokens; on exhaustion the agent
  answers with what it has and says so.

### Memory and compaction

* Chat history is stored in full (SQLite). The prompt is assembled each turn:
  system + tool specs → pinned memory (facts saved with `remember`, speaker names) → running
  summary of older turns → the last turns verbatim → the question.
* Tool results are large; in history they are kept as short *stubs* (tool, arguments, a
  2–3-line digest, a reference) — the full result can be re-read if needed.
* **Auto-compaction**: when the assembled prompt passes ~60 % of the model's context, the
  oldest verbatim turns are folded into the summary (by the titles / sub-agent model),
  keeping decisions, facts with times, open questions and user preferences.

## 7. Revisions (agreed, not implemented yet)

### Any model, small or large, local or remote

* The agent is **model-agnostic**: a *model profile* (context window, native tool calling or
  not, speed class) drives its behaviour instead of assuming a small model.
  * Providers (OpenRouter, OpenAI-compatible, Hugging Face router): native OpenAI `tools`
    calls with JSON-schema validation and one retry on a malformed call.
  * Local llama.cpp: the same tool schema enforced by a grammar.
* Prompt and read sizes scale with the context window: how much transcript one `read` returns,
  how many search hits, when compaction starts. A 128k-context model reads whole chapters, a
  4k one reads paragraphs — same tools, different budgets.

### Budget for provider calls

* New kind of work **«provider calls»**: how many requests to remote models run at once
  (default 4), with an optional per-provider override. Remote calls take no GPU / RAM from the
  pools, only these slots; the scheduler treats them like any other slot.

### Long recordings, long answers

* The agent never needs the whole recording in its context: it navigates by the outline, reads
  bounded ranges, uses search and the focused scan.
* **Long answers are written in parts**: for a large answer the agent first fixes an outline
  (its sections), then writes and sends each section as its own message (`say`), each fitting
  the output budget. Sections already sent are kept as short stubs in the context, so a report
  longer than the context still comes out whole and consistent.

### No useless repeats

* Every tool call has a **fingerprint** (tool + normalised arguments + the data version it
  reads). A call with a fingerprint already done in this answer is not executed again: the
  agent gets «already done at step N, the result was the same: …» and a hint what to do instead.
* **Novelty check**: a result whose content adds nothing new (all its fragments were already
  seen) is reported as «nothing new».
* **Stall guard**: several steps in a row without new information → the agent is told to
  answer with what it has (or to change approach once); hard step / token limits stay.

### Plan, sub-agents, verification

* **Todo plan** (tool `plan`): for multi-step requests the agent writes a short checklist, ticks
  items off, can revise it; the plan is shown in the answer's details and kept in the context
  (it survives compaction). Simple questions skip it.
* No standing hierarchy of manager / workers / checker agents (more calls, more ways to drift,
  slower on local models). Instead, one orchestrating agent that can **delegate** a bounded
  sub-task to the sub-agent model (`delegate`: own short context, returns a compact result)
  — e.g. «summarise 40:00–55:00», map steps of the focused scan, in parallel within the budget.
* **Verification**: every claim with a time citation is checked mechanically against the
  transcript (the cited range exists and contains the quoted words); failed citations go back
  to the agent once to fix. For long reports a final self-check pass (sub-agent model) compares
  the report with the notes it was built from.

### What the agent knows about the recording

A *recording card* at the top of the context: title, duration and a size class (short /
medium / long), language, speakers with names (or «not named»), whether diarization was done,
index state (outline, chapters count, searchable), the user's current playback position, and
the chat's pinned facts.

### Other

* API keys move from `settings.toml` to the OS keychain (`keyring`); settings keep only a
  reference.
* Speech recognition keeps GigaAM as the default; the user switches models if needed.

## 8. Ideas from AnythingLLM and OpenTranscribe (proposed)

Studied for ideas only; nothing is copied.

### Retrieval (P3) — from OpenTranscribe

* **Route, don't fuse.** Different kinds of questions use different sources, never mixed into
  one ranking: *lookup* (who said what → transcript chunks), *summary* (precomputed chapter
  digests), *count* (talk time per speaker, how often a word / topic comes up, first / last
  mention — computed by code from the database, never by the model), *time* («в начале»,
  «после 30-й минуты» → a time filter). The chunk search always runs too, so a misread
  question still finds evidence.
* **Chunks by speaker turns**: consecutive turns of one speaker, long turns split with a
  sliding overlap (~200 words / 40 overlap) — better units than fixed seconds.
* **Small-to-big at read time**: a short hit («Да», «Согласен») is widened with its
  neighbouring replies before it reaches the model — it may be the whole answer, but needs
  context.
* **Cross-encoder reranker** after hybrid search (a small multilingual ONNX model, CPU) plus
  **diversity sampling** (not five hits from the same minute). The single biggest quality step
  for retrieval.
* **Digests at ingest**: per chapter — summary, decisions, action items, open questions,
  topics. Summary questions are answered from them cheaply and consistently.
* **Coverage check** for whole-recording work (focused scan, summaries): every chunk is either
  processed or listed with a reason; the answer states gaps instead of sounding complete.
* **Speakers in the question**: a name typed in the question («что Дана говорила о сроках»)
  adds a parallel search limited to that speaker (never a silent hard filter); an ambiguous
  name → the agent asks.
* **Follow-up rewriting**: «а что она сказала про сроки?» → a standalone query with the name
  and topic resolved from the chat, before searching.
* **Citations select, never construct**: `[n]` markers can only point at excerpts actually
  given to the model; invalid markers are dropped.
* **Honest trace**: each step reports *found / empty / skipped / failed* — «nothing found» and
  «search broke» never look the same in the details.

### Speech (P2) — from OpenTranscribe

* **Voiceprints across recordings**: the speaker embedding of every named speaker is saved;
  in a new recording, matching voices get the name suggested (or set above a confidence).
* **Custom vocabulary**: names and terms the user adds bias recognition / correction.

### The agent (P4) — from AnythingLLM

* **Models without native tool calling**: tool descriptions with few-shot examples in the
  prompt, JSON parsed and validated, one corrective retry — every tool ships examples.
* **Duplicate guard** beyond exact repeats: per-tool cooldowns and «once per answer» tools
  (complements the fingerprints of 7).
* **Tool selection**: when many tools exist, only the most relevant (embedding similarity to
  the request) go into the prompt — keeps small contexts free.
* **Context manager**: the reply always keeps ≥ 20 % of the window; priorities question >
  evidence > history; the last three exchanges are kept; over-long items are cut in the
  middle, not dropped.
* **Model routing with fallback**: rules send light requests (titles, short lookups) to the
  small model and heavy ones to the large / remote model; a provider that fails or rate-limits
  is cooled down and the request goes to the next model.
* **Ask the user**: a tool to ask a clarifying question instead of guessing.

## 9. Phases

| Phase | Content |
|---|---|
| P0 ✅ | Budget settings + page, device preference per model, GPU detection, scheduler core (leases, round robin, slots) with tests, `calculator` and `time` tools. |
| P1 ✅ | llama.cpp with Vulkan (any GPU with a driver; the loader is delay-loaded, so the app starts without it) and the CPU (static CRT), model loading through the scheduler (`runtime.rs`), GGUF defaults, real model check (load + one token), «provider calls» budget, API keys in the OS keychain (`core::secrets`), VRAM on the Budget page, model memory on the Queue page. ONNX Runtime moved to P2 with its first model. |
| P2 (mostly done) | ✅ sherpa-onnx (static, ONNX Runtime inside): Silero VAD, GigaAM v3 with punctuation, pyannote segmentation + WeSpeaker embeddings; `transcribe` queue task resumable phrase by phrase (`agent::transcribe`), transcript per file by content id (`core::transcript`, `.ai/media/<id>.db`); «Transcribe» and the Dialog pane in the studio (replies by speaker, word highlight while playing, click to seek, speaker names); the queue worker runs from app start. Left: voiceprints across recordings, names proposed by the agent model (P4), custom vocabulary. |
| P3 ✅ | ✅ chunks along speaker turns (~200 words, overlap for long turns), FTS5 over Snowball stems (ru / en), embeddings through llama.cpp (EmbeddingGemma 300M GGUF, GPU), hybrid search (RRF + diversity) — `core::retrieval`; `index` queue task after every transcription; search in the studio's Dialog pane; reranker role (BGE reranker v2 M3 GGUF through llama.cpp) re-orders 24 candidates (`agent::search`); outline: chapter digests (title, summary, decisions, action items, open questions, topics) by the agent model as constrained JSON, window by window (`core::outline`, `agent::outline` task after indexing), shown as chapter headings in the Dialog pane.; search over a catalog folder (`agent::search::search_files`: one query embedding, candidates per file reranked together) from the catalogs side bar. Focused scan and coverage move to P4 as agent tools. |
| P4 ✅ | ✅ agent loop (`agent::assistant`): grammar-constrained JSON steps with an explicit `action` (local) or native tool calls with one corrective retry (providers, «provider calls» slots); the first step always looks into the recording; tools outline / search / read / mentions (exact counts and times by code) / speakers / calculator / time; fingerprints, novelty, stall guard, step limit; citations checked against the transcript; old results shortened to fit the context; chats per recording (`core::chat`), the studio's Chat pane (Markdown, citations seek, steps in the details); outlines written in the recording's language; answers stream as they are written (the `answer` field out of the step JSON for local models, SSE for providers) with a loop guard (DRY sampler + cutting a repeated paragraph); the focused scan tool (`assistant::scan`: every ~900-word part read against an instruction by the agent's model, findings with checked times, coverage — unread parts named —, complete scans cached per instruction, progress in the chat).; model profiles (the local agent context in settings, each provider's context); chat memory (`remember`) and history folded into a summary when it outgrows 30 % of the context (`assistant::memory`); `plan` checklist; `say` sections of long answers (bounded by the grammar, cited or refused, then a one-line close); `rename_speakers`; `delegate` (a stretch read whole by the sub-agent: a provider's model or the agent's own); a clarifying question instead of a guess; a chat over a catalog folder (`assistant::folder`: the folder card with each recording's chapters, one search across all, per-recording tools, citations `[#N m:ss]` that open the recording). |
| P5 ✅ | ✅ web search and page reading (`agent::web`, `assistant::internet`): DuckDuckGo HTML by default (no key; may ask for a captcha), SearXNG (JSON), Brave or Tavily (key in the OS keychain), an optional proxy; pages reduced to their main text as lean Markdown (dom_smoothie); `web_search` / `web_read` tools only when enabled, one call at a time through the scheduler's web slot; links in answers and sections kept only to pages the agent was shown; the «Web search» card on the Providers page with a test search. The Queue page shows the budget in use: memory bars (GPU, RAM), busy and waiting slots per kind of work; a system symbol font as fallback for file names. |
