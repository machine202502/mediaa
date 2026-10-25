# MediaA: project rules

MediaA is in active development and has no released versions yet. These rules apply to every
assistant working in this repository.

## Decision model

Start from the goal, constraints and observable success criteria. Separate facts from assumptions,
verify claims against the code, tests or actual behavior, and change the hypothesis when evidence
contradicts it.

Choose the simplest solution that fully handles the real risks. Do not add abstractions, settings
or infrastructure without a concrete consumer. Stop when the acceptance criteria are met.

Code is written for practical use, and its primary maintainer is an LLM rather than a human
reader. Prefer implementations that an LLM can understand and change safely from a small,
local context: explicit types, contracts and data flow; clear names; cohesive modules; bounded
state and side effects; few hidden dependencies and implicit conventions. Tests state the
expected behavior. This orientation does not justify abstractions or ceremony by itself:
additional structure is useful only when it measurably reduces context required for a change or
the likelihood of errors.

Treat your own proposal critically: look for contradictions, hidden assumptions, unnecessary
dependencies and failure paths. For important changes, check errors, cancellation, repeated runs
and boundary cases. Distinguish a blocking defect from a preference; only the defect should expand
the scope.

Size the planning effort to the task. A simple local change does not need a separate design.
Complex, ambiguous or cross-component work may need one; its format is defined in Workflow.

## Workflow

Work proceeds in this order:

1. For a complex feature or change, write a design to `workflow/plans/<name>.md` when the design
   cost is justified by the risk or uncertainty.
2. Put the resulting units of work in [workflow/tasks.md](workflow/tasks.md). Tasks have only
   four fields: `ID`, `Дата и время`, `Описание`, `Статус`. The date and time use the format
   `DD.MM.YY HH:MM`.
3. Execute tasks one at a time. Before starting, set the status to `в работе`; after verification,
   set it to `готово` or `отказ` before moving to the next task.

Allowed statuses are `ожидание`, `в работе`, `готово` and `отказ`. Tasks start in `ожидание`.
A plan is not a task list and does not replace the table.
If verification finds a problem, the task remains `в работе` until it is fixed.

The design states the goal, constraints, chosen approach, affected areas, verification and
material risks. Alternatives are included only to the extent needed to explain the choice.

## Project memory

Before non-trivial work, read [workflow/rules.md](workflow/rules.md),
[workflow/practices.md](workflow/practices.md) and [workflow/stones.md](workflow/stones.md).

- `rules.md` contains standing rules explicitly fixed by the user. The model must not promote its
  own conclusions to rules without an explicit instruction.
- `stones.md` contains verified project facts, earlier problems, constraints and durable lessons.
  Update, shorten or clear it when entries become stale.
- `practices.md` contains reusable approaches for solving recurring tasks. Update or clear it when
  a better approach is established.

Authority for project decisions is:

1. the user's current explicit instruction;
2. `rules.md`, this file and [AGENTS.md](AGENTS.md);
3. verified code, tests and observed behavior;
4. `stones.md` and `practices.md`.

The user's opinion is final. If the user rejects a stone or practice, change or remove that entry
immediately. If the user says a practice is now a rule, record it in `rules.md`. A current user
instruction overrides earlier project memory and must also be reflected in the documents when the
change is durable.

Do not leave contradictions unresolved. If `stones.md` or `practices.md` contradicts a rule or a
current user instruction, update the lower-priority memory in the same change. If the contradiction
changes required behavior and the user's intent is unclear, ask before implementing.

## Git

- Do not create, amend, rebase, merge or otherwise produce a commit without the user's explicit
  permission for that specific batch of changes. The permission applies only to that batch.
- Do not push, force-push, create tags or open pull requests without separate explicit
  permission for the named branch.
- Permission for one operation never covers the next operation.
- Before every push, verify that statuses in `workflow/tasks.md` match the actual state of the
  work, then run `cargo fmt --all`, and push only after formatting succeeds. If formatting
  changes files, do not push a stale commit: bring the permitted batch to a consistent committed
  state first.

## Code rules

- No backward compatibility: no migrations, deprecated aliases, fallback paths for old settings,
  compatibility re-exports or transitional shims. Update the target code directly.
- Delete code, locale keys, tests and documentation as soon as they become unused.
- Prefer mature, maintained crates over hand-written implementations.
- Cover changed behavior with tests. Run the relevant Rust tests and
  `cargo clippy --workspace --all-targets` when Rust code changes and its current verified state
  does not already cover the change.
- Do not rerun Rust tests when the code has not changed since the last successful test run.
  Formatting-only edits do not count as a code change.
- For documentation, configuration and script-only changes, run validation relevant to those
  files instead of the Rust test suite.
- Do not write comments in code. Names and tests carry the meaning. A comment is allowed only
  when it is structurally required:
  - `// SAFETY: …` before `unsafe`, and `# Safety` on public unsafe APIs;
  - the only explanation inside an intentionally empty or ignored branch;
  - a tool directive such as a shebang, `# shellcheck`, `eslint-…`, `@ts-…`,
    `biome-ignore`, `noqa` or `type: ignore`.
- Put a Rust lint reason in `#[allow(lint, reason = "…")]` or
  `#[expect(lint, reason = "…")]`, not in a free-standing comment.

Build, test and platform instructions are in [README.md](README.md).

Report the result first, then what was verified, the remaining risks and anything required from
the user. Do not call work complete while mandatory steps remain.

## Project skills

- `$mediaa-audit` — rational technical audit of implementation, architecture, LLM-assisted
  development and build efficiency.
- `$mediaa-complexity` — audit for unjustified safeguards, abstractions and engineering
  complexity.

Canonical skills live in `.agents/skills/`. Codex and VS Code discover them directly; the files
under `.claude/skills/` are thin discovery wrappers for Claude Code. Update the canonical
`SKILL.md`, never only a wrapper.

Audit skills report findings only; they do not authorize code changes.
