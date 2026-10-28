//! Reverse video, extra mouse encodings, modifyOtherKeys and terminal reports.
use newport_terminal::{
    Engine, Key, KeyAction, KeyInput, Mode, Modifiers, MouseAction, MouseButton, MouseInput,
};

fn replies(engine: &mut Engine, input: &[u8]) -> String {
    String::from_utf8(engine.advance(input).unwrap().replies.concat()).unwrap()
}

fn press(
    engine: &mut Engine,
    key: Key,
    modifiers: Modifiers,
    text: Option<&str>,
    unshifted: char,
) -> Vec<u8> {
    engine
        .encode_key(KeyInput {
            key,
            action: KeyAction::Press,
            modifiers,
            text,
            unshifted,
            composing: false,
            option_as_alt: true,
        })
        .unwrap()
}

fn click(
    engine: &mut Engine,
    action: MouseAction,
    x: f32,
    y: f32,
    modifiers: Modifiers,
) -> Vec<u8> {
    engine
        .encode_mouse(MouseInput {
            action,
            button: Some(MouseButton::Left),
            modifiers,
            x,
            y,
            any_button_pressed: action != MouseAction::Release,
        })
        .unwrap()
}

#[test]
fn reverse_video_swaps_the_default_colors_and_reports_its_state() {
    let mut engine = Engine::new(10, 2).unwrap();
    engine.advance(b"text").unwrap();
    let normal = engine.frame().unwrap().clone();
    engine.advance(b"\x1b[?5h").unwrap();
    let reversed = engine.frame().unwrap().clone();
    assert_eq!(reversed.foreground, normal.background);
    assert_eq!(reversed.background, normal.foreground);
    assert_eq!(replies(&mut engine, b"\x1b[?5$p"), "\x1b[?5;1$y");
    engine.advance(b"\x1b[?5l").unwrap();
    assert_eq!(engine.frame().unwrap().background, normal.background);
    assert_eq!(replies(&mut engine, b"\x1b[?5$p"), "\x1b[?5;2$y");
}

#[test]
fn x10_mouse_reports_presses_only_without_modifiers() {
    let mut engine = Engine::new(80, 24).unwrap();
    engine.resize(80, 24, 10, 20).unwrap();
    engine.advance(b"\x1b[?9h").unwrap();
    assert!(engine.mode(Mode::X10_MOUSE).unwrap());
    assert_eq!(
        click(&mut engine, MouseAction::Press, 25., 65., Modifiers::CTRL),
        b"\x1b[M #$"
    );
    assert!(
        click(
            &mut engine,
            MouseAction::Release,
            25.,
            65.,
            Modifiers::empty()
        )
        .is_empty()
    );
    engine.advance(b"\x1b[?9l").unwrap();
    assert!(
        click(
            &mut engine,
            MouseAction::Press,
            25.,
            65.,
            Modifiers::empty()
        )
        .is_empty()
    );
}

#[test]
fn urxvt_and_pixel_mouse_encodings() {
    let mut engine = Engine::new(80, 24).unwrap();
    engine.resize(80, 24, 10, 20).unwrap();
    engine.advance(b"\x1b[?1000h\x1b[?1015h").unwrap();
    assert_eq!(
        click(
            &mut engine,
            MouseAction::Press,
            2500.,
            65.,
            Modifiers::empty()
        ),
        b"\x1b[32;80;4M"
    );
    engine
        .advance(b"\x1b[?1015l\x1b[?1003h\x1b[?1016h")
        .unwrap();
    for (x, report) in [(25., "\x1b[<35;25;65M"), (26., "\x1b[<35;26;65M")] {
        let motion = engine
            .encode_mouse(MouseInput {
                action: MouseAction::Motion,
                button: None,
                modifiers: Modifiers::empty(),
                x,
                y: 65.,
                any_button_pressed: false,
            })
            .unwrap();
        assert_eq!(motion, report.as_bytes());
    }
    assert_eq!(replies(&mut engine, b"\x1b[?1016$p"), "\x1b[?1016;1$y");
}

#[test]
fn modify_other_keys_reports_modified_keys() {
    let mut engine = Engine::new(20, 2).unwrap();
    let ctrl = Modifiers::CTRL;
    assert_eq!(
        press(&mut engine, Key::BracketLeft, ctrl, None, '['),
        b"\x1b"
    );
    engine.advance(b"\x1b[>4;2m").unwrap();
    assert_eq!(replies(&mut engine, b"\x1b[?4m"), "\x1b[>4;2m");
    assert_eq!(
        press(&mut engine, Key::BracketLeft, ctrl, None, '['),
        b"\x1b[27;5;91~"
    );
    assert_eq!(
        press(&mut engine, Key::A, ctrl, None, 'a'),
        b"\x1b[27;5;97~"
    );
    assert_eq!(
        press(&mut engine, Key::Enter, Modifiers::SHIFT, None, '\0'),
        b"\x1b[27;2;13~"
    );
    assert_eq!(
        press(&mut engine, Key::A, Modifiers::SHIFT, Some("A"), 'a'),
        b"A"
    );
    // Level 1 changes only keys without a conventional encoding.
    engine.advance(b"\x1b[>4;1m").unwrap();
    assert_eq!(press(&mut engine, Key::A, ctrl, None, 'a'), b"\x01");
    assert_eq!(
        press(&mut engine, Key::Semicolon, ctrl, None, ';'),
        b"\x1b[27;5;59~"
    );
    engine.advance(b"\x1b[>4m").unwrap();
    assert_eq!(
        press(&mut engine, Key::Semicolon, ctrl, None, ';'),
        b"\x1b[59;5u"
    );
}

#[test]
fn capability_setting_and_version_queries_are_answered_in_order() {
    let mut engine = Engine::new(20, 2).unwrap();
    assert_eq!(
        replies(&mut engine, b"\x1b[5n\x1bP+q5463;524742;4d73\x1b\\\x1b[6n"),
        concat!(
            "\x1b[0n",
            "\x1bP1+r5463\x1b\\",
            "\x1bP1+r524742=38\x1b\\",
            "\x1bP1+r4D73=5C455D35323B25703125733B25703225735C303037\x1b\\",
            "\x1b[1;1R",
        )
    );
    assert_eq!(
        replies(&mut engine, b"\x1bP+q6e6f\x1b\\"),
        "\x1bP0+r6E6F\x1b\\"
    );
    assert_eq!(
        replies(&mut engine, b"\x1b[1;4;38;2;1;2;3m\x1bP$qm\x1b\\"),
        "\x1bP1$r0;1;4;38:2::1:2:3m\x1b\\"
    );
    assert_eq!(
        replies(&mut engine, b"\x1b[4 q\x1bP$q q\x1b\\"),
        "\x1bP1$r4 q\x1b\\"
    );
    assert_eq!(replies(&mut engine, b"\x1bP$qr\x1b\\"), "\x1bP0$r\x1b\\");
    assert!(replies(&mut engine, b"\x1b[>q").starts_with("\x1bP>|Newport "));
    assert_eq!(replies(&mut engine, b"\x1b[c"), "\x1b[?62;22c");
}

#[test]
fn queries_split_across_chunks_are_answered_once() {
    let mut engine = Engine::new(20, 2).unwrap();
    let mut all = Vec::new();
    for byte in b"\x1bP+q5463\x1b\\\x1b[>q" {
        all.extend(
            engine
                .advance(std::slice::from_ref(byte))
                .unwrap()
                .replies
                .concat(),
        );
    }
    let all = String::from_utf8(all).unwrap();
    assert!(
        all.starts_with("\x1bP1+r5463\x1b\\\x1bP>|Newport "),
        "{all:?}"
    );
}

#[test]
fn reconnect_resets_extension_modes() {
    let mut engine = Engine::new(20, 2).unwrap();
    engine
        .advance(b"\x1b[?5h\x1b[?9h\x1b[>4;2m\x1b[?2027l")
        .unwrap();
    engine.reconnect().unwrap();
    assert!(!engine.mode(Mode::X10_MOUSE).unwrap());
    assert_eq!(
        replies(&mut engine, b"\x1b[?5$p\x1b[?2027$p\x1b[?4m"),
        "\x1b[?5;2$y\x1b[?2027;1$y\x1b[>4;0m"
    );
}
