pub mod error;
pub mod functions;
pub mod mp4;
pub mod saver;
pub mod types;

pub use error::OutputError;
pub use functions::{
    calculate_clip_start_pts, extract_thumbnail, generate_output_path, generate_thumbnail,
    h264_nal_type, hevc_nal_type,
};
pub use saver::{spawn_clip_saver, spawn_clip_saver_with_defaults};
pub use types::{Muxer, MuxerConfig};
