//! # EncodedPacket - Trait Implementations
//!
//! This module contains trait implementations for `EncodedPacket`.
//!
//! ## Implemented Traits
//!
//! - `Debug`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)


use super::types::EncodedPacket;

impl std::fmt::Debug for EncodedPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedPacket")
            .field("size", &self.data.len())
            .field("pts", &self.pts)
            .field("dts", &self.dts)
            .field("is_keyframe", &self.is_keyframe)
            .field("stream", &self.stream)
            .field("resolution", &self.resolution)
            .finish()
    }
}

