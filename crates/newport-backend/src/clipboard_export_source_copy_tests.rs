use super::*;
use crate::agent_runtime::protocol::clipboard::CopyLease;

async fn setup_copy() -> (Receiver, ExportOffer, Grant, Demands, CommittedCopy) {
    let (mut receiver, offer, grant, demands) = setup();
    receiver.disable();
    receiver
        .enable_version(&offer.offer.runtime, wire::COPY_VERSION)
        .unwrap();
    advertise(&mut receiver, Some(offer.clone()));
    let (pending, _) = receiver.commit(&receiver.lease().unwrap()).unwrap();
    receiver
        .accept(
            &wire::Event::Committed {
                id: pending.id(),
                copy: Some(CopyLease {
                    id: Uuid::new_v4().to_string(),
                    offer: offer.clone(),
                }),
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
    (
        receiver,
        offer,
        grant,
        demands,
        pending.receive().await.unwrap(),
    )
}

#[tokio::test]
async fn copy_binding_requires_exact_focus_identity_and_releases_rejected_authority() {
    for wrong in 0..3 {
        let (receiver, _, grant, demands, copy) = setup_copy().await;
        let runtime = Uuid::parse_str(&grant.0.runtime).unwrap();
        let worker = Uuid::parse_str(&grant.0.worker).unwrap();
        let bad = match wrong {
            0 => Grant::new(Uuid::new_v4(), worker, 1),
            1 => Grant::new(runtime, Uuid::new_v4(), 1),
            _ => Grant::new(runtime, worker, 2),
        }
        .unwrap();
        assert!(demands.source_copy(copy, &bad).is_none());
        assert!(receiver.copy_release().is_some());
    }
    let (_receiver, _, grant, demands, copy) = setup_copy().await;
    let source = demands.source_copy(copy, &grant).unwrap();
    assert!(source.is_current());
    assert!(
        !source.readable(),
        "pending publication authorized payload reads"
    );
    let (other, _, other_grant, _, other_copy) = setup_copy().await;
    assert!(demands.source_copy(other_copy, &other_grant).is_none());
    assert!(other.copy_release().is_some());
    assert!(source.is_current());
}

#[tokio::test]
async fn focus_lost_before_confirmation_cannot_be_accepted_later() {
    let (receiver, _, grant, demands, copy) = setup_copy().await;
    let source = demands.source_copy(copy, &grant).unwrap();
    assert!(source.begin_publication());
    grant.revoke();
    assert!(!source.is_current());
    assert!(!source.publication_confirmed());
    assert!(!source.readable());
    assert!(!source.copy().unwrap().is_current());
    assert!(receiver.copy_release().is_some());
}

#[tokio::test]
async fn confirmed_native_copy_survives_focus_but_keeps_all_other_fences() {
    for revoke in 0..4 {
        let (mut receiver, _, grant, demands, copy) = setup_copy().await;
        let source = demands.source_copy(copy, &grant).unwrap();
        assert!(source.begin_publication());
        assert!(source.publication_confirmed());
        assert!(
            !source.begin_publication(),
            "accepted source authorized another write"
        );
        drop(grant);
        advertise(&mut receiver, None);
        assert!(source.readable());
        let (read, _) = receiver
            .read_copy(source.copy().unwrap(), "text/plain")
            .unwrap();
        send(&mut receiver, read.id(), b"accepted", true);
        let data = Data {
            source: source.as_ref().clone(),
            data: read.receive().await.unwrap(),
            _slot: demands.slots.clone().try_acquire_owned().unwrap(),
        };
        assert_eq!(data.bytes(), Some(b"accepted".as_slice()));
        match revoke {
            0 => source.publication_revoked(),
            1 => source.copy().unwrap().revoke(),
            2 => receiver.disconnect(),
            _ => drop(demands),
        }
        assert!(!source.is_current());
        assert!(data.bytes().is_none());
        assert!(
            receiver
                .read_copy(source.copy().unwrap(), "text/plain")
                .is_err()
        );
    }
}

#[tokio::test]
async fn racing_focus_loss_and_publication_has_one_consistent_outcome() {
    for _ in 0..32 {
        let (_receiver, _, grant, demands, copy) = setup_copy().await;
        let source = demands.source_copy(copy, &grant).unwrap();
        assert!(source.begin_publication());
        let barrier = std::sync::Barrier::new(2);
        let accepted = std::thread::scope(|threads| {
            let confirmation = threads.spawn(|| {
                barrier.wait();
                source.publication_confirmed()
            });
            barrier.wait();
            grant.revoke();
            confirmation.join().unwrap()
        });
        assert_eq!(source.is_current(), accepted);
        assert_eq!(source.readable(), accepted);
        assert_eq!(source.copy().unwrap().is_current(), accepted);
        source.publication_revoked();
        assert!(
            !source.publication_confirmed(),
            "revoked publication revived"
        );
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn unavailable_or_unpolled_native_publication_releases_the_runtime_copy() {
    use crate::desktop_integration::{ClipboardAccess, NativeClipboard};
    use newport_platform::clipboard::helper::{Board, owner::Owner};
    for available in [false, true] {
        let (receiver, _, grant, demands, copy) = setup_copy().await;
        let source = demands.source_copy(copy, &grant).unwrap();
        let native = NativeClipboard::new(available.then(|| {
            Owner::new(
                "unused".into(),
                Board::Test {
                    name: "unused".into(),
                },
            )
        }));
        drop(native.publish(1, source.offer().offer.formats.clone(), source.clone()));
        assert!(!source.copy().unwrap().is_current());
        assert!(receiver.copy_release().is_some());
    }
}
