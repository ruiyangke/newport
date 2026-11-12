use super::*;
fn window_state() -> WindowState {
    WindowState {
        window: 7,
        generation: 3,
        kind: WindowKind::Dialog,
        parent: 2,
        modal: true,
        position: [-40, 12],
        logical: [640, 480],
        pixels: [1280, 960],
        apply_size: false,
        decoration: Decoration::Client,
        actions: Actions {
            close: true,
            minimize: false,
            maximize: true,
            resize: true,
            fullscreen: false,
        },
        min: Some([200, 100]),
        max: None,
        title: "Préférences".into(),
        activate: Activate::Attention,
    }
}

fn stub_messages() -> Vec<ToStub> {
    vec![
        ToStub::Welcome {
            name: "Calculator".into(),
            server: "devbox".into(),
        },
        ToStub::Window {
            state: window_state(),
            surfaces: 2,
        },
        ToStub::Surface {
            window: 7,
            generation: 3,
            index: 2,
        },
        ToStub::Frame {
            window: 7,
            generation: 3,
            index: 1,
        },
        ToStub::Title {
            window: 7,
            title: "Untitled".into(),
        },
        ToStub::Operation {
            window: 7,
            operation: Operation::Move,
        },
        ToStub::Operation {
            window: 7,
            operation: Operation::Minimize,
        },
        ToStub::Operation {
            window: 7,
            operation: Operation::Zoom { maximized: true },
        },
        ToStub::Operation {
            window: 7,
            operation: Operation::Fullscreen { enabled: false },
        },
        ToStub::Operation {
            window: 7,
            operation: Operation::Activate,
        },
        ToStub::Destroy { window: 7 },
        ToStub::QuitReply { allow: false },
        ToStub::Alert {
            message: "Can't open".into(),
            detail: "No route".into(),
        },
        ToStub::Quit,
        ToStub::TextContext {
            window: 7,
            editing: true,
            caret: Some([12, -3]),
        },
        ToStub::TextContext {
            window: 7,
            editing: false,
            caret: None,
        },
        ToStub::Cursor {
            window: 7,
            cursor: cursor::I_BEAM,
        },
    ]
}

fn host_messages() -> Vec<ToHost> {
    vec![
        ToHost::Hello {
            version: VERSION,
            reason: LaunchReason::User,
            bundle_id: "app.newport.remote.a.b".into(),
        },
        ToHost::Pointer {
            window: 1,
            generation: 2,
            x: 10.5,
            y: 20.25,
            modifiers: modifier::SHIFT,
        },
        ToHost::Button {
            window: 1,
            generation: 2,
            x: 1.,
            y: 2.,
            button: 3,
            down: true,
            modifiers: modifier::COMMAND,
        },
        ToHost::Scroll {
            window: 1,
            generation: 2,
            x: 1.,
            y: 2.,
            dx: -0.5,
            dy: 12.,
            precise: true,
            phase: 4,
            momentum: 0,
            modifiers: modifier::CONTROL,
        },
        ToHost::KeyUp {
            window: 1,
            generation: 2,
            code: 0x26,
            modifiers: 0,
        },
        ToHost::Modifiers {
            window: 1,
            generation: 2,
            code: 0x38,
            flags: 1 << 17 | 0x02,
        },
        ToHost::Exit {
            window: 1,
            generation: 2,
        },
        ToHost::Key {
            window: 1,
            generation: 2,
            code: 0,
            characters: "å".into(),
            unmodified: "a".into(),
            modifiers: modifier::OPTION,
            repeat: false,
        },
        ToHost::Text {
            window: 1,
            generation: 2,
            text: "日本".into(),
        },
        ToHost::Preedit {
            window: 1,
            generation: 2,
            text: "五笔".into(),
            cursor: 2,
        },
        ToHost::Focus {
            window: 1,
            key: false,
        },
        ToHost::Resized {
            window: 1,
            logical: [800, 600],
        },
        ToHost::CloseAsked { window: 1 },
        ToHost::Minimized {
            window: 1,
            minimized: true,
        },
        ToHost::Fullscreen {
            window: 1,
            enabled: true,
        },
        ToHost::Presented {
            window: 1,
            generation: 2,
            index: 0,
        },
        ToHost::AppActive { active: true },
        ToHost::QuitAsked,
        ToHost::Reopen,
        ToHost::Display {
            window: 1,
            scale: 2.,
            work_area: [0., 25., 1512., 920.],
            content: [100., 800.],
        },
        ToHost::Visible {
            window: 1,
            visible: false,
        },
    ]
}

#[test]
fn all_current_messages_round_trip_without_native_transport_dependencies() {
    for message in stub_messages() {
        let frame = encode_stub(&message).unwrap();
        assert!(frame.len() <= MAX_MESSAGE_BYTES);
        assert_eq!(decode_stub(&frame).unwrap(), message);
    }
    for message in host_messages() {
        let frame = encode_host(&message).unwrap();
        assert!(frame.len() <= MAX_MESSAGE_BYTES);
        assert_eq!(decode_host(&frame).unwrap(), message);
    }
}
#[test]
fn incomplete_or_coalesced_datagrams_are_rejected() {
    for message in stub_messages() {
        let frame = encode_stub(&message).unwrap();
        for end in 0..frame.len() {
            assert!(decode_stub(&frame[..end]).is_err());
        }
        let mut extra = frame.clone();
        extra.extend_from_slice(&frame);
        assert!(decode_stub(&extra).is_err());
    }
    for message in host_messages() {
        let frame = encode_host(&message).unwrap();
        for end in 0..frame.len() {
            assert!(decode_host(&frame[..end]).is_err());
        }
    }
}
#[test]
fn invalid_dimensions_indices_text_versions_and_coordinates_fail_before_encoding() {
    let mut state = window_state();
    state.logical[0] = MAX_WINDOW_EXTENT + 1;
    assert!(encode_stub(&ToStub::Window { state, surfaces: 1 }).is_err());
    assert!(
        encode_stub(&ToStub::Window {
            state: window_state(),
            surfaces: MAX_SURFACES_PER_WINDOW + 1
        })
        .is_err()
    );
    assert!(
        encode_stub(&ToStub::Frame {
            window: 1,
            generation: 1,
            index: MAX_SURFACES_PER_WINDOW
        })
        .is_err()
    );
    assert!(
        encode_stub(&ToStub::Title {
            window: 1,
            title: "é".repeat(MAX_TITLE_BYTES)
        })
        .is_err()
    );
    assert!(
        encode_stub(&ToStub::Cursor {
            window: 1,
            cursor: cursor::HIDDEN + 1
        })
        .is_err()
    );
    assert!(
        encode_host(&ToHost::Pointer {
            window: 1,
            generation: 1,
            x: f64::NAN,
            y: 0.,
            modifiers: 0
        })
        .is_err()
    );
    assert!(
        encode_host(&ToHost::Hello {
            version: VERSION + 1,
            reason: LaunchReason::User,
            bundle_id: "fixture".into()
        })
        .is_err()
    );
}
fn raw<T: prost::Message + protobuf::Layout>(value: &T) -> Vec<u8> {
    let mut frame = Vec::new();
    protobuf::write_message(&mut frame, value, &LIMITS).unwrap();
    frame
}
#[test]
fn presence_false_and_version_are_distinct() {
    let mut message = pb::HostFrame {
        version: Some(VERSION),
        payload: Some(pb::host_frame::Payload::AppActive(Box::new(
            pb::HostAppActive { active: None },
        ))),
    };
    assert!(decode_host(&raw(&message)).is_err());
    message.payload = Some(pb::host_frame::Payload::AppActive(Box::new(
        pb::HostAppActive {
            active: Some(false),
        },
    )));
    assert_eq!(
        decode_host(&raw(&message)).unwrap(),
        ToHost::AppActive { active: false }
    );
    message.version = Some(VERSION + 1);
    assert!(decode_host(&raw(&message)).is_err());
}
#[test]
fn duplicate_known_fields_and_mixed_operations_are_rejected_but_unknown_metadata_is_skipped() {
    let frame = encode_stub(&ToStub::Quit).unwrap();
    let original = body(&frame).unwrap();
    for suffix in [&[8, 1][..], &[0x12, 0][..]] {
        let mut duplicate = original.to_vec();
        duplicate.extend_from_slice(suffix);
        let mut frame = Vec::new();
        protobuf::write_frame(&mut frame, &duplicate).unwrap();
        assert!(decode_stub(&frame).is_err());
    }
    let mut unknown = original.to_vec();
    unknown.extend_from_slice(&[0xa0, 0x06, 1]);
    let mut frame = Vec::new();
    protobuf::write_frame(&mut frame, &unknown).unwrap();
    assert_eq!(decode_stub(&frame).unwrap(), ToStub::Quit);
}
#[test]
fn admission_preserves_explicit_refusal_and_never_admits_windows_before_welcome() {
    let mut admission = Admission::default();
    assert!(admission.accept(&ToStub::Destroy { window: 1 }).is_err());
    assert_eq!(admission.phase(), Phase::AwaitingWelcome);
    admission
        .accept(&ToStub::Alert {
            message: "Unavailable".into(),
            detail: String::new(),
        })
        .unwrap();
    assert_eq!(admission.phase(), Phase::AwaitingWelcome);
    admission
        .accept(&ToStub::Welcome {
            name: "Fixture".into(),
            server: "Lab".into(),
        })
        .unwrap();
    assert_eq!(admission.phase(), Phase::Ready);
    assert!(
        admission
            .accept(&ToStub::Welcome {
                name: "Fixture".into(),
                server: "Lab".into()
            })
            .is_err()
    );
    let mut refused = Admission::default();
    refused.accept(&ToStub::Quit).unwrap();
    assert!(
        refused
            .accept(&ToStub::Welcome {
                name: "Fixture".into(),
                server: "Lab".into()
            })
            .is_err()
    );
}
