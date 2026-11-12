use super::super::tests::{copy, fixture};
use super::*;

#[test]
fn every_control_and_event_round_trips_with_maximum_metadata_and_chunks() {
    let copy = copy();
    for control in [
        Control::Enable { version: VERSION },
        Control::Disable,
        Control::Read {
            id: u64::MAX,
            offer: fixture(),
            format: fixture().offer.formats[0].clone(),
        },
        Control::Cancel { id: u64::MAX },
        Control::Commit {
            id: u64::MAX,
            offer: fixture(),
        },
        Control::Release { copy: copy.clone() },
        Control::ReadCopy {
            id: u64::MAX,
            copy: copy.clone(),
            format: fixture().offer.formats[0].clone(),
        },
    ] {
        let bytes = encode_control(&control).unwrap();
        assert!(bytes.len() <= MAX_FRAME);
        assert_eq!(decode_control(&bytes).unwrap(), control);
    }
    for event in [
        Event::Offer { offer: None },
        Event::Offer {
            offer: Some(fixture()),
        },
        Event::Data {
            id: u64::MAX,
            offset: MAX_BYTES - CHUNK,
            chunk: Some(Chunk {
                bytes: vec![255; CHUNK],
                eof: true,
            }),
        },
        Event::Data {
            id: 1,
            offset: 0,
            chunk: None,
        },
        Event::Data {
            id: 1,
            offset: 0,
            chunk: Some(Chunk {
                bytes: vec![],
                eof: true,
            }),
        },
        Event::Committed { id: 1, copy: None },
        Event::Committed {
            id: 1,
            copy: Some(copy.clone()),
        },
        Event::CopyRevoked { id: copy.id },
    ] {
        let bytes = encode_event(&event).unwrap();
        assert!(bytes.len() <= MAX_FRAME);
        assert_eq!(decode_event(&bytes).unwrap(), event);
    }
}

#[test]
fn presence_and_failure_have_distinct_stable_bytes() {
    assert_eq!(encode_control(&Control::Disable).unwrap(), [18, 0]);
    assert_eq!(
        encode_event(&Event::Offer { offer: None }).unwrap(),
        [10, 0]
    );
    assert_eq!(
        encode_event(&Event::Data {
            id: 1,
            offset: 0,
            chunk: None
        })
        .unwrap(),
        [18, 2, 8, 1]
    );
    assert_eq!(
        encode_event(&Event::Data {
            id: 1,
            offset: 0,
            chunk: Some(Chunk {
                bytes: vec![],
                eof: true
            })
        })
        .unwrap(),
        [18, 6, 8, 1, 26, 2, 16, 1]
    );
}

#[test]
fn repeated_oneofs_singular_fields_and_narrowing_values_are_rejected() {
    for body in [
        vec![18, 0, 18, 0],                     // Repeated disable.
        vec![18, 0, 34, 2, 8, 1],               // Disable and cancel.
        vec![34, 4, 8, 1, 8, 2],                // Repeated cancel ID.
        vec![10, 6, 8, 129, 128, 128, 128, 16], // Version narrows to one.
        vec![34, 2, 10, 0],                     // ID with the wrong wire type.
        vec![18, 0, 0],
    ] {
        assert!(decode_control(&body).is_err(), "{body:?}");
    }
    for body in [
        vec![10, 0, 10, 0],
        vec![18, 6, 8, 1, 26, 2, 16, 2], // Noncanonical boolean.
        vec![18, 10, 8, 1, 26, 2, 16, 1, 26, 2, 16, 1], // Repeated chunk.
    ] {
        assert!(decode_event(&body).is_err(), "{body:?}");
    }
}

#[test]
fn counts_and_lengths_are_checked_before_generated_decoding() {
    let mut value = offer_to_wire(&fixture());
    value.formats = vec!["a/b".into(); MAX_FORMATS + 1];
    let event = |offer| pb::ExportEvent {
        event: Some(pb::export_event::Event::Offered(pb::Offered {
            offer: Some(offer),
        })),
    };
    assert!(scan::event(&event(value).encode_to_vec()).is_err());
    let mut value = offer_to_wire(&fixture());
    value.formats = vec!["x".repeat(MAX_FORMAT_BYTES + 1)];
    assert!(scan::event(&event(value).encode_to_vec()).is_err());
    let value = pb::ExportEvent {
        event: Some(pb::export_event::Event::Data(pb::Data {
            id: 1,
            offset: 0,
            chunk: Some(pb::Chunk {
                data: vec![0; CHUNK + 1].into(),
                eof: true,
            }),
        })),
    };
    assert!(scan::event(&value.encode_to_vec()).is_err());
    assert!(decode_event(&vec![0; MAX_FRAME + 1]).is_err());
}

#[test]
fn unknown_metadata_is_skipped_but_missing_operations_and_truncation_fail() {
    let mut bytes = encode_control(&Control::Disable).unwrap();
    bytes.extend([0xa0, 0x06, 1]);
    assert_eq!(decode_control(&bytes).unwrap(), Control::Disable);
    assert!(decode_control(&[0xa0, 0x06, 1]).is_err());
    let bytes = encode_event(&Event::Committed {
        id: 1,
        copy: Some(copy()),
    })
    .unwrap();
    for end in [0, 1, 2, bytes.len() - 1] {
        assert!(decode_event(&bytes[..end]).is_err());
    }
    let mut repeated = vec![18, 0];
    for _ in 0..MAX_FIELDS {
        repeated.extend([0xa0, 0x06, 1]);
    }
    assert!(decode_control(&repeated).is_err());
}
