//! Auto-generated module structure
//!
//! Serde default helpers live in [`functions`] and are crate-private; embedders use [`types`]
//! ([`Config`](types::Config), section structs, and enums).

pub mod advancedconfig_traits;
pub mod audioconfig_traits;
pub(crate) mod functions;
pub mod generalconfig_traits;
pub mod hotkeyconfig_traits;
pub mod types;
pub mod videoconfig_traits;

// Embedder-facing API: configuration types and shared limits (not serde plumbing).
pub use functions::{
    ESTIMATED_MIC_AUDIO_BITRATE_BPS, ESTIMATED_SYSTEM_AUDIO_BITRATE_BPS, MAX_FRAMERATE,
    RECOMMENDED_BUFFER_BASE_OVERHEAD_MB, RECOMMENDED_BUFFER_HEADROOM_PERCENT,
};
pub use types::*;
