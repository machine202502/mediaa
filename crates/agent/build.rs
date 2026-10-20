//! Embeds the prompt templates of `prompts/` into the binary (read by `src/prompts.rs`).

fn main() {
    minijinja_embed::embed_templates!("prompts", &[".j2"]);
}
