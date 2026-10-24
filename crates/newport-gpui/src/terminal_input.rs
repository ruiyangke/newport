//! Map platform key identities to Ghostty. VT sequences belong to the engine;
//! local shortcuts and native text composition remain the UI's responsibility.
use gpui_kit as gpui;

use newport_terminal::{Engine, Key, KeyAction, KeyInput, KittyKeyFlags, Modifiers, Result};

/// The platform's logical key and optional layout-generated text. Callers must
/// intercept app shortcuts first and pair releases only with forwarded presses.
pub struct KeyEvent<'a> {
    pub key: &'a str,
    pub text: Option<&'a str>,
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub action: KeyAction,
    pub composing: bool,
    pub native_identity: Option<NativeKeyIdentity>,
    pub native_modifiers: Option<Modifiers>,
    /// macOS only: Option sends Meta keys instead of composing text.
    /// Elsewhere Alt is always Meta.
    pub option_as_meta: bool,
}

impl<'a> KeyEvent<'a> {
    pub fn press(key: &'a str) -> Self {
        Self {
            key,
            text: None,
            shift: false,
            alt: false,
            control: false,
            action: KeyAction::Press,
            composing: false,
            native_identity: None,
            native_modifiers: None,
            option_as_meta: false,
        }
    }
}

/// Native physical position plus event-time layout identity. Retain this across
/// repeats/releases; text and modifier state still come from each current event.
#[derive(Clone, Copy, Debug)]
pub struct NativeKeyIdentity {
    pub key: Key,
    pub unshifted: char,
}

pub fn native_key_identity(
    physical: Option<&gpui::PhysicalKey>,
    metadata: Option<&gpui::KeyEventMetadata>,
) -> Option<NativeKeyIdentity> {
    let metadata = metadata?;
    let gpui::PhysicalKey::Native(code) = physical? else {
        return None;
    };
    let key = platform_physical_key(*code)?;
    Some(NativeKeyIdentity {
        key,
        unshifted: metadata
            .unmodified_text
            .as_deref()
            .and_then(|s| s.chars().next())
            .unwrap_or('\0'),
    })
}

#[cfg(target_os = "macos")]
fn platform_physical_key(code: u32) -> Option<Key> {
    Some(macos_physical_key(code))
}

#[cfg(windows)]
fn platform_physical_key(code: u32) -> Option<Key> {
    Some(windows_physical_key(code))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_physical_key(_code: u32) -> Option<Key> {
    // Each backend needs its own audited native-code mapping. Never interpret
    // another OS's opaque codes or derive physical position from layout text.
    None
}

pub fn native_modifiers(metadata: Option<&gpui::KeyEventMetadata>) -> Option<Modifiers> {
    metadata.map(|metadata| {
        current_modifiers(metadata.modifiers, metadata.caps_lock, metadata.num_lock)
    })
}

pub fn current_modifiers(raw: gpui::Modifiers, caps_lock: bool, num_lock: bool) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, raw.shift);
    modifiers.set(Modifiers::ALT, raw.alt);
    modifiers.set(Modifiers::CTRL, raw.control);
    modifiers.set(Modifiers::SUPER, raw.platform);
    modifiers.set(Modifiers::CAPS_LOCK, caps_lock);
    modifiers.set(Modifiers::NUM_LOCK, num_lock);
    modifiers
}

#[cfg(target_os = "macos")]
fn macos_physical_key(code: u32) -> Key {
    // Ghostty 3425025e585a3403340d4dc6d65132f42e05e605,
    // src/input/keycodes.zig native Mac column (Chromium DOM-code mapping).
    // Native key positions are independent of the user's active layout.
    match code {
        0x00 => Key::A,
        0x01 => Key::S,
        0x02 => Key::D,
        0x03 => Key::F,
        0x04 => Key::H,
        0x05 => Key::G,
        0x06 => Key::Z,
        0x07 => Key::X,
        0x08 => Key::C,
        0x09 => Key::V,
        0x0a => Key::IntlBackslash,
        0x0b => Key::B,
        0x0c => Key::Q,
        0x0d => Key::W,
        0x0e => Key::E,
        0x0f => Key::R,
        0x10 => Key::Y,
        0x11 => Key::T,
        0x12 => Key::Digit1,
        0x13 => Key::Digit2,
        0x14 => Key::Digit3,
        0x15 => Key::Digit4,
        0x16 => Key::Digit6,
        0x17 => Key::Digit5,
        0x18 => Key::Equal,
        0x19 => Key::Digit9,
        0x1a => Key::Digit7,
        0x1b => Key::Minus,
        0x1c => Key::Digit8,
        0x1d => Key::Digit0,
        0x1e => Key::BracketRight,
        0x1f => Key::O,
        0x20 => Key::U,
        0x21 => Key::BracketLeft,
        0x22 => Key::I,
        0x23 => Key::P,
        0x24 => Key::Enter,
        0x25 => Key::L,
        0x26 => Key::J,
        0x27 => Key::Quote,
        0x28 => Key::K,
        0x29 => Key::Semicolon,
        0x2a => Key::Backslash,
        0x2b => Key::Comma,
        0x2c => Key::Slash,
        0x2d => Key::N,
        0x2e => Key::M,
        0x2f => Key::Period,
        0x30 => Key::Tab,
        0x31 => Key::Space,
        0x32 => Key::Backquote,
        0x33 => Key::Backspace,
        0x35 => Key::Escape,
        0x36 => Key::MetaRight,
        0x37 => Key::MetaLeft,
        0x38 => Key::ShiftLeft,
        0x39 => Key::CapsLock,
        0x3a => Key::AltLeft,
        0x3b => Key::ControlLeft,
        0x3c => Key::ShiftRight,
        0x3d => Key::AltRight,
        0x3e => Key::ControlRight,
        0x40 => Key::F17,
        0x41 => Key::NumpadDecimal,
        0x43 => Key::NumpadMultiply,
        0x45 => Key::NumpadAdd,
        0x47 => Key::NumLock,
        0x48 => Key::AudioVolumeUp,
        0x49 => Key::AudioVolumeDown,
        0x4a => Key::AudioVolumeMute,
        0x4b => Key::NumpadDivide,
        0x4c => Key::NumpadEnter,
        0x4e => Key::NumpadSubtract,
        0x4f => Key::F18,
        0x50 => Key::F19,
        0x51 => Key::NumpadEqual,
        0x52 => Key::Numpad0,
        0x53 => Key::Numpad1,
        0x54 => Key::Numpad2,
        0x55 => Key::Numpad3,
        0x56 => Key::Numpad4,
        0x57 => Key::Numpad5,
        0x58 => Key::Numpad6,
        0x59 => Key::Numpad7,
        0x5a => Key::F20,
        0x5b => Key::Numpad8,
        0x5c => Key::Numpad9,
        0x5d => Key::IntlYen,
        0x5e => Key::IntlRo,
        0x5f => Key::NumpadComma,
        0x60 => Key::F5,
        0x61 => Key::F6,
        0x62 => Key::F7,
        0x63 => Key::F3,
        0x64 => Key::F8,
        0x65 => Key::F9,
        0x67 => Key::F11,
        0x69 => Key::F13,
        0x6a => Key::F16,
        0x6b => Key::F14,
        0x6d => Key::F10,
        0x6e => Key::ContextMenu,
        0x6f => Key::F12,
        0x71 => Key::F15,
        0x72 => Key::Insert,
        0x73 => Key::Home,
        0x74 => Key::PageUp,
        0x75 => Key::Delete,
        0x76 => Key::F4,
        0x77 => Key::End,
        0x78 => Key::F2,
        0x79 => Key::PageDown,
        0x7a => Key::F1,
        0x7b => Key::ArrowLeft,
        0x7c => Key::ArrowRight,
        0x7d => Key::ArrowDown,
        0x7e => Key::ArrowUp,
        _ => Key::Unidentified,
    }
}

#[cfg(any(windows, test))]
fn windows_physical_key(code: u32) -> Key {
    // Ghostty 3425025e585a3403340d4dc6d65132f42e05e605,
    // src/input/keycodes.zig native Windows column (Chromium DOM-code mapping).
    // GPUI packs the WM_KEY LPARAM scan byte plus extended bit as 0x100;
    // Ghostty's table expresses that same extended prefix as 0xe000.
    // In particular Pause is 0x045, NumLock 0x145; PrintScreen is 0x137.
    // Keep extended keypad/navigation and left/right modifiers distinct.
    match code {
        0x001 => Key::Escape,
        0x002 => Key::Digit1,
        0x003 => Key::Digit2,
        0x004 => Key::Digit3,
        0x005 => Key::Digit4,
        0x006 => Key::Digit5,
        0x007 => Key::Digit6,
        0x008 => Key::Digit7,
        0x009 => Key::Digit8,
        0x00a => Key::Digit9,
        0x00b => Key::Digit0,
        0x00c => Key::Minus,
        0x00d => Key::Equal,
        0x00e => Key::Backspace,
        0x00f => Key::Tab,
        0x010 => Key::Q,
        0x011 => Key::W,
        0x012 => Key::E,
        0x013 => Key::R,
        0x014 => Key::T,
        0x015 => Key::Y,
        0x016 => Key::U,
        0x017 => Key::I,
        0x018 => Key::O,
        0x019 => Key::P,
        0x01a => Key::BracketLeft,
        0x01b => Key::BracketRight,
        0x01c => Key::Enter,
        0x01d => Key::ControlLeft,
        0x01e => Key::A,
        0x01f => Key::S,
        0x020 => Key::D,
        0x021 => Key::F,
        0x022 => Key::G,
        0x023 => Key::H,
        0x024 => Key::J,
        0x025 => Key::K,
        0x026 => Key::L,
        0x027 => Key::Semicolon,
        0x028 => Key::Quote,
        0x029 => Key::Backquote,
        0x02a => Key::ShiftLeft,
        0x02b => Key::Backslash,
        0x02c => Key::Z,
        0x02d => Key::X,
        0x02e => Key::C,
        0x02f => Key::V,
        0x030 => Key::B,
        0x031 => Key::N,
        0x032 => Key::M,
        0x033 => Key::Comma,
        0x034 => Key::Period,
        0x035 => Key::Slash,
        0x036 => Key::ShiftRight,
        0x037 => Key::NumpadMultiply,
        0x038 => Key::AltLeft,
        0x039 => Key::Space,
        0x03a => Key::CapsLock,
        0x03b => Key::F1,
        0x03c => Key::F2,
        0x03d => Key::F3,
        0x03e => Key::F4,
        0x03f => Key::F5,
        0x040 => Key::F6,
        0x041 => Key::F7,
        0x042 => Key::F8,
        0x043 => Key::F9,
        0x044 => Key::F10,
        0x045 => Key::Pause,
        0x046 => Key::ScrollLock,
        0x047 => Key::Numpad7,
        0x048 => Key::Numpad8,
        0x049 => Key::Numpad9,
        0x04a => Key::NumpadSubtract,
        0x04b => Key::Numpad4,
        0x04c => Key::Numpad5,
        0x04d => Key::Numpad6,
        0x04e => Key::NumpadAdd,
        0x04f => Key::Numpad1,
        0x050 => Key::Numpad2,
        0x051 => Key::Numpad3,
        0x052 => Key::Numpad0,
        0x053 => Key::NumpadDecimal,
        0x056 => Key::IntlBackslash,
        0x057 => Key::F11,
        0x058 => Key::F12,
        0x059 => Key::NumpadEqual,
        0x064 => Key::F13,
        0x065 => Key::F14,
        0x066 => Key::F15,
        0x067 => Key::F16,
        0x068 => Key::F17,
        0x069 => Key::F18,
        0x06a => Key::F19,
        0x06b => Key::F20,
        0x06c => Key::F21,
        0x06d => Key::F22,
        0x06e => Key::F23,
        0x070 => Key::KanaMode,
        0x073 => Key::IntlRo,
        0x076 => Key::F24,
        0x079 => Key::Convert,
        0x07b => Key::NonConvert,
        0x07d => Key::IntlYen,
        0x07e => Key::NumpadComma,
        0x10a => Key::Paste,
        0x110 => Key::MediaTrackPrevious,
        0x117 => Key::Cut,
        0x118 => Key::Copy,
        0x119 => Key::MediaTrackNext,
        0x11c => Key::NumpadEnter,
        0x11d => Key::ControlRight,
        0x120 => Key::AudioVolumeMute,
        0x121 => Key::LaunchApp2,
        0x122 => Key::MediaPlayPause,
        0x124 => Key::MediaStop,
        0x12c => Key::Eject,
        0x12e => Key::AudioVolumeDown,
        0x130 => Key::AudioVolumeUp,
        0x132 => Key::BrowserHome,
        0x135 => Key::NumpadDivide,
        0x137 => Key::PrintScreen,
        0x138 => Key::AltRight,
        0x13b => Key::Help,
        0x145 => Key::NumLock,
        0x147 => Key::Home,
        0x148 => Key::ArrowUp,
        0x149 => Key::PageUp,
        0x14b => Key::ArrowLeft,
        0x14d => Key::ArrowRight,
        0x14f => Key::End,
        0x150 => Key::ArrowDown,
        0x151 => Key::PageDown,
        0x152 => Key::Insert,
        0x153 => Key::Delete,
        0x15b => Key::MetaLeft,
        0x15c => Key::MetaRight,
        0x15d => Key::ContextMenu,
        0x15e => Key::Power,
        0x15f => Key::Sleep,
        0x163 => Key::WakeUp,
        0x165 => Key::BrowserSearch,
        0x166 => Key::BrowserFavorites,
        0x167 => Key::BrowserRefresh,
        0x168 => Key::BrowserStop,
        0x169 => Key::BrowserForward,
        0x16a => Key::BrowserBack,
        0x16b => Key::LaunchApp1,
        0x16c => Key::LaunchMail,
        0x16d => Key::MediaSelect,
        _ => Key::Unidentified,
    }
}

/// `None` delegates to native text input. `Some`, including empty bytes on a
/// legacy key release, means the key was handled. No escape sequences are
/// constructed here; Ghostty honors the modes requested by the foreground TUI.
pub fn encode_key(engine: &mut Engine, event: KeyEvent<'_>) -> Result<Option<Vec<u8>>> {
    if event.composing {
        return Ok(None);
    }
    let named = event.key.to_ascii_lowercase();
    let (fallback_key, printable) = key_identity(&named);
    let key = event
        .native_identity
        .map_or(fallback_key, |identity| identity.key);
    let unshifted = event
        .native_identity
        .map_or(printable.unwrap_or('\0'), |identity| identity.unshifted);
    let mut modifiers = event.native_modifiers.unwrap_or_else(Modifiers::empty);
    if event.native_modifiers.is_none() {
        modifiers.set(Modifiers::SHIFT, event.shift || named == "backtab");
        modifiers.set(Modifiers::ALT, event.alt);
        modifiers.set(Modifiers::CTRL, event.control);
    }
    let shift = modifiers.contains(Modifiers::SHIFT);
    let alt = modifiers.contains(Modifiers::ALT);
    let control = modifiers.contains(Modifiers::CTRL);
    let control_or_super = modifiers.intersects(Modifiers::CTRL | Modifiers::SUPER);
    let is_text = printable.is_some();
    // macOS Option (unless it is Meta) composes text without Ctrl/Super.
    // Windows AltGr is routed by KeyDownEvent::prefer_character_input;
    // ordinary Ctrl+Alt commands must reach the remote application. Releases
    // belong to already-forwarded presses even if modifiers changed while held.
    if event.action != KeyAction::Release
        && is_text
        && cfg!(target_os = "macos")
        && alt
        && !control_or_super
        && !event.option_as_meta
    {
        return Ok(None);
    }
    if key == Key::Unidentified && !is_text {
        return Ok(None);
    }
    let kitty_flags = engine.kitty_keyboard_flags()?;
    let protocol_text =
        kitty_flags.intersects(KittyKeyFlags::REPORT_ALL | KittyKeyFlags::REPORT_EVENTS);
    // Ctrl/Super commands need encoding even in disambiguation-only Kitty mode.
    if is_text && !control_or_super && !alt && !protocol_text {
        return Ok(None);
    }
    let fallback_text = printable.map(|ch| {
        if event.shift {
            ch.to_uppercase().collect::<String>()
        } else {
            ch.to_string()
        }
    });
    // Releases must not carry committed text. Ghostty still needs the original
    // unshifted identity to encode a matching Kitty release event.
    let text = (event.action != KeyAction::Release)
        .then_some(event.text.or(fallback_text.as_deref()))
        .flatten();
    let mut consumed = Modifiers::empty();
    // Shift has already been applied to layout-generated text. Control, Alt,
    // and Super remain unconsumed so Ghostty can encode command combinations.
    if text.is_some() && shift {
        consumed.insert(Modifiers::SHIFT);
    }
    let output = engine.encode_key(KeyInput {
        key,
        action: event.action,
        modifiers,
        consumed_modifiers: consumed,
        text,
        unshifted,
        composing: false,
        option_as_alt: option_as_alt(&event),
    })?;
    // Newport preserves the conventional readline/Vim aliases. Ghostty's
    // default fixterms encoder disambiguates these three keys, even without an
    // application opt-in. Normalize only that exact legacy result: Kitty and
    // modifyOtherKeys output must retain the original physical key identity.
    let legacy_alias = if kitty_flags.is_empty() && control && !shift && !alt {
        match (named.as_str(), output.as_slice()) {
            ("i", b"\x1b[105;5u") => Some(Key::Tab),
            ("m", b"\x1b[109;5u") => Some(Key::Enter),
            ("[", b"\x1b[91;5u") => Some(Key::Escape),
            _ => None,
        }
    } else {
        None
    };
    if let Some(key) = legacy_alias {
        return engine
            .encode_key(KeyInput {
                key,
                action: event.action,
                modifiers: Modifiers::empty(),
                consumed_modifiers: Modifiers::empty(),
                text: None,
                unshifted: '\0',
                composing: false,
                option_as_alt: option_as_alt(&event),
            })
            .map(Some);
    }
    Ok((!output.is_empty() || event.action == KeyAction::Release).then_some(output))
}

fn option_as_alt(event: &KeyEvent<'_>) -> bool {
    !cfg!(target_os = "macos") || event.option_as_meta
}

fn key_identity(name: &str) -> (Key, Option<char>) {
    use Key::*;
    let named = match name {
        "up" | "arrowup" => ArrowUp,
        "down" | "arrowdown" => ArrowDown,
        "left" | "arrowleft" => ArrowLeft,
        "right" | "arrowright" => ArrowRight,
        "home" => Home,
        "end" => End,
        "pageup" | "page-up" => PageUp,
        "pagedown" | "page-down" => PageDown,
        "enter" | "return" => Enter,
        "escape" | "esc" => Escape,
        "backspace" => Backspace,
        "delete" | "forwarddelete" => Delete,
        "insert" => Insert,
        "tab" | "backtab" => Tab,
        "shift" => ShiftLeft,
        "control" | "ctrl" => ControlLeft,
        "alt" | "option" => AltLeft,
        "capslock" => CapsLock,
        "numlock" => NumLock,
        "scrolllock" => ScrollLock,
        "pause" => Pause,
        "printscreen" => PrintScreen,
        "f1" => F1,
        "f2" => F2,
        "f3" => F3,
        "f4" => F4,
        "f5" => F5,
        "f6" => F6,
        "f7" => F7,
        "f8" => F8,
        "f9" => F9,
        "f10" => F10,
        "f11" => F11,
        "f12" => F12,
        "f13" => F13,
        "f14" => F14,
        "f15" => F15,
        "f16" => F16,
        "f17" => F17,
        "f18" => F18,
        "f19" => F19,
        "f20" => F20,
        "f21" => F21,
        "f22" => F22,
        "f23" => F23,
        "f24" => F24,
        "f25" => F25,
        _ => Unidentified,
    };
    if named != Unidentified {
        return (named, None);
    }
    if let Some(keypad) = name
        .strip_prefix("numpad")
        .or_else(|| name.strip_prefix("kp"))
    {
        let key = match keypad {
            "0" => Numpad0,
            "1" => Numpad1,
            "2" => Numpad2,
            "3" => Numpad3,
            "4" => Numpad4,
            "5" => Numpad5,
            "6" => Numpad6,
            "7" => Numpad7,
            "8" => Numpad8,
            "9" => Numpad9,
            "decimal" | "." => NumpadDecimal,
            "add" | "+" => NumpadAdd,
            "subtract" | "-" => NumpadSubtract,
            "multiply" | "*" => NumpadMultiply,
            "divide" | "/" => NumpadDivide,
            "equal" | "=" => NumpadEqual,
            "separator" | "," => NumpadSeparator,
            "enter" => NumpadEnter,
            _ => Unidentified,
        };
        return (key, None);
    }
    let printable = if name == "space" { " " } else { name };
    let mut chars = printable.chars();
    let Some(ch) = chars.next() else {
        return (Unidentified, None);
    };
    if chars.next().is_some() || ch.is_control() {
        return (Unidentified, None);
    }
    let key = match ch {
        'a' => A,
        'b' => B,
        'c' => C,
        'd' => D,
        'e' => E,
        'f' => F,
        'g' => G,
        'h' => H,
        'i' => I,
        'j' => J,
        'k' => K,
        'l' => L,
        'm' => M,
        'n' => N,
        'o' => O,
        'p' => P,
        'q' => Q,
        'r' => R,
        's' => S,
        't' => T,
        'u' => U,
        'v' => V,
        'w' => W,
        'x' => X,
        'y' => Y,
        'z' => Z,
        '0' => Digit0,
        '1' => Digit1,
        '2' => Digit2,
        '3' => Digit3,
        '4' => Digit4,
        '5' => Digit5,
        '6' => Digit6,
        '7' => Digit7,
        '8' => Digit8,
        '9' => Digit9,
        '`' | '~' => Backquote,
        '-' | '_' => Minus,
        '=' | '+' => Equal,
        '[' | '{' => BracketLeft,
        ']' | '}' => BracketRight,
        '\\' | '|' => Backslash,
        ';' | ':' => Semicolon,
        '\'' | '"' => Quote,
        ',' | '<' => Comma,
        '.' | '>' => Period,
        '/' | '?' => Slash,
        ' ' => Space,
        _ => Unidentified,
    };
    (key, Some(ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(
        engine: &mut Engine,
        key: &str,
        shift: bool,
        alt: bool,
        control: bool,
    ) -> Option<Vec<u8>> {
        encode_key(
            engine,
            KeyEvent {
                shift,
                alt,
                control,
                ..KeyEvent::press(key)
            },
        )
        .unwrap()
    }

    #[test]
    fn cursor_keys_obey_application_mode_and_modifiers() {
        let mut engine = Engine::new(80, 24).unwrap();
        assert_eq!(
            encoded(&mut engine, "up", false, false, false).unwrap(),
            b"\x1b[A"
        );
        engine.advance(b"\x1b[?1h").unwrap();
        assert_eq!(
            encoded(&mut engine, "home", false, false, false).unwrap(),
            b"\x1bOH"
        );
        assert_eq!(
            encoded(&mut engine, "left", true, true, true).unwrap(),
            b"\x1b[1;8D"
        );
    }

    #[test]
    fn ordinary_text_and_native_composition_stay_on_platform_path() {
        let mut engine = Engine::new(80, 24).unwrap();
        for key in ["a", "A", " ", "space", "é", "你", "🙂", "shift"] {
            assert!(
                encoded(&mut engine, key, false, false, false).is_none(),
                "{key}"
            );
        }
        engine.advance(b"\x1b[>31u").unwrap();
        assert!(
            encode_key(
                &mut engine,
                KeyEvent {
                    composing: true,
                    ..KeyEvent::press("enter")
                }
            )
            .unwrap()
            .is_none()
        );
        if cfg!(target_os = "macos") {
            assert!(encoded(&mut engine, "e", false, true, false).is_none());
        }
        assert_eq!(
            encoded(&mut engine, "e", false, true, true).unwrap(),
            b"\x1b[101;7u"
        );
    }

    #[test]
    fn super_printable_keys_use_kitty_disambiguation_without_text_reporting() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(b"\x1b[>1u").unwrap();
        assert!(encoded(&mut engine, "n", false, false, false).is_none());
        for option_as_meta in [false, true] {
            for (key, modifiers, expected) in [
                ("n", Modifiers::SUPER, b"\x1b[110;9u".as_slice()),
                ("i", Modifiers::SUPER | Modifiers::ALT, b"\x1b[105;11u"),
            ] {
                assert_eq!(
                    encode_key(
                        &mut engine,
                        KeyEvent {
                            native_modifiers: Some(modifiers),
                            text: Some(key),
                            option_as_meta,
                            ..KeyEvent::press(key)
                        },
                    )
                    .unwrap()
                    .as_deref(),
                    Some(expected),
                    "{key}, option_as_meta={option_as_meta}"
                );
            }
        }
    }

    #[test]
    fn navigation_function_and_shell_control_keys_are_encoded_by_ghostty() {
        let mut engine = Engine::new(80, 24).unwrap();
        for (key, shift, alt, control, expected) in [
            ("f1", false, false, false, &b"\x1bOP"[..]),
            ("f4", false, false, true, &b"\x1b[1;5S"[..]),
            ("f12", true, false, false, &b"\x1b[24;2~"[..]),
            ("tab", true, false, false, &b"\x1b[Z"[..]),
            ("c", false, false, true, &b"\x03"[..]),
            ("q", false, false, true, &b"\x11"[..]),
            ("r", false, false, true, &b"\x12"[..]),
            ("space", false, false, true, &b"\0"[..]),
        ] {
            assert_eq!(
                encoded(&mut engine, key, shift, alt, control).unwrap(),
                expected,
                "{key}"
            );
        }
    }

    #[test]
    fn ghostty_extended_legacy_keys_retain_modifier_identity() {
        let mut engine = Engine::new(80, 24).unwrap();
        // Ghostty supports extended functions and disambiguates modified keys.
        for (key, shift, alt, control, expected) in [
            ("f13", false, false, false, b"\x1b[25~".as_slice()),
            ("f25", false, false, false, b"\x1b[46~"),
            ("f3", false, false, true, b"\x1b[13;5~"),
            ("f1", false, true, false, b"\x1b[1;3P"),
            ("enter", true, false, false, b"\x1b[27;2;13~"),
            ("escape", false, false, true, b"\x1b[27;5;27~"),
            ("tab", false, false, true, b"\x1b[27;5;9~"),
            ("backspace", true, true, true, b"\x7f"),
            ("pageup", false, true, false, b"\x1b[5;3~"),
            ("insert", true, false, false, b"\x1b[2;2~"),
            ("c", true, false, true, b"\x1b[99;6u"),
            (";", false, false, true, b"\x1b[59;5u"),
        ] {
            assert_eq!(
                encoded(&mut engine, key, shift, alt, control).unwrap(),
                expected,
                "{key}"
            );
        }
    }

    #[test]
    fn shifted_page_keys_reach_terminal_applications() {
        let mut engine = Engine::new(80, 24).unwrap();
        for (key, shift, alt, control, expected) in [
            ("pageup", true, false, false, b"\x1b[5;2~".as_slice()),
            ("page-down", true, false, false, b"\x1b[6;2~"),
            ("page-up", true, true, true, b"\x1b[5;8~"),
            ("pagedown", true, true, true, b"\x1b[6;8~"),
        ] {
            assert_eq!(
                encoded(&mut engine, key, shift, alt, control).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn shift_pressed_after_navigation_key_does_not_swallow_kitty_release() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(b"\x1b[>3u").unwrap();
        for (key, press, release) in [
            ("pageup", b"\x1b[5~".as_slice(), b"\x1b[5;2:3~".as_slice()),
            ("page-up", b"\x1b[5~", b"\x1b[5;2:3~"),
            ("pagedown", b"\x1b[6~", b"\x1b[6;2:3~"),
            ("page-down", b"\x1b[6~", b"\x1b[6;2:3~"),
            ("home", b"\x1b[1;1:1H", b"\x1b[1;2:3H"),
            ("end", b"\x1b[1;1:1F", b"\x1b[1;2:3F"),
        ] {
            assert_eq!(
                encode_key(&mut engine, KeyEvent::press(key))
                    .unwrap()
                    .unwrap(),
                press,
                "{key}"
            );
            // Shift becomes held after the navigation press was forwarded.
            assert_eq!(
                encode_key(
                    &mut engine,
                    KeyEvent {
                        action: KeyAction::Release,
                        shift: true,
                        ..KeyEvent::press(key)
                    }
                )
                .unwrap()
                .unwrap(),
                release,
                "{key}"
            );
        }
    }

    #[test]
    fn kitty_tracks_printable_press_repeat_release_with_associated_text() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(b"\x1b[>31u").unwrap();
        for (action, expected) in [
            (KeyAction::Press, &b"\x1b[97;;97u"[..]),
            (KeyAction::Repeat, &b"\x1b[97;1:2;97u"[..]),
            (KeyAction::Release, &b"\x1b[97;1:3u"[..]),
        ] {
            assert_eq!(
                encode_key(
                    &mut engine,
                    KeyEvent {
                        action,
                        text: Some("a"),
                        ..KeyEvent::press("a")
                    }
                )
                .unwrap()
                .unwrap(),
                expected
            );
        }
        engine.advance(b"\x1b[<u").unwrap();
        assert!(encoded(&mut engine, "a", false, false, false).is_none());
        assert!(
            encode_key(
                &mut engine,
                KeyEvent {
                    action: KeyAction::Release,
                    ..KeyEvent::press("enter")
                }
            )
            .unwrap()
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn conventional_control_aliases_preserve_negotiated_protocols() {
        let mut engine = Engine::new(80, 24).unwrap();
        for (key, expected) in [("i", b"\t".as_slice()), ("m", b"\r"), ("[", b"\x1b")] {
            assert_eq!(
                encoded(&mut engine, key, false, false, true).unwrap(),
                expected
            );
        }
        engine.advance(b"\x1b[>1u").unwrap();
        assert_eq!(
            encoded(&mut engine, "[", false, false, true).unwrap(),
            b"\x1b[91;5u"
        );
        engine.advance(b"\x1b[<u\x1b[>4;2m").unwrap();
        assert_eq!(
            encoded(&mut engine, "[", false, false, true).unwrap(),
            b"\x1b[27;5;91~"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn native_composition_modifiers_do_not_swallow_a_forwarded_key_release() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(b"\x1b[>31u").unwrap();
        assert_eq!(
            encode_key(&mut engine, KeyEvent::press("a"))
                .unwrap()
                .unwrap(),
            b"\x1b[97;;97u"
        );

        // Option is pressed while A is held.
        // New text still belongs to native composition, including repeats.
        for action in [KeyAction::Press, KeyAction::Repeat] {
            assert!(
                encode_key(
                    &mut engine,
                    KeyEvent {
                        action,
                        alt: true,
                        text: Some("é"),
                        ..KeyEvent::press("e")
                    },
                )
                .unwrap()
                .is_none()
            );
        }
        // A's release must reach the TUI with the current modifier state,
        // without inserting text supplied by the platform on a key-up event.
        assert_eq!(
            encode_key(
                &mut engine,
                KeyEvent {
                    action: KeyAction::Release,
                    alt: true,
                    text: Some("a"),
                    ..KeyEvent::press("a")
                },
            )
            .unwrap()
            .unwrap(),
            b"\x1b[97;3:3u"
        );
    }

    #[test]
    fn keypad_identity_is_distinct_from_text_input() {
        let mut engine = Engine::new(80, 24).unwrap();
        assert!(encoded(&mut engine, "1", false, false, false).is_none());
        assert_eq!(
            encoded(&mut engine, "numpad1", false, false, false).unwrap(),
            b"1"
        );
        engine.advance(b"\x1b[?1035l\x1b=").unwrap();
        assert_eq!(
            encoded(&mut engine, "numpad1", false, false, false).unwrap(),
            b"\x1bOq"
        );
        assert_eq!(
            encoded(&mut engine, "kpenter", false, false, false).unwrap(),
            b"\x1bOM"
        );
    }

    #[test]
    fn native_lock_flags_are_not_lost_by_shortcut_modifiers() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(b"\x1b[>31u").unwrap();
        let metadata = gpui::KeyEventMetadata {
            unmodified_text: Some("a".into()),
            modifiers: Default::default(),
            caps_lock: true,
            num_lock: true,
        };
        let result = encode_key(
            &mut engine,
            KeyEvent {
                native_identity: Some(NativeKeyIdentity {
                    key: Key::A,
                    unshifted: 'a',
                }),
                native_modifiers: native_modifiers(Some(&metadata)),
                text: Some("a"),
                ..KeyEvent::press("a")
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(result, b"\x1b[97;193;97u");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_option_metadata_keeps_composition_native_but_releases_forwarded_keys() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(b"\x1b[>31u").unwrap();
        let metadata = gpui::KeyEventMetadata {
            unmodified_text: Some("e".into()),
            modifiers: gpui::Modifiers {
                alt: true,
                ..Default::default()
            },
            caps_lock: false,
            num_lock: false,
        };
        for action in [KeyAction::Press, KeyAction::Repeat] {
            assert!(
                encode_key(
                    &mut engine,
                    KeyEvent {
                        native_identity: Some(NativeKeyIdentity {
                            key: Key::E,
                            unshifted: 'e'
                        }),
                        native_modifiers: native_modifiers(Some(&metadata)),
                        action,
                        text: Some("´"),
                        ..KeyEvent::press("e")
                    }
                )
                .unwrap()
                .is_none()
            );
        }
        assert_eq!(
            encode_key(
                &mut engine,
                KeyEvent {
                    native_identity: Some(NativeKeyIdentity {
                        key: Key::E,
                        unshifted: 'e'
                    }),
                    native_modifiers: native_modifiers(Some(&metadata)),
                    action: KeyAction::Release,
                    ..KeyEvent::press("e")
                }
            )
            .unwrap()
            .unwrap(),
            b"\x1b[101;3:3u"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn option_as_meta_sends_meta_keys_instead_of_composed_text() {
        let mut engine = Engine::new(80, 24).unwrap();
        // macOS reports the composed character (Option+F is "ƒ") as the text.
        let option = |key, text, option_as_meta| KeyEvent {
            alt: true,
            text: Some(text),
            option_as_meta,
            ..KeyEvent::press(key)
        };
        for (key, text, expected) in [
            ("f", "ƒ", b"\x1bf"),
            ("b", "∫", b"\x1bb"),
            ("d", "∂", b"\x1bd"),
        ] {
            assert_eq!(
                encode_key(&mut engine, option(key, text, true))
                    .unwrap()
                    .unwrap(),
                expected,
                "{key}"
            );
            assert!(
                encode_key(&mut engine, option(key, text, false))
                    .unwrap()
                    .is_none(),
                "{key} composes natively when Option is not Meta"
            );
        }
        engine.advance(b"\x1b[>1u").unwrap();
        assert_eq!(
            encode_key(&mut engine, option("f", "ƒ", true))
                .unwrap()
                .unwrap(),
            b"\x1b[102;3u",
            "an enhanced keyboard protocol still reports the unmodified key"
        );
    }

    #[test]
    fn windows_scan_codes_preserve_extended_key_and_layout_independent_identity() {
        for (scan, key) in [
            (0x002, Key::Digit1),
            (0x015, Key::Y),
            (0x027, Key::Semicolon),
            (0x01c, Key::Enter),
            (0x11c, Key::NumpadEnter),
            (0x035, Key::Slash),
            (0x135, Key::NumpadDivide),
            (0x037, Key::NumpadMultiply),
            (0x137, Key::PrintScreen),
            (0x045, Key::Pause),
            (0x145, Key::NumLock),
            (0x047, Key::Numpad7),
            (0x147, Key::Home),
            (0x048, Key::Numpad8),
            (0x148, Key::ArrowUp),
            (0x053, Key::NumpadDecimal),
            (0x153, Key::Delete),
            (0x01d, Key::ControlLeft),
            (0x11d, Key::ControlRight),
            (0x038, Key::AltLeft),
            (0x138, Key::AltRight),
            (0x056, Key::IntlBackslash),
            (0x073, Key::IntlRo),
            (0x07d, Key::IntlYen),
        ] {
            assert_eq!(windows_physical_key(scan), key, "scan {scan:#x}");
        }
        // Don't alias malformed/foreign codes by masking them into a real key.
        for scan in [0, 0x100, 0x200, 0xe01c, 0xffff] {
            assert_eq!(windows_physical_key(scan), Key::Unidentified);
        }
    }

    #[test]
    fn windows_physical_positions_and_layout_scalars_encode_independent_kitty_fields() {
        for (scan, logical, base, text, shift, press, release) in [
            (
                0x002,
                "!",
                '1',
                "!",
                true,
                "\x1b[49:33;2;33u",
                "\x1b[49;1:3u",
            ),
            (
                0x027,
                "Ö",
                'ö',
                "Ö",
                true,
                "\x1b[246:214:59;2;214u",
                "\x1b[246::59;1:3u",
            ),
            (
                0x015,
                "z",
                'z',
                "z",
                false,
                "\x1b[122::121;;122u",
                "\x1b[122::121;1:3u",
            ),
        ] {
            let mut engine = Engine::new(80, 24).unwrap();
            engine.advance(b"\x1b[>31u").unwrap();
            let identity = NativeKeyIdentity {
                key: windows_physical_key(scan),
                unshifted: base,
            };
            let modifiers = current_modifiers(
                gpui::Modifiers {
                    shift,
                    ..Default::default()
                },
                false,
                false,
            );
            assert_eq!(
                encode_key(
                    &mut engine,
                    KeyEvent {
                        native_identity: Some(identity),
                        native_modifiers: Some(modifiers),
                        text: Some(text),
                        ..KeyEvent::press(logical)
                    }
                )
                .unwrap()
                .unwrap(),
                press.as_bytes()
            );
            assert_eq!(
                encode_key(
                    &mut engine,
                    KeyEvent {
                        native_identity: Some(identity),
                        native_modifiers: Some(Modifiers::empty()),
                        action: KeyAction::Release,
                        ..KeyEvent::press(logical)
                    }
                )
                .unwrap()
                .unwrap(),
                release.as_bytes()
            );
        }
    }
}
