use crate::{Key, KeyAction, Modifiers, MouseAction, MouseButton, Result};
use libghostty_vt::{Terminal, key, mouse};

/// Platform key identity and layout text. UI shortcuts are handled before this
/// reaches the encoder. IME commits are passed as text, never fabricated keys.
pub struct KeyInput<'a> {
    pub key: Key,
    pub action: KeyAction,
    pub modifiers: Modifiers,
    pub consumed_modifiers: Modifiers,
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

pub(crate) struct InputState {
    key: key::Encoder<'static>,
    key_event: key::Event<'static>,
    mouse: mouse::Encoder<'static>,
    mouse_event: mouse::Event<'static>,
}

impl InputState {
    pub fn reset(&mut self) {
        self.mouse.reset();
    }

    pub fn new(columns: u16, rows: u16) -> Result<Self> {
        let mut state = Self {
            key: key::Encoder::new()?,
            key_event: key::Event::new()?,
            mouse: mouse::Encoder::new()?,
            mouse_event: mouse::Event::new()?,
        };
        state.resize(columns, rows, 8, 16);
        Ok(state)
    }

    pub fn resize(&mut self, columns: u16, rows: u16, width: u16, height: u16) {
        self.mouse.set_size(mouse::EncoderSize {
            screen_width: u32::from(columns) * u32::from(width),
            screen_height: u32::from(rows) * u32::from(height),
            cell_width: width.into(),
            cell_height: height.into(),
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
        });
        self.mouse.reset();
    }

    pub fn key(&mut self, terminal: &Terminal<'_, '_>, input: KeyInput<'_>) -> Result<Vec<u8>> {
        self.key
            .set_options_from_terminal(terminal)
            .set_macos_option_as_alt(if input.option_as_alt {
                key::OptionAsAlt::True
            } else {
                key::OptionAsAlt::False
            });
        self.key_event
            .set_key(input.key)
            .set_action(input.action)
            .set_mods(input.modifiers)
            .set_consumed_mods(input.consumed_modifiers)
            .set_utf8(input.text)
            .set_unshifted_codepoint(input.unshifted)
            .set_composing(input.composing);
        let mut output = Vec::new();
        self.key.encode_to_vec(&self.key_event, &mut output)?;
        Ok(output)
    }

    pub fn mouse(&mut self, terminal: &Terminal<'_, '_>, input: MouseInput) -> Result<Vec<u8>> {
        if !input.x.is_finite() || !input.y.is_finite() {
            return Err(crate::Error::InvalidValue);
        }
        self.mouse
            .set_options_from_terminal(terminal)
            .set_any_button_pressed(input.any_button_pressed);
        self.mouse_event
            .set_action(input.action)
            .set_button(input.button)
            .set_mods(input.modifiers)
            .set_position(mouse::Position {
                x: input.x,
                y: input.y,
            });
        let mut output = Vec::new();
        self.mouse.encode_to_vec(&self.mouse_event, &mut output)?;
        Ok(output)
    }
}
