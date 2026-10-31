pub mod graph;
pub mod parts;

pub use graph::graph as webp_graph;
pub use parts::{
    merge_animation as merge_parts, normalize_animation as normalize_part,
    trim_animation as trim_part, validate_animation as validate_part,
};
