//! Hotkey string parsing shared by config validation and the Win32 platform layer.

/// Modifier bits for [`parse_hotkey_components`].
pub const MOD_ALT_BIT: u8 = 1;
pub const MOD_CTRL_BIT: u8 = 2;
pub const MOD_SHIFT_BIT: u8 = 4;
pub const MOD_WIN_BIT: u8 = 8;

/// Parse a hotkey string like `Alt+F9` into modifier bits and a virtual-key code.
///
/// Returns an error string suitable for logging or wrapping in `anyhow`.
pub fn parse_hotkey_components(hotkey: &str) -> Result<(u8, u32), String> {
    let parts: Vec<&str> = hotkey.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return Err(format!("Invalid hotkey format: '{hotkey}'"));
    }

    let mut modifiers = 0u8;
    let mut key = 0u32;
    let mut seen_key = false;

    for part in &parts {
        let normalized = part.to_ascii_lowercase();
        match normalized.as_str() {
            "alt" => modifiers |= MOD_ALT_BIT,
            "ctrl" | "control" => modifiers |= MOD_CTRL_BIT,
            "shift" => modifiers |= MOD_SHIFT_BIT,
            "win" => modifiers |= MOD_WIN_BIT,
            _ => {
                let mut parsed_key = None;
                if normalized.len() >= 2 && normalized.starts_with('f') {
                    if let Ok(n) = normalized[1..].parse::<u32>() {
                        if (1..=24).contains(&n) {
                            parsed_key = Some(0x6F + n);
                        }
                    }
                } else if normalized.len() == 1 {
                    let Some(ch) = normalized.chars().next() else {
                        return Err(format!("Empty key token in '{hotkey}'"));
                    };
                    let ch = ch.to_ascii_uppercase() as u32;
                    if (0x30..=0x39).contains(&ch) || (0x41..=0x5A).contains(&ch) {
                        parsed_key = Some(ch);
                    }
                }

                let Some(parsed_key) = parsed_key else {
                    return Err(format!("Unsupported hotkey token '{part}' in '{hotkey}'"));
                };
                if seen_key {
                    return Err(format!("Hotkey '{hotkey}' contains multiple key tokens"));
                }
                key = parsed_key;
                seen_key = true;
            }
        }
    }

    if key == 0 {
        return Err(format!("Could not parse hotkey: {hotkey}"));
    }
    if modifiers == 0 {
        return Err(format!(
            "Hotkey '{hotkey}' must include at least one modifier"
        ));
    }

    Ok((modifiers, key))
}

/// Validate all hotkey strings on [`crate::config::Config`].
pub fn validate_hotkey_config_strings(config: &crate::config::Config) {
    use tracing::warn;

    let fields = [
        ("save_clip", config.hotkeys.save_clip.as_str()),
        ("toggle_recording", config.hotkeys.toggle_recording.as_str()),
        ("screenshot", config.hotkeys.screenshot.as_str()),
        ("open_gallery", config.hotkeys.open_gallery.as_str()),
    ];

    for (name, value) in fields {
        if parse_hotkey_components(value).is_err() {
            warn!(
                "Config: hotkey field '{}' ('{}') is invalid — hotkey registration may fail until fixed",
                name, value
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_f9() {
        let (m, k) = parse_hotkey_components("Alt+F9").unwrap();
        assert!(m & MOD_ALT_BIT != 0);
        assert_eq!(k, 0x78);
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse_hotkey_components("Alt+Mouse4").is_err());
    }
}
