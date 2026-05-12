//! Fuzz target for hotkey string parsing.
//!
//! Feeds arbitrary strings to `parse_hotkey_components` and ensures it
//! never panics, regardless of input.
//!
//! # Running
//!
//! ```bash
//! cargo fuzz run hotkey_parsing -- -max_len=64 -timeout=5
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use liteclip_core::hotkey_parse::parse_hotkey_components;

// Maximum input length — hotkey strings are short by nature.
fuzz_target!(|data: &[u8]| {
    if data.len() > 128 {
        return; // Skip overly long inputs
    }

    let input = String::from_utf8_lossy(data);

    // Must never panic regardless of input
    let _result = parse_hotkey_components(&input);
});
