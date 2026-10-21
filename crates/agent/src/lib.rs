//! Agent section back end: the chat loop, tools, indexing, transcription and provider checks.
//! Models run in the engine ([`mediaa_engine::Engine`]), in the app's process.

pub mod assistant;
pub mod check;
pub mod diarize;
pub mod index;
#[cfg(test)]
mod live;
pub mod models;
pub mod outline;
pub mod pipeline;
pub mod prompts;
pub mod providers;
pub mod scope;
pub mod search;
pub mod slots;
pub mod tools;
pub mod web;
pub mod transcribe;
pub mod worker;
