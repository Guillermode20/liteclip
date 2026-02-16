//! Video encoding module
//!
//! Encodes captured frames to H.264/H.265 using hardware (NVENC/AMF/QSV) or software encoders.

pub mod cpu_readback;
pub mod encoder_mod;
pub mod hw_encoder;
pub mod sw_encoder;

pub use encoder_mod::*;
pub use hw_encoder::*;
pub use sw_encoder::*;
