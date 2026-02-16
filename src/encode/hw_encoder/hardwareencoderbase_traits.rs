//! # HardwareEncoderBase - Trait Implementations
//!
//! This module contains trait implementations for `HardwareEncoderBase`.
//!
//! ## Implemented Traits
//!
//! - `Drop`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::types::HardwareEncoderBase;

/// Drop implementation for HardwareEncoderBase.
/// Note: The actual FFmpeg process cleanup is handled by ManagedFfmpegProcess::Drop.
impl Drop for HardwareEncoderBase {
    fn drop(&mut self) {
        // The ManagedFfmpegProcess (if present) will be dropped automatically
        // and will handle all cleanup (stdin close, process wait/kill, thread joins)
        // This explicit take ensures it's dropped during HardwareEncoderBase's drop
        if self.ffmpeg.take().is_some() {
            // ManagedFfmpegProcess::Drop handles the cleanup
        }
    }
}
