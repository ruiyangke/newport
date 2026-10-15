//! Platform pointer identities; Ghostty owns protocol and coordinate encoding.
use newport_terminal::{Engine, Modifiers, MouseAction, MouseButton, MouseInput, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}
impl Button {
    fn bit(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Middle => 2,
            Self::Right => 4,
        }
    }
    fn ghostty(self) -> MouseButton {
        match self {
            Self::Left => MouseButton::Left,
            Self::Middle => MouseButton::Middle,
            Self::Right => MouseButton::Right,
        }
    }
}

/// Releases must be paired independently when multiple buttons are held.
#[derive(Default)]
pub struct PressedButtons(u8);
impl PressedButtons {
    pub fn press(&mut self, button: Button) {
        self.0 |= button.bit();
    }
    pub fn release(&mut self, button: Button) -> bool {
        let pressed = self.0 & button.bit() != 0;
        self.0 &= !button.bit();
        pressed
    }
    pub fn held(&self) -> Option<Button> {
        [Button::Left, Button::Middle, Button::Right]
            .into_iter()
            .find(|button| self.0 & button.bit() != 0)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Press(Button),
    Release(Button),
    Move(Option<Button>),
    WheelUp,
    WheelDown,
}

/// Position is relative to the terminal grid in physical pixels, matching the
/// metrics given to Engine::resize. Shift remains reserved for local selection.
pub fn encode(
    engine: &mut Engine,
    action: Action,
    x: f32,
    y: f32,
    alt: bool,
    control: bool,
) -> Result<Vec<u8>> {
    let (action, button, pressed) = match action {
        Action::Press(button) => (MouseAction::Press, Some(button.ghostty()), true),
        Action::Release(button) => (MouseAction::Release, Some(button.ghostty()), false),
        Action::Move(button) => (
            MouseAction::Motion,
            button.map(Button::ghostty),
            button.is_some(),
        ),
        Action::WheelUp => (MouseAction::Press, Some(MouseButton::Four), false),
        Action::WheelDown => (MouseAction::Press, Some(MouseButton::Five), false),
    };
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::ALT, alt);
    modifiers.set(Modifiers::CTRL, control);
    engine.encode_mouse(MouseInput {
        action,
        button,
        modifiers,
        x,
        y,
        any_button_pressed: pressed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simultaneous_presses_keep_independent_releases() {
        let mut buttons = PressedButtons::default();
        buttons.press(Button::Left);
        buttons.press(Button::Right);
        assert!(buttons.release(Button::Left));
        assert!(!buttons.release(Button::Left));
        assert_eq!(buttons.held(), Some(Button::Right));
        assert!(buttons.release(Button::Right));
        assert_eq!(buttons.held(), None);
    }
    #[test]
    fn sgr_tracks_metrics_release_identity_and_wheel() {
        let mut engine = Engine::new(500, 300).unwrap();
        engine.resize(500, 300, 10, 20).unwrap();
        assert!(
            encode(
                &mut engine,
                Action::Press(Button::Left),
                0.,
                0.,
                false,
                false
            )
            .unwrap()
            .is_empty()
        );
        engine.advance(b"\x1b[?1000h\x1b[?1006h").unwrap();
        assert_eq!(
            encode(
                &mut engine,
                Action::Press(Button::Left),
                0.,
                0.,
                false,
                false
            )
            .unwrap(),
            b"\x1b[<0;1;1M"
        );
        assert_eq!(
            encode(
                &mut engine,
                Action::Release(Button::Right),
                4995.,
                5990.,
                true,
                true
            )
            .unwrap(),
            b"\x1b[<26;500;300m"
        );
        assert_eq!(
            encode(&mut engine, Action::WheelDown, 55., 170., false, false).unwrap(),
            b"\x1b[<65;6;9M"
        );
    }
    #[test]
    fn motion_obeys_application_requested_tracking() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(b"\x1b[?1000h\x1b[?1006h").unwrap();
        assert!(
            encode(
                &mut engine,
                Action::Move(Some(Button::Left)),
                32.,
                112.,
                false,
                false
            )
            .unwrap()
            .is_empty()
        );
        engine.advance(b"\x1b[?1002h").unwrap();
        assert!(
            encode(&mut engine, Action::Move(None), 32., 112., false, false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            encode(
                &mut engine,
                Action::Move(Some(Button::Middle)),
                32.,
                112.,
                false,
                false
            )
            .unwrap(),
            b"\x1b[<33;5;8M"
        );
        engine.advance(b"\x1b[?1003h").unwrap();
        assert_eq!(
            encode(&mut engine, Action::Move(None), 40., 112., false, false).unwrap(),
            b"\x1b[<35;6;8M"
        );
    }
    #[test]
    fn legacy_coordinates_are_bounded_and_utf8_extends_them() {
        let mut engine = Engine::new(500, 300).unwrap();
        engine.resize(500, 300, 8, 16).unwrap();
        engine.advance(b"\x1b[?1000h").unwrap();
        assert_eq!(
            encode(
                &mut engine,
                Action::Release(Button::Right),
                0.,
                0.,
                false,
                false
            )
            .unwrap(),
            b"\x1b[M#!!"
        );
        assert_eq!(
            encode(
                &mut engine,
                Action::Press(Button::Left),
                222. * 8.,
                0.,
                false,
                false
            )
            .unwrap(),
            [27, 91, 77, 32, 255, 33]
        );
        assert!(
            encode(
                &mut engine,
                Action::Press(Button::Left),
                223. * 8.,
                0.,
                false,
                false
            )
            .unwrap()
            .is_empty()
        );
        engine.advance(b"\x1b[?1005h").unwrap();
        assert_eq!(
            encode(
                &mut engine,
                Action::Press(Button::Left),
                223. * 8.,
                0.,
                false,
                false
            )
            .unwrap(),
            "\x1b[M \u{100}!".as_bytes()
        );
    }

    #[test]
    fn pixel_tracking_uses_physical_position_instead_of_cell_indices() {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.resize(80, 24, 10, 20).unwrap();
        engine.advance(b"\x1b[?1003h\x1b[?1016h").unwrap();
        assert_eq!(
            encode(&mut engine, Action::Move(None), 25., 65., false, false).unwrap(),
            b"\x1b[<35;25;65M"
        );
        // Two positions within one cell still produce different pixel reports.
        assert_eq!(
            encode(&mut engine, Action::Move(None), 26., 65., false, false).unwrap(),
            b"\x1b[<35;26;65M"
        );
    }
}
