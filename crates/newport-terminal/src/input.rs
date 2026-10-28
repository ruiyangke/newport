use crate::KittyKeyFlags;
use crate::handler::Modes;
use crate::{Key, KeyAction, Modifiers, MouseAction, MouseButton, Result};
use alacritty_terminal::term::TermMode;

/// Platform key identity and layout text. UI shortcuts are handled before this
/// reaches the encoder. IME commits are passed as text, never fabricated keys.
pub struct KeyInput<'a> {
    pub key: Key,
    pub action: KeyAction,
    pub modifiers: Modifiers,
    pub text: Option<&'a str>,
    pub unshifted: char,
    pub composing: bool,
    pub option_as_alt: bool,
}

pub struct MouseInput {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub modifiers: Modifiers,
    /// Coordinates relative to the terminal grid, in physical pixels.
    pub x: f32,
    pub y: f32,
    pub any_button_pressed: bool,
}

/// Encoding belongs to the host (alacritty_terminal exposes negotiated modes,
/// not a platform keyboard API). No application shortcuts live here.
pub(crate) struct InputState {
    columns: u16,
    rows: u16,
    width: u16,
    height: u16,
    last_motion: Option<(u32, u32, u8)>,
}
impl InputState {
    pub fn new(columns: u16, rows: u16) -> Result<Self> {
        Ok(Self {
            columns,
            rows,
            width: 8,
            height: 16,
            last_motion: None,
        })
    }
    pub fn reset(&mut self) {
        self.last_motion = None;
    }
    pub fn resize(&mut self, columns: u16, rows: u16, width: u16, height: u16) {
        self.columns = columns;
        self.rows = rows;
        self.width = width;
        self.height = height;
        self.reset();
    }
    pub fn key(&mut self, mode: TermMode, modes: &Modes, input: KeyInput<'_>) -> Result<Vec<u8>> {
        Ok(encode_key(mode, modes.modify_other_keys, input))
    }
    pub fn mouse(&mut self, mode: TermMode, modes: &Modes, input: MouseInput) -> Result<Vec<u8>> {
        if !input.x.is_finite() || !input.y.is_finite() {
            return Err(crate::Error::InvalidValue);
        }
        // X10 reporting (mode 9) sends button presses only, without modifiers.
        let x10 = modes.x10_mouse && !mode.intersects(TermMode::MOUSE_MODE);
        if !mode.intersects(TermMode::MOUSE_MODE) && !x10
            || x10 && input.action != MouseAction::Press
        {
            return Ok(Vec::new());
        }
        let motion = input.action == MouseAction::Motion;
        if motion
            && !(mode.contains(TermMode::MOUSE_MOTION)
                || mode.contains(TermMode::MOUSE_DRAG) && input.any_button_pressed)
        {
            return Ok(Vec::new());
        }
        let (x, y) = if modes.pixel_mouse {
            let width = u32::from(self.columns) * u32::from(self.width);
            let height = u32::from(self.rows) * u32::from(self.height);
            (
                (input.x.max(0.) as u32).min(width - 1),
                (input.y.max(0.) as u32).min(height - 1),
            )
        } else {
            (
                u32::from(((input.x.max(0.) / self.width as f32) as u16).min(self.columns - 1) + 1),
                u32::from(((input.y.max(0.) / self.height as f32) as u16).min(self.rows - 1) + 1),
            )
        };
        let mut code = match input.button {
            Some(MouseButton::Left) => 0,
            Some(MouseButton::Middle) => 1,
            Some(MouseButton::Right) => 2,
            Some(MouseButton::Four) => 64,
            Some(MouseButton::Five) => 65,
            Some(MouseButton::Six) => 66,
            Some(MouseButton::Seven) => 67,
            Some(MouseButton::Eight) => 128,
            Some(MouseButton::Nine) => 129,
            Some(MouseButton::Ten) => 130,
            Some(MouseButton::Eleven) => 131,
            None => 3,
        };
        let release = input.action == MouseAction::Release;
        if release && (64..=67).contains(&code) {
            return Ok(Vec::new());
        }
        let sgr = mode.contains(TermMode::SGR_MOUSE) || modes.pixel_mouse;
        if release && !sgr {
            code = 3;
        }
        if motion {
            code += 32;
        }
        if !x10 {
            if input.modifiers.contains(Modifiers::SHIFT) {
                code += 4;
            }
            if input.modifiers.contains(Modifiers::ALT) {
                code += 8;
            }
            if input.modifiers.contains(Modifiers::CTRL) {
                code += 16;
            }
        }
        let position = (x, y, code);
        if motion && self.last_motion == Some(position) {
            return Ok(Vec::new());
        }
        self.last_motion = Some(position);
        if sgr {
            return Ok(
                format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes(),
            );
        }
        if modes.urxvt_mouse {
            return Ok(format!("\x1b[{};{x};{y}M", u32::from(code) + 32).into_bytes());
        }
        let mut out = b"\x1b[M".to_vec();
        out.push(code + 32);
        if mode.contains(TermMode::UTF8_MOUSE) {
            for n in [x, y] {
                if n > 2015 {
                    return Ok(Vec::new());
                }
                let mut buffer = [0; 4];
                out.extend_from_slice(
                    char::from_u32(n + 32)
                        .unwrap()
                        .encode_utf8(&mut buffer)
                        .as_bytes(),
                );
            }
        } else {
            if x > 223 || y > 223 {
                return Ok(Vec::new());
            }
            out.extend_from_slice(&[(x + 32) as u8, (y + 32) as u8]);
        }
        Ok(out)
    }
}
pub(crate) fn kitty_flags(mode: TermMode) -> KittyKeyFlags {
    let mut flags = KittyKeyFlags::empty();
    for (m, f) in [
        (
            TermMode::DISAMBIGUATE_ESC_CODES,
            KittyKeyFlags::DISAMBIGUATE,
        ),
        (TermMode::REPORT_EVENT_TYPES, KittyKeyFlags::REPORT_EVENTS),
        (
            TermMode::REPORT_ALTERNATE_KEYS,
            KittyKeyFlags::REPORT_ALTERNATES,
        ),
        (TermMode::REPORT_ALL_KEYS_AS_ESC, KittyKeyFlags::REPORT_ALL),
        (TermMode::REPORT_ASSOCIATED_TEXT, KittyKeyFlags::REPORT_TEXT),
    ] {
        flags.set(f, mode.contains(m));
    }
    flags
}
fn encode_key(mode: TermMode, modify_other_keys: u8, input: KeyInput<'_>) -> Vec<u8> {
    use Key::*;
    if input.composing {
        return Vec::new();
    }
    let flags = kitty_flags(mode);
    let release = input.action == KeyAction::Release;
    if release && !flags.contains(KittyKeyFlags::REPORT_EVENTS) {
        return Vec::new();
    }
    let enhanced = flags.intersects(
        KittyKeyFlags::DISAMBIGUATE | KittyKeyFlags::REPORT_ALL | KittyKeyFlags::REPORT_EVENTS,
    );
    let all = flags.contains(KittyKeyFlags::REPORT_ALL);
    let mut mods = input.modifiers;
    if !enhanced {
        mods.remove(Modifiers::CAPS_LOCK | Modifiers::NUM_LOCK);
    }
    let unshifted = if input.unshifted != '\0' {
        Some(input.unshifted)
    } else {
        key_char(input.key)
    };
    let text = input.text.filter(|s| !s.is_empty());
    let keypad = keypad(input.key);
    let event = flags.contains(KittyKeyFlags::REPORT_EVENTS) && input.action != KeyAction::Press;
    let named = matches!(input.key, Enter | Tab | Escape | Backspace);
    let encode = enhanced
        && (all
            || release
            || input.action == KeyAction::Repeat && flags.contains(KittyKeyFlags::REPORT_EVENTS)
            || keypad.is_some()
            || input.key == Escape
            || mods.intersects(Modifiers::CTRL | Modifiers::ALT | Modifiers::SUPER)
            || named && mods.contains(Modifiers::SHIFT));
    let mut base = None;
    if enhanced && let Some((_, _, code)) = keypad {
        base = Some((code.to_string(), 'u'));
    }
    if base.is_none() {
        base = match input.key {
            ArrowUp => Some(("1".into(), 'A')),
            ArrowDown => Some(("1".into(), 'B')),
            ArrowRight => Some(("1".into(), 'C')),
            ArrowLeft => Some(("1".into(), 'D')),
            Home => Some(("1".into(), 'H')),
            End => Some(("1".into(), 'F')),
            PageUp => Some(("5".into(), '~')),
            PageDown => Some(("6".into(), '~')),
            Insert => Some(("2".into(), '~')),
            Delete => Some(("3".into(), '~')),
            F1 => Some(("1".into(), 'P')),
            F2 => Some(("1".into(), 'Q')),
            // CSI 1;<mods>R would read as a cursor position report.
            F3 if enhanced || !mods.is_empty() => Some(("13".into(), '~')),
            F3 => Some(("1".into(), 'R')),
            F4 => Some(("1".into(), 'S')),
            _ => function_number(input.key).and_then(|n| {
                if enhanced && n >= 13 {
                    Some(((57376 + n - 13).to_string(), 'u'))
                } else {
                    const CODES: [u32; 21] = [
                        15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34, 42, 43, 44,
                        45, 46,
                    ];
                    CODES
                        .get(n.checked_sub(5)? as usize)
                        .map(|code| (code.to_string(), '~'))
                }
            }),
        };
    }
    if base.is_none() && enhanced {
        let code = match input.key {
            ScrollLock => Some(57359),
            PrintScreen => Some(57361),
            Pause => Some(57362),
            ContextMenu => Some(57363),
            MediaPlayPause => Some(57430),
            MediaStop => Some(57432),
            MediaTrackNext => Some(57435),
            MediaTrackPrevious => Some(57436),
            AudioVolumeDown => Some(57438),
            AudioVolumeUp => Some(57439),
            AudioVolumeMute => Some(57440),
            _ => None,
        };
        base = code.map(|code| (code.to_string(), 'u'));
    }
    if base.is_none() && encode {
        let code = match input.key {
            Enter => Some(13),
            Tab => Some(9),
            Escape => Some(27),
            Backspace => Some(127),
            CapsLock if all => Some(57358),
            NumLock if all => Some(57360),
            ScrollLock => Some(57359),
            PrintScreen => Some(57361),
            Pause => Some(57362),
            ContextMenu => Some(57363),
            ShiftLeft if all => Some(57441),
            ControlLeft if all => Some(57442),
            AltLeft if all => Some(57443),
            MetaLeft if all => Some(57444),
            ShiftRight if all => Some(57447),
            ControlRight if all => Some(57448),
            AltRight if all => Some(57449),
            MetaRight if all => Some(57450),
            _ => unshifted.map(u32::from),
        };
        if let Some(code) = code {
            let modifier = match input.key {
                ShiftLeft | ShiftRight => Some(Modifiers::SHIFT),
                ControlLeft | ControlRight => Some(Modifiers::CTRL),
                AltLeft | AltRight => Some(Modifiers::ALT),
                MetaLeft | MetaRight => Some(Modifiers::SUPER),
                _ => None,
            };
            if let Some(modifier) = modifier {
                mods.set(modifier, !release);
            }
            let mut payload = code.to_string();
            if flags.contains(KittyKeyFlags::REPORT_ALTERNATES)
                && mods.contains(Modifiers::SHIFT)
                && let Some(ch) = text.and_then(|s| s.chars().next())
                && ch as u32 != code
            {
                payload.push_str(&format!(":{}", ch as u32));
            }
            if flags.contains(KittyKeyFlags::REPORT_ALTERNATES)
                && let Some(physical) = key_char(input.key)
                && physical as u32 != code
            {
                if !payload.contains(':') {
                    payload.push(':');
                }
                payload.push_str(&format!(":{}", physical as u32));
            }
            base = Some((payload, 'u'));
        }
    }
    if let Some((base, final_char)) = base {
        if !enhanced && mods.is_empty() && matches!(input.key, F1 | F2 | F3 | F4) {
            return format!("\x1bO{final_char}").into_bytes();
        }
        if !enhanced
            && mods.is_empty()
            && mode.contains(TermMode::APP_CURSOR)
            && matches!(
                input.key,
                ArrowUp | ArrowDown | ArrowLeft | ArrowRight | Home | End
            )
        {
            return format!("\x1bO{final_char}").into_bytes();
        }
        // Chords with Ctrl, Super or Alt-as-Meta are commands, not text.
        let command = mods.intersects(Modifiers::CTRL | Modifiers::SUPER)
            || mods.contains(Modifiers::ALT) && input.option_as_alt;
        let associated = if !release && !command && flags.contains(KittyKeyFlags::REPORT_TEXT) {
            text.filter(|s| !s.chars().any(char::is_control))
        } else {
            None
        };
        let mut out = String::from("\x1b[");
        if base != "1"
            || final_char == 'u'
            || final_char == '~'
            || !mods.is_empty()
            || event
            || associated.is_some()
        {
            out.push_str(&base);
        }
        if !mods.is_empty() || event {
            out.push_str(&format!(";{}", mods.bits() as u16 + 1));
        } else if associated.is_some() {
            out.push(';');
        }
        if event {
            out.push_str(if release { ":3" } else { ":2" });
        }
        if let Some(text) = associated {
            out.push(';');
            out.push_str(
                &text
                    .chars()
                    .map(|c| (c as u32).to_string())
                    .collect::<Vec<_>>()
                    .join(":"),
            );
        }
        out.push(final_char);
        return out.into_bytes();
    }
    if release {
        return Vec::new();
    }
    if keypad.is_none()
        && let Some(bytes) =
            modified_key(input.key, unshifted, chord(mods, &input), modify_other_keys)
    {
        return bytes;
    }
    let mut bytes = if let Some((ch, app, _)) = keypad {
        if mode.contains(TermMode::APP_KEYPAD) {
            format!("\x1bO{app}").into_bytes()
        } else {
            ch.to_string().into_bytes()
        }
    } else {
        match input.key {
            Enter => vec![b'\r'],
            Tab if mods.contains(Modifiers::SHIFT) => b"\x1b[Z".to_vec(),
            Tab => vec![b'\t'],
            Escape => vec![27],
            Backspace if mods.contains(Modifiers::CTRL) => vec![8],
            Backspace => vec![127],
            _ => {
                if mods.contains(Modifiers::CTRL) {
                    match unshifted.and_then(control_byte) {
                        Some(byte) => vec![byte],
                        None => text.unwrap_or_default().as_bytes().to_vec(),
                    }
                } else if mods.contains(Modifiers::ALT) && input.option_as_alt {
                    unshifted
                        .map(|c| {
                            if mods.contains(Modifiers::SHIFT) {
                                c.to_uppercase().collect::<String>()
                            } else {
                                c.to_string()
                            }
                        })
                        .unwrap_or_default()
                        .into_bytes()
                } else {
                    text.map(str::as_bytes).unwrap_or_default().to_vec()
                }
            }
        }
    };
    if mods.contains(Modifiers::ALT) && input.option_as_alt && !bytes.is_empty() {
        bytes.insert(0, 27);
    }
    bytes
}
/// Modifiers that change a legacy key report: Alt counts only as Meta.
fn chord(mods: Modifiers, input: &KeyInput<'_>) -> Modifiers {
    let mut chord = mods & (Modifiers::SHIFT | Modifiers::CTRL | Modifiers::SUPER);
    if mods.contains(Modifiers::ALT) && input.option_as_alt {
        chord |= Modifiers::ALT;
    }
    chord
}

/// Legacy reports for modified keys that have no conventional byte, so a
/// program can tell them apart: `CSI 27;<mods>;<code>~` for Enter, Tab and
/// Escape, and `CSI <code>;<mods>u` for Ctrl chords without a control byte.
/// xterm's modifyOtherKeys selects `CSI 27` for these (level 1) or for every
/// modified key (level 2).
fn modified_key(
    key: Key,
    unshifted: Option<char>,
    chord: Modifiers,
    modify_other_keys: u8,
) -> Option<Vec<u8>> {
    if chord.is_empty() {
        return None;
    }
    let parameter = u16::from(chord.bits()) + 1;
    let other_keys = |code: u32| Some(format!("\x1b[27;{parameter};{code}~").into_bytes());
    let named = match key {
        Key::Enter => Some(13),
        Key::Tab => Some(9),
        Key::Escape => Some(27),
        Key::Backspace => Some(127),
        _ => None,
    };
    if let Some(code) = named {
        if modify_other_keys == 2 {
            return other_keys(code);
        }
        // Backspace keeps DEL/BS, Shift+Tab is backtab, and Alt prefixes ESC.
        let conventional = key == Key::Backspace
            || !chord.intersects(Modifiers::CTRL | Modifiers::SUPER)
                && (key == Key::Tab || !chord.contains(Modifiers::SHIFT));
        return if conventional { None } else { other_keys(code) };
    }
    let character = unshifted.filter(|c| !c.is_control())?;
    if modify_other_keys == 2
        && (chord.intersects(Modifiers::CTRL | Modifiers::ALT | Modifiers::SUPER)
            || character == ' ')
    {
        return other_keys(character as u32);
    }
    if !chord.contains(Modifiers::CTRL) {
        return None;
    }
    let shifted_letter = chord.contains(Modifiers::SHIFT) && character.is_ascii_alphabetic();
    if control_byte(character).is_some() && !shifted_letter {
        return None;
    }
    let code = character.to_ascii_lowercase() as u32;
    if modify_other_keys == 1 {
        other_keys(code)
    } else {
        Some(format!("\x1b[{code};{parameter}u").into_bytes())
    }
}

/// The C0 byte that Ctrl plus this key conventionally sends.
fn control_byte(character: char) -> Option<u8> {
    Some(match character.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        ' ' | '@' | '2' => 0,
        '[' | '3' => 27,
        '\\' | '4' => 28,
        ']' | '5' => 29,
        '^' | '6' => 30,
        '_' | '7' | '/' => 31,
        '?' | '8' => 127,
        _ => return None,
    })
}

fn keypad(key: Key) -> Option<(char, char, u32)> {
    use Key::*;
    Some(match key {
        Numpad0 => ('0', 'p', 57399),
        Numpad1 => ('1', 'q', 57400),
        Numpad2 => ('2', 'r', 57401),
        Numpad3 => ('3', 's', 57402),
        Numpad4 => ('4', 't', 57403),
        Numpad5 => ('5', 'u', 57404),
        Numpad6 => ('6', 'v', 57405),
        Numpad7 => ('7', 'w', 57406),
        Numpad8 => ('8', 'x', 57407),
        Numpad9 => ('9', 'y', 57408),
        NumpadDecimal => ('.', 'n', 57409),
        NumpadDivide => ('/', 'o', 57410),
        NumpadMultiply => ('*', 'j', 57411),
        NumpadSubtract => ('-', 'm', 57412),
        NumpadAdd => ('+', 'k', 57413),
        NumpadEnter => ('\r', 'M', 57414),
        NumpadEqual => ('=', 'X', 57415),
        NumpadComma | NumpadSeparator => (',', 'l', 57416),
        _ => return None,
    })
}
fn function_number(key: Key) -> Option<u32> {
    Some(match key {
        Key::F1 => 1,
        Key::F2 => 2,
        Key::F3 => 3,
        Key::F4 => 4,
        Key::F5 => 5,
        Key::F6 => 6,
        Key::F7 => 7,
        Key::F8 => 8,
        Key::F9 => 9,
        Key::F10 => 10,
        Key::F11 => 11,
        Key::F12 => 12,
        Key::F13 => 13,
        Key::F14 => 14,
        Key::F15 => 15,
        Key::F16 => 16,
        Key::F17 => 17,
        Key::F18 => 18,
        Key::F19 => 19,
        Key::F20 => 20,
        Key::F21 => 21,
        Key::F22 => 22,
        Key::F23 => 23,
        Key::F24 => 24,
        Key::F25 => 25,
        Key::F26 => 26,
        Key::F27 => 27,
        Key::F28 => 28,
        Key::F29 => 29,
        Key::F30 => 30,
        Key::F31 => 31,
        Key::F32 => 32,
        Key::F33 => 33,
        Key::F34 => 34,
        Key::F35 => 35,
        _ => return None,
    })
}
fn key_char(key: Key) -> Option<char> {
    Some(match key {
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        Key::Digit0 => '0',
        Key::Digit1 => '1',
        Key::Digit2 => '2',
        Key::Digit3 => '3',
        Key::Digit4 => '4',
        Key::Digit5 => '5',
        Key::Digit6 => '6',
        Key::Digit7 => '7',
        Key::Digit8 => '8',
        Key::Digit9 => '9',
        Key::Space => ' ',
        Key::Backquote => '`',
        Key::Minus => '-',
        Key::Equal => '=',
        Key::BracketLeft => '[',
        Key::BracketRight => ']',
        Key::Backslash => '\\',
        Key::Semicolon => ';',
        Key::Quote => '\'',
        Key::Comma => ',',
        Key::Period => '.',
        Key::Slash => '/',
        _ => return None,
    })
}
