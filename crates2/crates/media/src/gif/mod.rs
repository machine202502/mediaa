pub mod encode;
pub mod graph;
pub mod trim;

pub use encode::{encode, encode_frames};
pub use graph::{graph as gif_graph, palette_graph, use_graph};
pub use trim::trim;
