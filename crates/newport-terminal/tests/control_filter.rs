use newport_terminal::Engine;

fn render(bytes: &[u8], chunk: usize) -> String {
    let mut engine = Engine::new(200, 3).unwrap();
    for piece in bytes.chunks(chunk) {
        engine.advance(piece).unwrap();
    }
    engine.visible_lines().unwrap().join("\n")
}

#[test]
fn delete_control_never_becomes_printable_at_batch_boundaries() {
    for offset in 0..129 {
        let mut input = vec![b'A'; 129];
        input[offset] = 0x7f;
        for chunk in [1, 2, 7, 16, 128, 1024] {
            assert_eq!(
                render(&input, chunk),
                "A".repeat(128),
                "offset={offset}, chunk={chunk}"
            );
        }
    }
}

#[test]
fn delete_preserves_grapheme_and_repeat_state() {
    for chunk in [1, 2, 3, 7, 1024] {
        assert_eq!(
            render("👩\u{200d}\x7f💻!".as_bytes(), chunk),
            "👩\u{200d}💻!"
        );
        assert_eq!(render(b"a\x7f\x1b[2b!", chunk), "aaa!");
        assert_eq!(render(b"\x1b[3\x7f1mX", chunk), "X");
        assert_eq!(render(b"\xc2\x7fB", chunk), "\u{fffd}B");
    }
}
