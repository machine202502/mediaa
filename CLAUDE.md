# Rules for AI assistants

MediaA is in active development and has no released versions yet.

## How we work

Two roles. The **lead** is the model the user talks to (any model): it thinks the work
through, designs the solution, writes self-contained briefs and reviews the results (diff,
tests). **Subagents** implement; the lead decides which subagent model gets which task. The
goal is to save the user's money: the lead does simple things itself when that is cheaper than
a brief and a review, and otherwise picks the cheapest subagent that will do the task well.
Subagent models and how to run them: [workflow/dev-context.md](workflow/dev-context.md), «Как работаем».
How to be the lead (model, limits, commits, notifications): [workflow/lead.md](workflow/lead.md).

## Documents (they reference each other)

- [workflow/todos.md](workflow/todos.md): only what is left to do — open tasks and ideas; the user picks what to take.
  A taken item is marked «взято в работу» with its batch; a postponed one stays with
  «Отложено (date): reason» (the user may come back to it).
- [workflow/done.md](workflow/done.md): finished tasks and rejected ideas; do not redo them. A task
  that is finished or rejected is removed from todos.md and recorded here.
- [workflow/dev-context.md](workflow/dev-context.md): reference for working on the project (build,
  tests, code map, test data, workflow, pitfalls).
- [workflow/lead.md](workflow/lead.md): how to be the lead. Tool-specific steps (which limit command,
  which notification tool) live there, not here.
- [workflow/process.md](workflow/process.md): what is in progress now, **only as tables** — one
  table per batch of work, batches one after another (newest last), columns «Шаг | Что |
  Исполнитель | Состояние | Лог»; no prose blocks. It is a growing log: nothing is ever removed,
  finished batches stay (their rows say «готово», the commit is a row), new ones are appended at the
  end; only the limits table at the top is updated in place. Every line of work is a row:
  subagent runs, the lead's own steps (design, reviews, live checks) and questions
  waiting for the user. The lead updates it whenever work starts or finishes. Format and the
  limits table: [workflow/lead.md](workflow/lead.md).
- **Closing a task is part of the task, never an offer.** When a task is finished, before
  reporting it or committing: record it in done.md under «Сделано» (rejected ideas go under
  «Отклонено»), remove it from todos.md, mark its rows done in process.md. After the commit, add
  the hash to its done.md entry and a «Коммит» row with the hash to its batch in process.md.

## Rules

- **Never push until the user says so.** `git push` (and `--force`, tags, PRs) only on an
  explicit word from the user for the named branch. Permission for one push never covers the
  next. Finished work stays local until then; the same goes for commits — see
  [workflow/lead.md](workflow/lead.md), «Коммиты и пуш». The repository's **git hooks**
  (`.githooks/`, installed by `node scripts/install-git-hooks.mjs`) stop every `git commit`,
  `push`, `rebase` and `merge` until the user answers «да» to one question about one exact
  branch and commit; one «да» covers that one call. `--no-verify` and disabling the hooks are
  forbidden.
- **No backward compatibility.** Do not add settings versions, migrations, fallbacks for old
  config fields, deprecated aliases, re-exports "for compatibility" or transitional shims.
  Change the target logic directly; old settings files and data may simply break and be
  regenerated with defaults.
- When a feature, field, key or file becomes unused, delete it (code, locale keys in every
  `crates/app/locales/*.toml`, tests, docs) in the same change.
- Prefer mature third-party crates over hand-written code.
- Keep everything covered by tests; run `cargo test --workspace` and
  `cargo clippy --workspace --all-targets` before finishing.
- **No comments in code of any kind** (Rust, scripts, TOML and other configs):
  names and tests carry the meaning. Explanations of decisions go to `workflow/*.md`, and only
  when they will be needed later. `cargo test --workspace` enforces it
  (`crates/core/tests/no_comments.rs`). A comment stays only where it is a part of the
  construct, i.e. explains what the code cannot show by itself:
  - `// SAFETY: …` right before an `unsafe` block or impl, and the `# Safety` section of the
    doc comment of an `unsafe fn`/`unsafe trait` (clippy `undocumented_unsafe_blocks`,
    `missing_safety_doc`);
  - a comment that is the only content of an empty block (`catch {}`, `{}`, `_ => {}`,
    `Err(_) => {}`, `Ok(_) | Err(_) => {}`) or that stands right before/after a deliberately
    ignored result (`let _ = …;`, `.ok();`, `.unwrap_or_default()` used to swallow an error)
    and says why it may be ignored;
  - tool directives that are comments by syntax: `#!` shebang, `# shellcheck …`,
    `// eslint-…`, `// @ts-…`, `// biome-ignore …`, `# noqa`, `# type: ignore`.
  A lint suppression in Rust is not a comment: move the reason into the attribute —
  `#[allow(lint, reason = "…")]` / `#[expect(lint, reason = "…")]`.
