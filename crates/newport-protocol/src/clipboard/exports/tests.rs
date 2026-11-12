use super::*;

pub(super) fn fixture() -> ExportOffer {
    ExportOffer {
        offer: super::super::Offer {
            runtime: "11111111-1111-4111-8111-111111111111".into(),
            revision: 1,
            origin: Origin::Native {
                worker: "native".into(),
                revision: 1,
            },
            formats: (0..MAX_FORMATS)
                .map(|n| format!("application/{n:03}{}", "x".repeat(241)))
                .collect(),
        },
        route_revision: 1,
    }
}
pub(super) fn copy() -> CopyLease {
    CopyLease {
        id: "22222222-2222-4222-8222-222222222222".into(),
        offer: fixture(),
    }
}

#[test]
fn copy_controls_validate_identity_authority_and_selected_format() {
    let copy = copy();
    for control in [
        Control::Enable { version: VERSION },
        Control::Disable,
        Control::Cancel { id: 1 },
        Control::Read {
            id: 1,
            offer: copy.offer.clone(),
            format: copy.offer.offer.formats[0].clone(),
        },
        Control::Commit {
            id: 2,
            offer: copy.offer.clone(),
        },
        Control::ReadCopy {
            id: 3,
            copy: copy.clone(),
            format: copy.offer.offer.formats[0].clone(),
        },
        Control::Release { copy: copy.clone() },
    ] {
        control.validate().unwrap();
    }
    for control in [
        Control::Enable {
            version: VERSION + 1,
        },
        Control::Cancel { id: 0 },
        Control::Commit {
            id: 0,
            offer: fixture(),
        },
        Control::Read {
            id: 1,
            offer: fixture(),
            format: "not/offered".into(),
        },
        Control::ReadCopy {
            id: 1,
            copy: copy.clone(),
            format: "not/offered".into(),
        },
        Control::Release {
            copy: CopyLease {
                id: "invalid".into(),
                ..copy
            },
        },
    ] {
        assert!(control.validate().is_err());
    }
}

#[test]
fn export_offers_require_native_identity_and_unique_bounded_formats() {
    let changes: [fn(&mut ExportOffer); 10] = [
        |v| v.offer.origin = Origin::Host,
        |v| v.offer.origin = Origin::Empty,
        |v| {
            v.offer.origin = Origin::Native {
                worker: "../worker".into(),
                revision: 1,
            }
        },
        |v| {
            v.offer.origin = Origin::Native {
                worker: "native".into(),
                revision: 0,
            }
        },
        |v| v.offer.runtime = "invalid".into(),
        |v| v.offer.revision = 0,
        |v| v.route_revision = 0,
        |v| v.offer.formats.clear(),
        |v| v.offer.formats.push("text/plain".into()),
        |v| v.offer.formats[1] = v.offer.formats[0].clone(),
    ];
    for invalid_format in ["", "../text", "text/", "text\0/plain"] {
        let mut offer = fixture();
        offer.offer.formats[0] = invalid_format.into();
        assert!(Event::Offer { offer: Some(offer) }.validate().is_err());
    }
    for change in changes {
        let mut offer = fixture();
        change(&mut offer);
        assert!(Event::Offer { offer: Some(offer) }.validate().is_err());
    }
}

#[test]
fn failure_absence_is_distinct_from_empty_completed_data() {
    for chunk in [
        None,
        Some(Chunk {
            bytes: vec![],
            eof: true,
        }),
    ] {
        Event::Data {
            id: 1,
            offset: 0,
            chunk,
        }
        .validate()
        .unwrap();
    }
    Event::Offer { offer: None }.validate().unwrap();
    Event::Committed { id: 1, copy: None }.validate().unwrap();
    for (offset, bytes, eof) in [
        (0, vec![], false),
        (0, vec![0; CHUNK + 1], true),
        (MAX_BYTES, vec![0], true),
        (usize::MAX, vec![0], true),
    ] {
        assert!(
            Event::Data {
                id: 1,
                offset,
                chunk: Some(Chunk { bytes, eof })
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        Event::Data {
            id: 0,
            offset: 0,
            chunk: None
        }
        .validate()
        .is_err()
    );
    assert!(Event::Committed { id: 0, copy: None }.validate().is_err());
    assert!(
        Event::CopyRevoked {
            id: "invalid".into()
        }
        .validate()
        .is_err()
    );
}
