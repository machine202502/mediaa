# Rules for AI assistants

MediaA is in active development and has no released versions yet.

- **No backward compatibility.** Do not add settings versions, migrations, fallbacks for old
  config fields, deprecated aliases, re-exports "for compatibility" or transitional shims.
  Change the target logic directly; old settings files and data may simply break and be
  regenerated with defaults.
- When a feature, field, key or file becomes unused, delete it (code, locale keys in every
  `crates/app/locales/*.toml`, tests, docs) in the same change.
- Prefer mature third-party crates over hand-written code.
- Keep everything covered by tests; run `cargo test --workspace` and
  `cargo clippy --workspace --all-targets` before finishing.
