use super::*;
use crate::{remote_apps::apps, worker};
use prost::Message;

fn app() -> Application {
    Application {
        id: "fixture.desktop".into(),
        name: "Éditeur".into(),
        description: "Line\n\t\u{1}".into(),
        keywords: vec!["text".into()],
        icon: Some("aWNvbg==".into()),
        actions: vec![DesktopAction {
            id: "new-window".into(),
            name: "New window".into(),
        }],
    }
}
fn field(number: u32, body: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    prost::encoding::encode_varint(u64::from(number) << 3 | 2, &mut output);
    prost::encoding::encode_varint(body.len() as u64, &mut output);
    output.extend_from_slice(body);
    output
}
fn catalogue(apps: &[u8]) -> Bytes {
    field(1, apps).into()
}

#[test]
fn catalogue_and_failures_round_trip_with_presence_and_limits() {
    let empty = Reply::Catalogue(Catalogue { apps: vec![] });
    assert_eq!(encode(&empty).unwrap(), [2, 10, 0]);
    for icon in [None, Some(String::new()), app().icon] {
        let mut entry = app();
        entry.icon = icon;
        let reply = Reply::Catalogue(Catalogue { apps: vec![entry] });
        assert_eq!(decode_complete(&encode(&reply).unwrap()).unwrap(), reply);
    }
    for retry in [
        common::Retry::Unknown,
        common::Retry::Yes,
        common::Retry::No,
    ] {
        let reply = Reply::Failed(common::Error {
            code: "discovery_failed".into(),
            message: "A directory could not be read".into(),
            retry,
        });
        assert_eq!(decode_complete(&encode(&reply).unwrap()).unwrap(), reply);
    }
    let invalid = Reply::Failed(common::Error::default());
    assert!(encode(&invalid).is_err());
}

#[test]
fn codec_free_budget_admits_boundary_metadata_and_icons() {
    let mut entry = app();
    entry.id = format!("{}.desktop", "a".repeat(apps::MAX_ID - 8));
    entry.name = "é".repeat(apps::MAX_NAME / 2);
    entry.description = "\u{1}".repeat(apps::MAX_DESCRIPTION);
    entry.keywords = vec!["k".repeat(apps::MAX_KEYWORD); apps::MAX_KEYWORDS];
    entry.actions = (0..apps::MAX_ACTIONS)
        .map(|i| DesktopAction {
            id: format!("{i:03}{}", "a".repeat(apps::MAX_ACTION_ID - 3)),
            name: "a".repeat(apps::MAX_NAME),
        })
        .collect();
    entry.icon = None;
    let metadata = apps::CATALOGUE_BUDGET + entry.byte_budget().unwrap();
    entry.icon = Some("A".repeat(apps::MAX_CATALOGUE_BYTES - metadata));
    let reply = Reply::Catalogue(Catalogue {
        apps: vec![entry.clone()],
    });
    let encoded = encode(&reply).unwrap();
    assert!(encoded.len() <= MAX_FRAMED_BYTES);
    assert_eq!(decode_complete(&encoded).unwrap(), reply);
    entry.icon.as_mut().unwrap().push('A');
    assert!(encode(&Reply::Catalogue(Catalogue { apps: vec![entry] })).is_err());
    let entries = (0..apps::MAX_APPLICATIONS)
        .map(|i| Application {
            id: format!("{i}.desktop"),
            ..app()
        })
        .collect();
    let reply = Reply::Catalogue(Catalogue { apps: entries });
    assert_eq!(decode_complete(&encode(&reply).unwrap()).unwrap(), reply);
}

#[test]
fn counts_and_duplicate_fields_fail_before_record_allocation() {
    let oversized = catalogue(&field(1, &[]).repeat(apps::MAX_APPLICATIONS + 1));
    assert!(matches!(
        decode(oversized),
        Err(Error::Invalid(Invalid("Too many catalogue values")))
    ));
    for (number, count) in [
        (4, apps::MAX_KEYWORDS + 1),
        (6, apps::MAX_ACTIONS + 1),
        (1, 2),
        (5, 2),
    ] {
        let body = catalogue(&field(1, &field(number, &[]).repeat(count)));
        assert!(matches!(
            decode(body),
            Err(Error::Invalid(Invalid("Too many catalogue values")))
        ));
    }
    let action = field(1, &[]).repeat(2);
    assert!(decode(catalogue(&field(1, &field(6, &action)))).is_err());
    assert!(
        decode(catalogue(&field(
            1,
            &field(2, &vec![b'a'; apps::MAX_NAME + 1])
        )))
        .is_err()
    );
    assert!(decode(Bytes::from([field(1, &[]), field(2, &[])].concat())).is_err());
    let mut repeated = pb::ServerFrame {
        message: Some(pb::server_frame::Message::Catalogue(pb::Catalogue {
            apps: vec![],
        })),
    }
    .encode_to_vec();
    repeated.extend_from_slice(&[0x98, 0x06, 0].repeat(MAX_FIELDS));
    assert!(matches!(
        decode(repeated.into()),
        Err(Error::Invalid(Invalid("Too many catalogue fields")))
    ));
}

#[test]
fn identity_error_enums_controls_and_unknown_metadata_are_checked() {
    let duplicate = Reply::Catalogue(Catalogue {
        apps: vec![app(), app()],
    });
    assert!(encode(&duplicate).is_err());
    let error = pb::ServerFrame {
        message: Some(pb::server_frame::Message::Error(protobuf::Error {
            code: "failed".into(),
            message: "Read failed".into(),
            retry: 3,
        })),
    };
    assert!(decode(error.encode_to_vec().into()).is_err());
    let control =
        worker::wire::encode_server(&worker::ServerControl::Exit(worker::Exit::Status(0))).unwrap();
    let mut control = protobuf::read_frame(&mut control.as_slice(), SERVER_LIMITS.message)
        .unwrap()
        .unwrap()
        .to_vec();
    assert!(matches!(
        decode(control.clone().into()),
        Err(Error::Control(_))
    ));
    control.extend_from_slice(&field(1, &[]));
    assert!(matches!(
        decode(control.into()),
        Err(Error::Control(worker::wire::Error::MixedControl))
    ));
    let mut known = field(1, &[]);
    known.extend_from_slice(&[0x98, 0x06, 0]);
    assert_eq!(
        decode(known.into()).unwrap(),
        Reply::Catalogue(Catalogue { apps: vec![] })
    );
    assert!(decode(Bytes::from_static(&[0x98, 0x06, 0])).is_err());
}

#[test]
fn complete_output_rejects_missing_partial_oversized_and_extra_frames() {
    assert!(matches!(decode_complete(&[]), Err(Error::Ended)));
    for value in [&[0x80][..], &[5, 10, 0][..], &[1, 0xff][..]] {
        assert!(decode_complete(value).is_err());
    }
    let encoded = encode(&Reply::Catalogue(Catalogue { apps: vec![] })).unwrap();
    for suffix in [vec![b'\n'], encoded.clone()] {
        assert!(matches!(
            decode_complete(&[encoded.clone(), suffix].concat()),
            Err(Error::Trailing)
        ));
    }
    let mut prefix = Vec::new();
    prost::encoding::encode_varint(SERVER_LIMITS.message as u64 + 1, &mut prefix);
    let mut input = std::io::Cursor::new(prefix.clone());
    assert!(matches!(
        read(&mut input),
        Err(Error::Frame(FrameError::TooLarge { .. }))
    ));
    assert_eq!(input.position() as usize, prefix.len());
}
