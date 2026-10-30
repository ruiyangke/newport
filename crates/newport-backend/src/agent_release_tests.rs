use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign::KeyPair;
use newport_release::needle::{Builds, Envelope, Manifest};
use std::io::Cursor;

fn metadata(pair: &KeyPair, version: &str, hash: &str) -> Vec<u8> {
    product_metadata(pair, version, hash, None)
}

fn product_metadata(
    pair: &KeyPair,
    version: &str,
    hash: &str,
    agent: Option<newport_release::agent_product::Product>,
) -> Vec<u8> {
    let bytes = serde_json::to_vec(&Manifest {
        schema_version: if agent.is_some() { 2 } else { 1 },
        product: "newport-needle".into(),
        version: version.into(),
        builds: Builds {
            x86_64: hash.into(),
            aarch64: hash.into(),
        },
        agent,
    })
    .unwrap();
    let comment = format!("file:needle-release-manifest.json\tversion:{version}");
    let signature = minisign::sign(
        Some(&pair.pk),
        &pair.sk,
        Cursor::new(&bytes),
        Some(&comment),
        None,
    )
    .unwrap();
    serde_json::to_vec(&Envelope {
        manifest: STANDARD.encode(bytes),
        signature: STANDARD.encode(signature.to_string()),
    })
    .unwrap()
}

#[test]
fn full_installations_are_preserved_by_core_clients_and_core_follows_release_order_to_full() {
    use newport_release::agent_product::{CORE_SERVICES, Product};
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
    let verify = |bytes: &[u8]| newport_release::needle::verify(bytes, &public);
    let mut full = Product {
        variant: Variant::Full,
        control_versions: vec![1],
        services: CORE_SERVICES
            .iter()
            .map(|name| (name.to_string(), vec![1]))
            .collect(),
    };
    full.services.insert("remote-apps".into(), vec![1]);
    let old_hash = "a".repeat(64);
    let candidate_hash = payload("x86_64").unwrap().sha256;
    let installed = product_metadata(&pair, "1.0.0", &old_hash, Some(full.clone()));
    let bundled = metadata(&pair, "2.0.0", candidate_hash);
    let output = format!(
        "Linux\nx86_64\n{old_hash}\n{}",
        String::from_utf8(installed).unwrap()
    );
    let mut status = parse_release_check(&output, &bundled, verify).unwrap();
    status.withhold_product_change();
    assert_eq!(status.installed_variant, Some(Variant::Full));
    assert_eq!(status.bundled_variant, Variant::Core);
    assert_eq!(status.update_available, Some(false));
    assert!(!status.automatic_update_available());
    assert!(!status.own_runtime_available());
    assert!(!Trigger::Required.allows(&status));
    assert!(require_product_replacement(&status, Trigger::Manual).is_err());
    // An older full agent isn't kept as installed: the person's own install
    // stages this Newport's build beside it.
    assert!(kept_installed(&status).is_none());
    assert_eq!(
        activation_for(&status, Trigger::Manual),
        Ok(Activation::Keep)
    );
    assert!(activation_for(&status, Trigger::Automatic).is_err());

    // A full agent of a newer release serves this Newport as it is.
    let newer_full = product_metadata(&pair, "3.0.0", &old_hash, Some(full.clone()));
    let newer_full = parse_release_check(
        &format!(
            "Linux\nx86_64\n{old_hash}\n{}",
            String::from_utf8(newer_full).unwrap()
        ),
        &bundled,
        verify,
    )
    .unwrap();
    let kept = kept_installed(&newer_full).expect("the newer full agent is kept");
    assert_eq!(kept.build_sha256, old_hash);
    assert!(kept.output.contains("already installed"));

    let bundled_full = product_metadata(&pair, "2.0.0", candidate_hash, Some(full.clone()));
    let same_variant = parse_release_check(&output, &bundled_full, verify).unwrap();
    assert!(kept_installed(&same_variant).is_none());
    assert!(same_variant.automatic_update_available());
    assert!(require_product_replacement(&same_variant, Trigger::Automatic).is_ok());

    // Core of an older or the same release moves to the bundled full agent
    // like any update; the entry point still follows release order.
    let core = |version| {
        let installed = String::from_utf8(metadata(&pair, version, &old_hash)).unwrap();
        let mut status = parse_release_check(
            &format!("Linux\nx86_64\n{old_hash}\n{installed}"),
            &bundled_full,
            verify,
        )
        .unwrap();
        status.withhold_product_change();
        assert_eq!(status.installed_variant, Some(Variant::Core));
        assert_eq!(status.bundled_variant, Variant::Full);
        status
    };
    let upgrade = core("1.0.0");
    assert!(kept_installed(&upgrade).is_none());
    assert_eq!(upgrade.build_relation, BuildRelation::Older);
    assert_eq!(upgrade.update_available, Some(true));
    assert!(upgrade.automatic_update_available());
    assert!(Trigger::Required.allows(&upgrade));
    assert_eq!(Trigger::Automatic.activation(&upgrade), Activation::Replace);
    for trigger in [Trigger::Automatic, Trigger::Required, Trigger::Manual] {
        assert!(require_product_replacement(&upgrade, trigger).is_ok());
    }
    let mut same_release = core("2.0.0");
    assert_eq!(same_release.build_relation, BuildRelation::SameRelease);
    assert!(same_release.own_runtime_available());
    assert!(require_product_replacement(&same_release, Trigger::Automatic).is_ok());
    same_release.installed_running = Some(true);
    assert_eq!(
        Trigger::Automatic.activation(&same_release),
        Activation::Keep
    );
    same_release.installed_running = Some(false);
    assert_eq!(
        Trigger::Automatic.activation(&same_release),
        Activation::UnlessRunning
    );

    // A newer core release, or core without release order, changes product
    // only when the person installs this Newport's agent.
    for refused in [core("3.0.0"), {
        let mut unordered = core("1.0.0");
        unordered.build_relation = BuildRelation::Different;
        unordered
    }] {
        assert!(!refused.automatic_update_available());
        assert!(!Trigger::Required.allows(&refused));
        assert!(require_product_replacement(&refused, Trigger::Automatic).is_err());
        assert!(require_product_replacement(&refused, Trigger::Required).is_err());
        assert!(require_product_replacement(&refused, Trigger::Manual).is_ok());
        // Core lacks Remote apps, so even a newer one isn't kept for this
        // full Newport.
        assert!(kept_installed(&refused).is_none());
    }
}

#[test]
fn needle_follows_newport_except_over_a_newer_release() {
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
    let verify = |bytes: &[u8]| newport_release::needle::verify(bytes, &public);
    let bundled_hash = payload("x86_64").unwrap().sha256.to_owned();
    let bundled = metadata(&pair, "1.10.0", &bundled_hash);
    let hash = "a".repeat(64);
    for (version, relation, update, allowed) in [
        ("1.9.0", BuildRelation::Older, true, true),
        ("1.10.0-rc.1", BuildRelation::Older, true, true),
        ("1.10.0", BuildRelation::SameRelease, false, true),
        ("1.11.0", BuildRelation::Newer, false, false),
    ] {
        let installed = metadata(&pair, version, &hash);
        let output = format!(
            "Linux\nx86_64\n{hash}\n{}",
            String::from_utf8(installed).unwrap()
        );
        let status = parse_release_check(&output, &bundled, verify).unwrap();
        assert_eq!(status.build_relation, relation);
        assert_eq!(status.update_available, Some(update));
        // Anything but a newer release has automatic work: an older one is
        // replaced, another build of this release replaced when idle or run
        // beside (unknown here, so beside once this Newport's is staged).
        assert_eq!(status.automatic_update_available(), allowed);
        assert_eq!(status.installed_version.as_deref(), Some(version));
        assert_eq!(status.bundled_version.as_deref(), Some("1.10.0"));
        assert_eq!(require_ordered_replacement(&status).is_ok(), allowed);
        // A newer release counts as installed: the person's own install
        // leaves it and installs nothing.
        assert_eq!(
            kept_installed(&status).map(|kept| kept.build_sha256),
            (!allowed).then(|| hash.clone())
        );
        assert_eq!(activation_for(&status, Trigger::Automatic).is_ok(), allowed);
        // A Newport without release order can't tell newer from older: its
        // own build replaces an idle one or runs beside it.
        let unsigned = parse_release_check(&output, &[], verify).unwrap();
        assert_eq!(unsigned.update_available, None);
        assert!(unsigned.automatic_update_available());
        assert!(require_ordered_replacement(&unsigned).is_ok());
    }
    let unsigned_remote = format!("Linux\nx86_64\n{hash}\n");
    let status = parse_release_check(&unsigned_remote, &bundled, verify).unwrap();
    assert_eq!(status.build_relation, BuildRelation::Different);
    assert_eq!(status.update_available, None);
    assert!(!status.automatic_update_available());
    assert!(require_product_replacement(&status, Trigger::Manual).is_err());
    assert!(require_ordered_replacement(&status).is_ok());
    // An installed product Newport can't identify stays; the install still succeeds.
    assert_eq!(
        activation_for(&status, Trigger::Manual),
        Ok(Activation::Keep)
    );
    assert!(activation_for(&status, Trigger::Required).is_err());
    let missing = parse_release_check("Linux\nx86_64\nmissing\n", &bundled, verify).unwrap();
    assert_eq!(missing.update_available, Some(false));
    assert_eq!(missing.installed, Some(false));
    assert!(!missing.automatic_update_available());

    // Resuming an old deployment must compare its recorded candidate, even
    // when this desktop itself bundles a newer, eligible release.
    let installed = String::from_utf8(metadata(&pair, "1.9.0", &hash)).unwrap();
    let output = format!("Linux\nx86_64\n{hash}\n{installed}");
    assert!(
        require_ordered_replacement(&parse_release_check(&output, &bundled, verify).unwrap())
            .is_ok()
    );
    let old_hash = "b".repeat(64);
    let old_candidate = metadata(&pair, "1.8.0", &old_hash);
    let resumed = parse_candidate_check(&output, &old_candidate, Some(&old_hash), verify).unwrap();
    assert_eq!(resumed.build_relation, BuildRelation::Newer);
    assert!(require_ordered_replacement(&resumed).is_err());
    let unordered = parse_candidate_check(&output, &[], Some(&old_hash), verify).unwrap();
    assert!(require_ordered_replacement(&unordered).is_ok());
}

#[test]
fn automatic_updates_reject_stale_evidence_and_throttle_reconnects() {
    let state = State::default();
    let known = Status {
        installed: Some(true),
        installed_variant: Some(Variant::Core),
        update_available: Some(true),
        build_relation: BuildRelation::Older,
        ..Status::default()
    };
    state.record(1, &Ok(known));
    assert!(state.snapshot().automatic_update_available());
    state.record(2, &Err("connection lost".into()));
    assert!(!state.snapshot().automatic_update_available());
    let now = Instant::now();
    assert!(state.claim_automatic_update(now));
    assert!(!state.claim_automatic_update(now));
    assert!(!state.claim_automatic_update(now + Duration::from_secs(3599)));
    assert!(state.claim_automatic_update(now + Duration::from_secs(3600)));
}

#[test]
fn automatic_updates_need_installation_approval() {
    let state = State::default();
    let mut server: crate::model::Server = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(), "name": "Automatic update fixture", "sshHost": "127.0.0.1",
        "sshPort": 22, "sshUser": "fixture", "agentInstallApproved": false
    }))
    .unwrap();
    assert!(!state.automatic_update_unblocked(&server));
    server.agent_install_approved = true;
    assert!(state.automatic_update_unblocked(&server));
}

#[tokio::test]
async fn automatic_update_defers_to_an_active_manual_operation() {
    let state = Arc::new(State::default());
    let server: crate::model::Server = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(), "name": "fixture", "sshHost": "127.0.0.1",
        "sshPort": 22, "sshUser": "fixture", "agentInstallApproved": true
    }))
    .unwrap();
    let (_retire, retired) = tokio::sync::watch::channel(false);
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn({
        let state = state.clone();
        async move {
            state
                .tasks
                .run(
                    crate::lifecycle_task::Request {
                        generation: 1,
                        action: crate::lifecycle_task::Action::Install {
                            integrations: false,
                        },
                    },
                    retired,
                    async move {
                        started.send(()).unwrap();
                        std::future::pending().await
                    },
                )
                .await
        }
    });
    ready.await.unwrap();
    assert!(!state.automatic_update_unblocked(&server));
    state.tasks.stop().await;
    assert!(task.await.unwrap().is_err());
    assert!(state.automatic_update_unblocked(&server));
}

#[test]
fn discovery_rejects_valid_signatures_for_the_wrong_binary_and_preserves_evidence() {
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
    let verify = |bytes: &[u8]| newport_release::needle::verify(bytes, &public);
    let installed = metadata(&pair, "1.0.0", &"a".repeat(64));
    let output = format!(
        "Linux\nx86_64\n{}\n{}",
        "b".repeat(64),
        String::from_utf8(installed).unwrap()
    );
    assert!(parse_release_check(&output, &[], verify).is_err());
    let known =
        parse_release_check(&format!("Linux\nx86_64\n{}\n", "b".repeat(64)), &[], verify).unwrap();
    let state = State::default();
    state.record(1, &Ok(known));
    state.record(1, &parse_release_check(&output, &[], verify));
    assert_eq!(state.snapshot().installed, Some(true));
    assert_eq!(
        state.snapshot().installed_hash.as_deref(),
        Some("b".repeat(64).as_str())
    );
    assert!(state.snapshot().error.is_some());
}

#[tokio::test]
#[ignore = "Requires the disposable Linux OpenSSH fixture"]
async fn remote_release_identity_is_recorded_once_and_checks_do_not_write() {
    let server: crate::model::Server = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(), "name": "Release identity fixture", "sshUser": "fixture",
        "sshHost": "127.0.0.1", "sshPort": std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile": std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved": true
    })).unwrap();
    let link = crate::ssh::link(&server, crate::ssh::no_jump_hosts);
    let session = &link;
    let run = |command: &str| {
        let command = command.to_owned();
        async move { command::stdout(session, &command, None).await }
    };
    manage_test(session, &server, false, false).await.unwrap();
    let initial = check(session).await.unwrap();
    let hash = initial.installed_hash.unwrap();
    let architecture = run("uname -m").await.unwrap();
    let architecture = architecture.trim();
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
    let verify = |bytes: &[u8]| newport_release::needle::verify(bytes, &public);
    let release = metadata(&pair, "1.2.3", &hash);
    record_release(session, &hash, architecture, &release, verify)
        .await
        .unwrap();
    let stamp_command = format!(
        "stat -c '%i:%y:%s' \"$HOME/.local/share/newport/needle/builds/{hash}/needle-release.json\""
    );
    let stamp = run(&stamp_command).await.unwrap();
    // A second desktop signing the same bytes must preserve the first record.
    record_release(
        session,
        &hash,
        architecture,
        &metadata(&pair, "1.2.4", &hash),
        verify,
    )
    .await
    .unwrap();
    let output = run(CHECK).await.unwrap();
    let observed = parse_release_check(&output, &[], verify).unwrap();
    assert_eq!(observed.installed_version.as_deref(), Some("1.2.3"));
    assert_eq!(observed.installed_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(run(&stamp_command).await.unwrap(), stamp);
    // The real trust root cannot accept this test key, and deployment refuses
    // the invalid metadata before modifying the active executable.
    assert!(check(session).await.is_err());
    assert!(manage_test(session, &server, true, false).await.is_err());
    assert_eq!(run(&stamp_command).await.unwrap(), stamp);
    run(&format!(
        "printf tampered > \"$HOME/.local/share/newport/needle/builds/{hash}/needle-release.json\""
    ))
    .await
    .unwrap();
    assert!(
        record_release(session, &hash, architecture, &release, verify)
            .await
            .is_err()
    );
    assert!(parse_release_check(&run(CHECK).await.unwrap(), &[], verify).is_err());
}

#[test]
fn agent_identity_reports_embedded_payloads_and_only_a_verified_matching_envelope() {
    use newport_release::agent_product::{CORE_SERVICES, Product};
    use sha2::{Digest, Sha256};
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
    let verify = |bytes: &[u8]| newport_release::needle::verify(bytes, &public);
    let (x86_64, aarch64) = (payload("x86_64").unwrap(), payload("aarch64").unwrap());
    let unsigned = agent_report(&[], verify).unwrap();
    assert_eq!(unsigned.schema, 1);
    assert_eq!(unsigned.variant, crate::agent_payload::variant());
    assert_eq!(unsigned.payloads.x86_64.sha256, x86_64.sha256);
    assert_eq!(unsigned.payloads.aarch64.size, aarch64.size as u64);
    assert!(unsigned.envelope.is_none());
    let mut product = Product {
        variant: Variant::Full,
        control_versions: vec![1],
        services: CORE_SERVICES
            .iter()
            .map(|name| (name.to_string(), vec![1]))
            .collect(),
    };
    product.services.insert("remote-apps".into(), vec![1]);
    let signed = |x86: &str, arm: &str| {
        let bytes = serde_json::to_vec(&Manifest {
            schema_version: 2,
            product: "newport-needle".into(),
            version: "1.2.3".into(),
            builds: Builds {
                x86_64: x86.into(),
                aarch64: arm.into(),
            },
            agent: Some(product.clone()),
        })
        .unwrap();
        let comment = "file:needle-release-manifest.json\tversion:1.2.3";
        let signature = minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(&bytes),
            Some(comment),
            None,
        )
        .unwrap();
        serde_json::to_vec(&Envelope {
            manifest: STANDARD.encode(bytes),
            signature: STANDARD.encode(signature.to_string()),
        })
        .unwrap()
    };
    let envelope = signed(x86_64.sha256, aarch64.sha256);
    let report = agent_report(&envelope, verify).unwrap();
    let embedded = report.envelope.as_ref().unwrap();
    assert_eq!(embedded.version, "1.2.3");
    assert_eq!(embedded.product.as_ref(), Some(&product));
    assert_eq!(embedded.size, envelope.len() as u64);
    assert_eq!(
        embedded.sha256,
        Sha256::digest(&envelope)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let parsed: newport_release::desktop::AgentReport =
        serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    assert_eq!(parsed, report);
    let other = "0".repeat(64);
    for wrong in [
        signed(x86_64.sha256, &other),
        signed(&other, aarch64.sha256),
    ] {
        assert!(agent_report(&wrong, verify).is_err());
    }
    let mut tampered = envelope.clone();
    tampered[20] ^= 1;
    assert!(agent_report(&tampered, verify).is_err());
}
