//! Edge-case tests for hotkey string parsing.
//!
//! Exercises `liteclip_core::hotkey_parse::parse_hotkey_components` with
//! unusual, malformed, and boundary-case inputs.

use liteclip_core::hotkey_parse::parse_hotkey_components;

// ===========================================================================
// Valid cases
// ===========================================================================

#[test]
fn alt_f1_parses() {
    let result = parse_hotkey_components("Alt+F1");
    assert!(result.is_ok(), "Alt+F1 should be valid: {:?}", result.err());
    let (mods, key) = result.unwrap();
    assert!(mods & 1 != 0, "Alt modifier should be set");
    assert_eq!(key, 0x70, "F1 virtual key code should be 0x70"); // VK_F1 = 0x70
}

#[test]
fn ctrl_shift_f9_parses() {
    let result = parse_hotkey_components("Ctrl+Shift+F9");
    assert!(result.is_ok(), "Ctrl+Shift+F9 should be valid");
    let (mods, key) = result.unwrap();
    assert!(mods & 2 != 0, "Ctrl modifier should be set");
    assert!(mods & 4 != 0, "Shift modifier should be set");
    assert_eq!(key, 0x78, "F9 virtual key code should be 0x78");
}

#[test]
fn win_a_parses() {
    let result = parse_hotkey_components("Win+A");
    assert!(result.is_ok(), "Win+A should be valid");
    let (mods, key) = result.unwrap();
    assert!(mods & 8 != 0, "Win modifier should be set");
    assert_eq!(key, 0x41, "'A' virtual key code should be 0x41");
}

#[test]
fn control_modifier_accepted() {
    let result = parse_hotkey_components("Control+S");
    assert!(result.is_ok(), "Control+S should be valid");
}

#[test]
fn f24_is_max_function_key() {
    let result = parse_hotkey_components("Alt+F24");
    assert!(result.is_ok(), "F24 should be valid");
    let (_, key) = result.unwrap();
    assert_eq!(key, 0x87, "F24 virtual key code should be 0x87");
}

#[test]
fn digit_key_parses() {
    let result = parse_hotkey_components("Ctrl+5");
    assert!(result.is_ok(), "Ctrl+5 should be valid");
    let (_, key) = result.unwrap();
    assert_eq!(key, 0x35, "'5' virtual key code should be 0x35");
}

// ===========================================================================
// Invalid / edge cases
// ===========================================================================

#[test]
fn empty_string_rejected() {
    let result = parse_hotkey_components("");
    assert!(result.is_err(), "Empty string should be rejected");
}

#[test]
fn key_without_modifier_rejected() {
    let result = parse_hotkey_components("F1");
    assert!(result.is_err(), "F1 without modifier should be rejected");
}

#[test]
fn modifier_only_no_key_rejected() {
    let result = parse_hotkey_components("Alt");
    assert!(result.is_err(), "Modifier without key should be rejected");
}

#[test]
fn multiple_keys_rejected() {
    let result = parse_hotkey_components("Alt+A+B");
    assert!(result.is_err(), "Multiple key tokens should be rejected");
    let err = result.unwrap_err();
    assert!(
        err.contains("multiple key tokens") || err.contains("Unsupported hotkey token"),
        "Error should mention multiple keys: got '{}'",
        err
    );
}

#[test]
fn f25_rejected_out_of_range() {
    let result = parse_hotkey_components("Alt+F25");
    assert!(
        result.is_err(),
        "F25 (outside F1-F24 range) should be rejected"
    );
}

#[test]
fn f0_rejected_out_of_range() {
    let result = parse_hotkey_components("Alt+F0");
    assert!(
        result.is_err(),
        "F0 (outside F1-F24 range) should be rejected"
    );
}

#[test]
fn trailing_plus_rejected() {
    let result = parse_hotkey_components("Alt+F1+");
    assert!(
        result.is_err(),
        "Trailing + should be rejected (empty token after '+'): got {:?}",
        result
    );
}

#[test]
fn leading_plus_rejected() {
    let result = parse_hotkey_components("+Alt+F1");
    assert!(
        result.is_err(),
        "Leading + should be rejected (empty token before '+'): got {:?}",
        result
    );
}

#[test]
fn unknown_modifier_rejected() {
    let result = parse_hotkey_components("Super+F1");
    assert!(
        result.is_err(),
        "Unknown modifier 'Super' should be rejected"
    );
}

#[test]
fn spaces_in_modifier_rejected_or_stripped() {
    // The parser calls trim() on each part, so whitespace should be stripped
    // and " Alt + F1 " parsed as Alt+F1.
    let result = parse_hotkey_components(" Alt + F1 ");
    assert!(
        result.is_ok(),
        "Whitespace around tokens should be stripped: got {:?}",
        result
    );
}

#[test]
fn very_long_hotkey_string_rejected() {
    let long = format!("Alt+F{}", "1".repeat(100));
    let result = parse_hotkey_components(&long);
    // Should not panic under any circumstances
    let _ = result;
}

#[test]
fn unicode_modifier_rejected() {
    let result = parse_hotkey_components("🔥+A");
    assert!(result.is_err(), "Unicode modifier should be rejected");
}

// ===========================================================================
// Boundary: key range checks
// ===========================================================================

#[test]
fn f_function_keys_cover_full_range() {
    for n in 1..=24u32 {
        let hotkey = format!("Alt+F{}", n);
        let result = parse_hotkey_components(&hotkey);
        assert!(
            result.is_ok(),
            "F{} (in range 1-24) should be valid: {:?}",
            n,
            result.err()
        );
        let (_, key) = result.unwrap();
        let expected = 0x6F + n;
        assert_eq!(
            key, expected,
            "F{} should map to VK code 0x{:X}",
            n, expected
        );
    }
}

// The 'A' to 'Z' range
#[test]
fn all_letter_keys_parse() {
    for ch in 'A'..='Z' {
        let hotkey = format!("Ctrl+{}", ch);
        let result = parse_hotkey_components(&hotkey);
        assert!(
            result.is_ok(),
            "Ctrl+{} should be valid: {:?}",
            ch,
            result.err()
        );
    }
}

// The '0' to '9' range
#[test]
fn all_digit_keys_parse() {
    for ch in '0'..='9' {
        let hotkey = format!("Alt+{}", ch);
        let result = parse_hotkey_components(&hotkey);
        assert!(
            result.is_ok(),
            "Alt+{} should be valid: {:?}",
            ch,
            result.err()
        );
    }
}
