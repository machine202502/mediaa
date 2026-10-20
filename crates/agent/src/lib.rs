//! Agent section back end: downloading Hugging Face models into the library's `.ai/models`,
//! validating model files and checking providers (keys, model names).

pub mod assistant;
pub mod check;
pub mod embed;
pub mod hub;
pub mod index;
pub mod llm;
pub mod local;
pub mod outline;
pub mod prompts;
pub mod providers;
pub mod runtime;
pub mod scheduler;
pub mod search;
pub mod speech;
pub mod tools;
pub mod web;
pub mod transcribe;
pub mod worker;
