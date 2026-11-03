#![forbid(unsafe_code)]

pub mod db;

pub use db::Db;

pub const SCHEMA_VERSION: u32 = 1;
