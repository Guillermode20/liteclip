//! # SharedReplayBuffer - Trait Implementations
//!
//! This module contains trait implementations for `SharedReplayBuffer`.
//!
//! ## Implemented Traits
//!
//! - `Clone`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use std::sync::Arc;

use super::types::SharedReplayBuffer;

impl Clone for SharedReplayBuffer {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

