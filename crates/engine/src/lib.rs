//! The model engine of MediaA: local language models, embeddings, reranking and speech (llama.cpp
//! and sherpa-onnx), Hugging Face model downloads and the memory budget models are loaded under.
//!
//! It runs inside the app's process: [`Engine`] is what the rest of the app talks to, and
//! [`LocalEngine`] is the implementation that runs the models.

pub mod embed;
pub mod engine;
pub mod hub;
pub mod llm;
pub mod local;
pub mod runtime;
pub mod scheduler;
pub mod speech;
#[cfg(feature = "testing")]
pub mod testing;

pub use engine::*;
