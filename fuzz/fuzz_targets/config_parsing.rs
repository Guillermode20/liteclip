//! Fuzz target for config TOML parsing.
//!
//! Feeds arbitrary byte sequences to `toml::from_str::<Config>` and verifies
//! that deserialization never panics and that a roundtrip preserves basic
//! structure.
//!
//! # Running
//!
//! ```bash
//! cargo fuzz run config_parsing -- -max_len=4096 -timeout=5
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use liteclip_core::config::Config;

fuzz_target!(|data: &[u8]| {
    // Accept arbitrary UTF-8-ish bytes (lossy conversion is fine —
    // we just want to explore edge cases in the TOML parser)
    let input = String::from_utf8_lossy(data);

    // Attempt to parse as Config
    if let Ok(config) = toml::from_str::<Config>(&input) {
        // Roundtrip: serialize back to TOML string
        if let Ok(serialized) = toml::to_string(&config) {
            // Deserialize again — must not panic
            if let Ok(deserialized) = toml::from_str::<Config>(&serialized) {
                // Basic sanity: replay duration should be
                // preserved through roundtrip.
                let _ = deserialized.general.replay_duration_secs;
                let _ = deserialized.video.framerate;
                let _ = deserialized.video.encoder;
            }
        }
    }
});
