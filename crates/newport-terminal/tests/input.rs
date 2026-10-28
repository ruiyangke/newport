use newport_terminal::{
    Engine, Key, KeyAction, KeyInput, Modifiers, MouseAction, MouseButton, MouseInput,
};

fn key(key: Key) -> KeyInput<'static> {
    KeyInput {
        key,
        action: KeyAction::Press,
        modifiers: Modifiers::empty(),
        text: None,
        unshifted: '\0',
        composing: false,
        option_as_alt: false,
    }
}

#[test]
fn key_encoder_follows_application_cursor_and_modifiers() {
    let mut engine = Engine::new(80, 24).unwrap();
    assert_eq!(engine.encode_key(key(Key::ArrowUp)).unwrap(), b"\x1b[A");
    engine.advance(b"\x1b[?1h").unwrap();
    assert_eq!(engine.encode_key(key(Key::ArrowUp)).unwrap(), b"\x1bOA");
    let mut modified = key(Key::ArrowUp);
    modified.modifiers = Modifiers::CTRL | Modifiers::SHIFT;
    assert_eq!(engine.encode_key(modified).unwrap(), b"\x1b[1;6A");
}

#[test]
fn focus_reports_only_when_requested() {
    let mut engine = Engine::new(80, 24).unwrap();
    assert!(engine.encode_focus(true).unwrap().is_empty());
    engine.advance(b"\x1b[?1004h").unwrap();
    assert_eq!(engine.encode_focus(true).unwrap(), b"\x1b[I");
    assert_eq!(engine.encode_focus(false).unwrap(), b"\x1b[O");
}

#[test]
fn paste_is_bracketed_and_cannot_inject_a_terminator() {
    let mut engine = Engine::new(80, 24).unwrap();
    assert_eq!(
        engine.encode_paste("hello\nworld").unwrap(),
        b"hello\rworld"
    );
    engine.advance(b"\x1b[?2004h").unwrap();
    assert_eq!(
        engine.encode_paste("hello\nworld").unwrap(),
        b"\x1b[200~hello\nworld\x1b[201~"
    );
    let bytes = engine.encode_paste("hello\x1b[201~world").unwrap();
    assert_eq!(bytes.windows(6).filter(|w| *w == b"\x1b[201~").count(), 1);
}

#[test]
fn mouse_encoding_tracks_mode_and_grid_metrics() {
    let mut engine = Engine::new(80, 24).unwrap();
    engine.resize(80, 24, 10, 20).unwrap();
    let press = || MouseInput {
        action: MouseAction::Press,
        button: Some(MouseButton::Left),
        modifiers: Modifiers::empty(),
        x: 25.,
        y: 65.,
        any_button_pressed: true,
    };
    assert!(engine.encode_mouse(press()).unwrap().is_empty());
    engine.advance(b"\x1b[?1000h\x1b[?1006h").unwrap();
    assert_eq!(engine.encode_mouse(press()).unwrap(), b"\x1b[<0;3;4M");
}
