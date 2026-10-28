use super::tests::{client, offer, packet};
use super::*;
use crate::clipboard::helper::wire;
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct Candidate {
    focused: AtomicBool,
    published: AtomicBool,
    revoked: AtomicBool,
    confirms: AtomicUsize,
    reject: AtomicBool,
}
impl Source for Candidate {
    fn is_current(&self) -> bool {
        !self.revoked.load(Ordering::Acquire)
            && (self.focused.load(Ordering::Acquire) || self.published.load(Ordering::Acquire))
    }
    fn request(&self, _: &str, _: crate::clipboard::provider::Reply) {}
    fn publication_confirmed(&self) -> bool {
        self.confirms.fetch_add(1, Ordering::AcqRel);
        if !self.is_current() || self.reject.load(Ordering::Acquire) {
            return false;
        }
        self.published.store(true, Ordering::Release);
        true
    }
    fn publication_revoked(&self) {
        self.revoked.store(true, Ordering::Release);
    }
}
fn publish(session: &mut Session<'_>) -> (Arc<Candidate>, oneshot::Receiver<Option<Publication>>) {
    let expected = offer(session);
    let candidate = Arc::new(Candidate::default());
    candidate.focused.store(true, Ordering::Release);
    let (reply, receive) = oneshot::channel();
    session
        .action(Action::Publish {
            expected,
            formats: vec!["text/plain".into()],
            source: candidate.clone(),
            reply,
        })
        .unwrap();
    assert!(!candidate.published.load(Ordering::Acquire));
    (candidate, receive)
}
fn acknowledged(session: &mut Session<'_>) {
    session
        .event(packet(
            Event::Published {
                generation: 1,
                revision: 78,
            },
            &[],
        ))
        .unwrap();
}
fn native_copy(session: &mut Session<'_>) {
    session
        .event(packet(
            Event::Snapshot {
                revision: 79,
                formats: vec!["image/png".into()],
                origin: None,
            },
            &[],
        ))
        .unwrap();
}

#[test]
fn native_owner_confirms_before_delivering_and_focus_loss_afterward_preserves_copy() {
    let (client, _peer) = client();
    let mut continuity = Continuity::default();
    let mut session = Session::new(
        client,
        &mut continuity,
        Arc::default(),
        Arc::new(AtomicBool::new(true)),
    );
    let (candidate, mut receive) = publish(&mut session);
    acknowledged(&mut session);
    candidate.focused.store(false, Ordering::Release);
    // The awaiting caller has not received/activated anything yet.
    let publication = receive.try_recv().unwrap().unwrap();
    assert_eq!(candidate.confirms.load(Ordering::Acquire), 1);
    assert!(publication.is_current());
    session.maintain().unwrap();
    assert!(session.import.is_some());
    drop(publication);
    assert!(candidate.revoked.load(Ordering::Acquire));
    assert!(!candidate.is_current());
}

#[test]
fn stale_rejected_and_abandoned_publications_cannot_be_accepted_by_late_ack() {
    for reject in 0..3 {
        let (client, _peer) = client();
        let mut continuity = Continuity::default();
        let mut session = Session::new(
            client,
            &mut continuity,
            Arc::default(),
            Arc::new(AtomicBool::new(true)),
        );
        let (candidate, receive) = publish(&mut session);
        let mut receive = Some(receive);
        match reject {
            0 => candidate.focused.store(false, Ordering::Release),
            1 => candidate.reject.store(true, Ordering::Release),
            _ => {
                receive.take();
            }
        }
        acknowledged(&mut session);
        assert!(!candidate.published.load(Ordering::Acquire));
        assert!(candidate.revoked.load(Ordering::Acquire));
        if let Some(mut receive) = receive {
            assert!(receive.try_recv().is_err());
        }
        session.maintain().unwrap();
        assert!(session.import.is_none());
        assert_eq!(
            candidate.confirms.load(Ordering::Acquire),
            usize::from(reject == 1)
        );
    }
}

#[test]
fn newer_local_snapshot_retires_pending_or_published_source() {
    for before_ack in [true, false] {
        let (client, _peer) = client();
        let mut continuity = Continuity::default();
        let mut session = Session::new(
            client,
            &mut continuity,
            Arc::default(),
            Arc::new(AtomicBool::new(true)),
        );
        let (candidate, mut receive) = publish(&mut session);
        let held = if before_ack {
            None
        } else {
            acknowledged(&mut session);
            Some(receive.try_recv().unwrap().unwrap())
        };
        native_copy(&mut session);
        assert!(session.import.is_none());
        assert!(candidate.revoked.load(Ordering::Acquire));
        if before_ack {
            acknowledged(&mut session);
            assert!(receive.try_recv().is_err());
            assert_eq!(candidate.confirms.load(Ordering::Acquire), 0);
        } else {
            assert!(!held.as_ref().unwrap().is_current());
        }
        let local = session.snapshot.as_ref().unwrap();
        assert_eq!(local.lease.native, 79);
        assert_eq!(local.origin, None);
        assert_eq!(local.formats, ["image/png"]);
        assert!(local.is_current());
    }
}

#[test]
fn native_rejection_and_owner_teardown_revoke_even_an_externally_held_source() {
    for rejection in [true, false] {
        let (client, _peer) = client();
        let mut continuity = Continuity::default();
        let mut session = Session::new(
            client,
            &mut continuity,
            Arc::default(),
            Arc::new(AtomicBool::new(true)),
        );
        let (candidate, _receive) = publish(&mut session);
        if rejection {
            session
                .event(packet(Event::Rejected { generation: 1 }, &[]))
                .unwrap();
        }
        drop(session);
        assert!(candidate.revoked.load(Ordering::Acquire));
        assert_eq!(candidate.confirms.load(Ordering::Acquire), 0);
    }
}

#[test]
fn adding_native_formats_at_the_same_change_count_invalidates_a_pending_publication() {
    let (client, _peer) = client();
    let mut continuity = Continuity::default();
    let mut session = Session::new(
        client,
        &mut continuity,
        Arc::default(),
        Arc::new(AtomicBool::new(true)),
    );
    let (candidate, mut receive) = publish(&mut session);
    session
        .event(packet(
            Event::Snapshot {
                revision: 77,
                formats: vec!["text/plain".into(), "image/png".into()],
                origin: None,
            },
            &[],
        ))
        .unwrap();
    assert!(candidate.revoked.load(Ordering::Acquire));
    acknowledged(&mut session);
    assert!(receive.try_recv().is_err());
    assert_eq!(candidate.confirms.load(Ordering::Acquire), 0);
    assert_eq!(
        session.snapshot.as_ref().unwrap().formats,
        ["text/plain", "image/png"]
    );
}

fn attach(session: &mut Session<'_>, generation: u64) -> oneshot::Receiver<Option<isize>> {
    let (reply, receive) = oneshot::channel();
    session
        .action(Action::AttachFiles {
            generation,
            folder: "folder".into(),
            reply,
        })
        .unwrap();
    receive
}

#[test]
fn attached_files_update_the_import_so_the_next_snapshot_keeps_it() {
    let (client, mut peer) = client();
    let mut continuity = Continuity::default();
    let mut session = Session::new(
        client,
        &mut continuity,
        Arc::default(),
        Arc::new(AtomicBool::new(true)),
    );
    let (candidate, mut receive) = publish(&mut session);
    acknowledged(&mut session);
    let _publication = receive.try_recv().unwrap().unwrap();
    let mut wait = attach(&mut session, 1);
    // The publication precedes the attachment on the wire.
    let first = wire::receive::<Command>(&mut peer, Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert!(matches!(first.header, Command::Publish { .. }));
    let second = wire::receive::<Command>(&mut peer, Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert!(matches!(
        second.header,
        Command::AttachFiles { generation: 1, expected_revision: 78, ref folder } if folder == "folder"
    ));
    session
        .event(packet(
            Event::Attached {
                generation: 1,
                revision: Some(80),
            },
            &[],
        ))
        .unwrap();
    assert_eq!(wait.try_recv().unwrap(), Some(80));
    assert_eq!(session.import.as_ref().unwrap().native, Some(80));
    session
        .event(packet(
            Event::Snapshot {
                revision: 80,
                formats: vec!["public.file-url".into()],
                origin: Some(1),
            },
            &[],
        ))
        .unwrap();
    assert!(session.import.is_some());
    assert!(!candidate.revoked.load(Ordering::Acquire));
    assert_eq!(session.snapshot.as_ref().unwrap().origin, Some(1));
}

#[test]
fn attach_is_only_sent_for_the_current_confirmed_import() {
    let (client, _peer) = client();
    let mut continuity = Continuity::default();
    let mut session = Session::new(
        client,
        &mut continuity,
        Arc::default(),
        Arc::new(AtomicBool::new(true)),
    );
    // Nothing imported yet.
    assert!(attach(&mut session, 1).try_recv().is_err());
    let (candidate, mut receive) = publish(&mut session);
    // Still unconfirmed: no native revision to expect.
    assert!(attach(&mut session, 1).try_recv().is_err());
    acknowledged(&mut session);
    let _publication = receive.try_recv().unwrap().unwrap();
    // Wrong generation.
    assert!(attach(&mut session, 2).try_recv().is_err());
    assert!(session.attach.is_none());
    // Refusal from the helper is reported and keeps the import.
    let mut refused = attach(&mut session, 1);
    assert!(session.attach.is_some());
    // A second attach while one is pending is refused.
    assert!(attach(&mut session, 1).try_recv().is_err());
    session
        .event(packet(
            Event::Attached {
                generation: 1,
                revision: None,
            },
            &[],
        ))
        .unwrap();
    assert_eq!(refused.try_recv().unwrap(), None);
    assert_eq!(session.import.as_ref().unwrap().native, Some(78));
    // An unsolicited or unknown attachment is a protocol error.
    assert!(
        session
            .event(packet(
                Event::Attached {
                    generation: 1,
                    revision: Some(9)
                },
                &[]
            ))
            .is_err()
    );
    // Revoked sources can no longer attach.
    candidate.revoked.store(true, Ordering::Release);
    assert!(attach(&mut session, 1).try_recv().is_err());
}
