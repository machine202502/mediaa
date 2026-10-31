pub mod encode;
pub mod graph;
pub mod parts;
pub mod reader;
pub mod stream;

pub use encode::{encode, EncodeOptions};
pub use graph::graph as webp_graph;
pub use parts::{
    merge_animation as merge_parts, normalize_animation as normalize_part,
    trim_animation as trim_part, validate_animation as validate_part,
};
pub use reader::{scale_frame, Info, Reader, WebpFrame};
pub use stream::{frame_stream, FrameStream, Producer};
