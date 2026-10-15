//! Preserve the original xterm oracle for the shared legacy keyboard behavior.
//! Ghostty intentionally adds fixterms/extended function keys; those differences
//! are named below and exercised by the adapter's protocol tests, not silently
//! accepted by replacing the original oracle with generated current output.
#![cfg(any(target_os = "macos", windows))]
use newport_gpui::terminal_input::{KeyEvent, encode_key, scroll_key};
use newport_terminal::Engine;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    key: String,
    modifiers: u8,
    application_cursor: bool,
    output: Option<String>,
    scroll: Option<i32>,
}

/// These categories no longer claim xterm byte-for-byte parity. Shell control
/// aliases Ctrl+i/m/[ are deliberately absent: their legacy bytes are preserved.
fn protocol_difference(f: &Fixture) -> Option<&'static str> {
    let key = f.key.as_str();
    let m = f.modifiers;
    if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (13..=25).contains(&number) {
            return Some("F13–F25 are supported");
        }
        if number == 3 && m != 0 {
            return Some("modified F3 uses Ghostty's tilde form");
        }
        if (1..=12).contains(&number)
            && ((cfg!(target_os = "macos") && m & 2 != 0 && m & 4 == 0)
                || (cfg!(windows) && m & 6 == 6))
        {
            return Some("modified function keys retain platform modifiers");
        }
    }
    match key {
        "enter" | "escape" if !matches!(m, 0 | 2) => {
            return Some("Enter and Escape retain non-Alt modifiers");
        }
        "tab" if m >= 2 => return Some("Tab retains non-Shift modifiers"),
        "backspace" if m == 7 => return Some("Shift+Control+Alt+Backspace uses Ghostty mapping"),
        "pageup" | "pagedown" if m == 2 => return Some("Page keys retain Alt"),
        "insert" if m != 0 => return Some("modified Insert is encoded after local shortcuts"),
        _ => {}
    }
    // Formerly ignored Ctrl+Shift/Ctrl+Alt letters and punctuation are encoded
    // by Ghostty's legacy fixterms extension. AltGr continues through native
    // text input on Windows, including Space (unlike the old xterm exception).
    let printable = key == "space" || key.chars().count() == 1;
    if printable && m & 4 != 0 && (m != 4 || f.output.is_none()) {
        return Some("additional modified printable keys use Ghostty/native AltGr");
    }
    None
}

#[test]
fn shared_legacy_keyboard_behavior_matches_original_xterm() {
    #[cfg(target_os = "macos")]
    let source = include_str!("fixtures/terminal-keyboard.json");
    #[cfg(windows)]
    let source = include_str!("fixtures/terminal-keyboard-windows.json");
    let fixtures: Vec<Fixture> = serde_json::from_str(source).unwrap();
    assert_eq!(fixtures.len(), if cfg!(windows) { 1120 } else { 844 });
    let mut differences = Vec::new();
    let mut excluded = BTreeMap::new();
    let mut checked = 0;
    for fixture in fixtures {
        let shift = fixture.modifiers & 1 != 0;
        let alt = fixture.modifiers & 2 != 0;
        let control = fixture.modifiers & 4 != 0;
        // Local scrolling remains identical for every original fixture,
        // including the protocol-specific cases below.
        assert_eq!(scroll_key(&fixture.key, shift, 24), fixture.scroll);
        if let Some(reason) = protocol_difference(&fixture) {
            *excluded.entry(reason).or_insert(0) += 1;
            continue;
        }
        checked += 1;
        let mut engine = Engine::new(80, 24).unwrap();
        if fixture.application_cursor {
            engine.advance(b"\x1b[?1h").unwrap();
        }
        for newline in [false, true] {
            engine
                .advance(if newline { b"\x1b[20h" } else { b"\x1b[20l" })
                .unwrap();
            let actual = encode_key(
                &mut engine,
                KeyEvent {
                    shift,
                    alt,
                    control,
                    ..KeyEvent::press(&fixture.key)
                },
            )
            .unwrap();
            let expected = fixture.output.as_ref().map(|s| s.as_bytes().to_vec());
            if actual != expected {
                differences.push(format!(
                    "{} modifiers={} cursor={} newline={newline}: output={actual:?}, expected={expected:?}",
                    fixture.key, fixture.modifiers, fixture.application_cursor
                ));
            }
        }
    }
    eprintln!("Compared {checked} original key cases; explicit protocol differences: {excluded:?}");
    assert_eq!(
        checked,
        if cfg!(windows) { 570 } else { 378 },
        "Legacy coverage changed; review the explicit protocol categories"
    );
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
