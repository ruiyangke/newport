use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign::KeyPair;
use newport_release::needle::{Builds, Envelope, Manifest};
use std::io::Cursor;

fn metadata(pair: &KeyPair, version: &str, hash: &str) -> Vec<u8> {
    let bytes = serde_json::to_vec(&Manifest {
        schema_version: 1,
        product: "newport-needle".into(),
        version: version.into(),
        builds: Builds {
            x86_64: hash.into(),
            aarch64: hash.into(),
        },
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
fn signed_discovery_orders_versions_and_prevents_downgrade() {
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
        assert_eq!(status.installed_version.as_deref(), Some(version));
        assert_eq!(status.bundled_version.as_deref(), Some("1.10.0"));
        assert_eq!(require_ordered_replacement(&status).is_ok(), allowed);
        // A local unsigned build cannot replace an ordered release silently.
        let unsigned = parse_release_check(&output, &[], verify).unwrap();
        assert_eq!(unsigned.update_available, None);
        assert!(require_ordered_replacement(&unsigned).is_err());
    }
    let unsigned_remote = format!("Linux\nx86_64\n{hash}\n");
    let status = parse_release_check(&unsigned_remote, &bundled, verify).unwrap();
    assert_eq!(status.build_relation, BuildRelation::Different);
    assert_eq!(status.update_available, None);
    // Explicit replacement of an unordered build remains available.
    assert!(require_ordered_replacement(&status).is_ok());
    let missing = parse_release_check("Linux\nx86_64\nmissing\n", &bundled, verify).unwrap();
    assert_eq!(missing.update_available, Some(false));
    assert_eq!(missing.installed, Some(false));

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
    assert!(require_ordered_replacement(&unordered).is_err());
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
    let session = ExecSession::connect(&server).await.unwrap();
    manage_test(&session, false, false).await.unwrap();
    let initial = check(&session).await.unwrap();
    let hash = initial.installed_hash.unwrap();
    let architecture = session.execute("uname -m", None).await.unwrap();
    let architecture = architecture.trim();
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
    let verify = |bytes: &[u8]| newport_release::needle::verify(bytes, &public);
    let release = metadata(&pair, "1.2.3", &hash);
    record_release(&session, &hash, architecture, &release, verify)
        .await
        .unwrap();
    let stamp_command = format!(
        "stat -c '%i:%y:%s' \"$HOME/.local/share/newport/needle/builds/{hash}/needle-release.json\""
    );
    let stamp = session.execute(&stamp_command, None).await.unwrap();
    // A second desktop signing the same bytes must preserve the first record.
    record_release(
        &session,
        &hash,
        architecture,
        &metadata(&pair, "1.2.4", &hash),
        verify,
    )
    .await
    .unwrap();
    let output = session.execute(CHECK, None).await.unwrap();
    let observed = parse_release_check(&output, &[], verify).unwrap();
    assert_eq!(observed.installed_version.as_deref(), Some("1.2.3"));
    assert_eq!(observed.installed_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(session.execute(&stamp_command, None).await.unwrap(), stamp);
    // The real trust root cannot accept this test key, and deployment refuses
    // the invalid metadata before modifying the active executable.
    assert!(check(&session).await.is_err());
    assert!(manage_test(&session, true, false).await.is_err());
    assert_eq!(session.execute(&stamp_command, None).await.unwrap(), stamp);
    session.execute(&format!("printf tampered > \"$HOME/.local/share/newport/needle/builds/{hash}/needle-release.json\""), None).await.unwrap();
    assert!(
        record_release(&session, &hash, architecture, &release, verify)
            .await
            .is_err()
    );
    assert!(
        parse_release_check(&session.execute(CHECK, None).await.unwrap(), &[], verify).is_err()
    );
    session.close().await;
}
