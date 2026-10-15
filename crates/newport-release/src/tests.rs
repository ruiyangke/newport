use super::*;
use minisign::KeyPair;
use std::fs;

#[path = "needle_tests.rs"]
mod needle;

fn public(pair: &KeyPair) -> String {
    STANDARD.encode(pair.pk.to_box().unwrap().to_string())
}

#[test]
fn signatures_match_the_updaters_verifier_and_bind_the_version() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("Newport.app.tar.gz");
    fs::write(&artifact, b"fixture package").unwrap();
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let output = sign(&artifact, "1.2.3", &public(&pair), &pair.sk).unwrap();
    assert_eq!(output.file_name().unwrap(), "Newport.app.tar.gz.sig");
    let text = String::from_utf8(STANDARD.decode(fs::read(output).unwrap()).unwrap()).unwrap();
    let key = minisign_verify::PublicKey::decode(&pair.pk.to_box().unwrap().to_string()).unwrap();
    let signature = minisign_verify::Signature::decode(&text).unwrap();
    key.verify(b"fixture package", &signature, true).unwrap();
    assert!(text.contains("\tfile:Newport.app.tar.gz\tversion:1.2.3\n"));
    assert!(key.verify(b"modified package", &signature, true).is_err());
    let forged =
        minisign_verify::Signature::decode(&text.replace("version:1.2.3", "version:9.0.0"))
            .unwrap();
    assert!(key.verify(b"fixture package", &forged, true).is_err());
}

#[test]
fn invalid_versions_names_and_wrong_keys_preserve_the_existing_signature() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("Newport.exe");
    fs::write(&artifact, b"fixture package").unwrap();
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let wrong = KeyPair::generate_unencrypted_keypair().unwrap();
    let output = artifact.with_added_extension("sig");
    fs::write(&output, b"previous signature").unwrap();
    assert!(sign(&artifact, "1.2.3", &public(&wrong), &pair.sk).is_err());
    for version in ["1.2", "v1.2.3", "1.2.3\tversion:9.0.0", "1.2.3\n"] {
        assert!(sign(&artifact, version, &public(&pair), &pair.sk).is_err());
    }
    assert!(
        sign(
            &temp.path().join("bad\tversion:9.exe"),
            "1.2.3",
            &public(&pair),
            &pair.sk
        )
        .is_err()
    );
    assert_eq!(fs::read(output).unwrap(), b"previous signature");
}

#[test]
fn encrypted_key_import_accepts_the_password_and_never_echoes_key_material() {
    let pair = KeyPair::generate_encrypted_keypair(Some("test-only-password".into())).unwrap();
    let encoded = STANDARD.encode(pair.sk.to_box(None).unwrap().to_string());
    let key = secret_key(&encoded, "test-only-password".into()).unwrap();
    assert_eq!(minisign::PublicKey::from_secret_key(&key).unwrap(), pair.pk);
    let error = secret_key(&encoded, "incorrect".into())
        .unwrap_err()
        .to_string();
    assert!(!error.contains(&encoded));
    assert!(!error.contains("incorrect"));
    assert!(secret_key("invalid-private-material", String::new()).is_err());
}

#[test]
fn imports_shipping_tauri_keys_with_empty_and_nonempty_passwords() {
    for (encoded, password, public) in [
        (
            include_str!("../tests/fixtures/tauri-empty.key"),
            "",
            include_str!("../tests/fixtures/tauri-empty.key.pub"),
        ),
        (
            include_str!("../tests/fixtures/tauri-password.key"),
            "fixture-password",
            include_str!("../tests/fixtures/tauri-password.key.pub"),
        ),
    ] {
        let key = secret_key(encoded, password.into()).unwrap();
        let expected = String::from_utf8(STANDARD.decode(public.trim()).unwrap()).unwrap();
        let expected = PublicKeyBox::from_string(&expected)
            .unwrap()
            .into_public_key()
            .unwrap();
        assert_eq!(
            minisign::PublicKey::from_secret_key(&key).unwrap(),
            expected
        );
        assert!(secret_key(encoded, "wrong password".into()).is_err());
    }
}
