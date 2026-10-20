# Rules for AI assistants

MediaA is in active development and has no released versions yet.

## How we work

Two roles. The **lead** is the model the user talks to (any model): it thinks the work
through, designs the solution, writes self-contained briefs and reviews the results (diff,
tests). **Subagents** implement; the lead decides which subagent model gets which task. The
goal is to save the user's money: the lead does simple things itself when that is cheaper than
a brief and a review, and otherwise picks the cheapest subagent that will do the task well.
Subagent models and how to run them: [docs/dev-context.md](docs/dev-context.md), «Как работаем».

## Documents (they reference each other)

- [docs/todos.md](docs/todos.md): every task and idea we may do; the user picks what to take.
- [docs/done.md](docs/done.md): finished tasks and rejected ideas; do not redo them. A task
  that is finished or rejected is removed from todos.md and recorded here.
- [docs/dev-context.md](docs/dev-context.md): reference for working on the project (build,
  tests, code map, test data, workflow, pitfalls).
- [docs/agent.md](docs/agent.md): the agent's architecture and phases.

## Rules

- **No backward compatibility.** Do not add settings versions, migrations, fallbacks for old
  config fields, deprecated aliases, re-exports "for compatibility" or transitional shims.
  Change the target logic directly; old settings files and data may simply break and be
  regenerated with defaults.
- When a feature, field, key or file becomes unused, delete it (code, locale keys in every
  `crates/app/locales/*.toml`, tests, docs) in the same change.
- Prefer mature third-party crates over hand-written code.
- Keep everything covered by tests; run `cargo test --workspace` and
  `cargo clippy --workspace --all-targets` before finishing.
- **Prompts live in template files, never in Rust code.** Every text sent to a model (system
  prompts, task prompts, tool descriptions, corrective messages) is a MiniJinja template in
  `crates/agent/prompts/*.j2`, registered in `crates/agent/src/prompts.rs` and rendered with
  `prompts::render`. Conditions and pieces belong in the template (`{% if %}`, `{% include %}`),
  the code only passes data. Any prompt change is made in the `.j2` file and its insta
  snapshot is reviewed.
