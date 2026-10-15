use base64::{Engine, engine::general_purpose::STANDARD};
use minisign::KeyPair;
use std::{fs, process::Command};

#[test]
fn cli_signs_from_contents_or_a_path_and_rejects_wrong_keys_without_replacing_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let pair = KeyPair::generate_unencrypted_keypair().unwrap();
    let encoded = STANDARD.encode(pair.sk.to_box(None).unwrap().to_string());
    let key_path = temp.path().join("secret.key");
    let public_path = temp.path().join("public.key");
    let artifact = temp.path().join("Newport fixture.exe");
    fs::write(&key_path, &encoded).unwrap();
    fs::write(
        &public_path,
        STANDARD.encode(pair.pk.to_box().unwrap().to_string()),
    )
    .unwrap();
    fs::write(&artifact, b"test release").unwrap();
    let invoke = |from_file: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_newport-release"));
        command.args([
            "sign".as_ref(),
            artifact.as_os_str(),
            "1.2.3".as_ref(),
            public_path.as_os_str(),
        ]);
        command
            .env_remove("NEWPORT_SIGNING_PRIVATE_KEY")
            .env_remove("NEWPORT_SIGNING_PRIVATE_KEY_PATH");
        command.env("NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD", "");
        if from_file {
            command.env("NEWPORT_SIGNING_PRIVATE_KEY_PATH", &key_path);
        } else {
            command.env("NEWPORT_SIGNING_PRIVATE_KEY", &encoded);
        }
        command.output().unwrap()
    };
    for from_file in [false, true] {
        let output = invoke(from_file);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&encoded));
        let signature = String::from_utf8(
            STANDARD
                .decode(fs::read(artifact.with_added_extension("sig")).unwrap())
                .unwrap(),
        )
        .unwrap();
        let signature = minisign_verify::Signature::decode(&signature).unwrap();
        let public =
            minisign_verify::PublicKey::decode(&pair.pk.to_box().unwrap().to_string()).unwrap();
        public.verify(b"test release", &signature, true).unwrap();
    }
    let before = fs::read(artifact.with_added_extension("sig")).unwrap();
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    fs::write(
        &public_path,
        STANDARD.encode(other.pk.to_box().unwrap().to_string()),
    )
    .unwrap();
    let result = invoke(false);
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains(&encoded));
    assert_eq!(
        fs::read(artifact.with_added_extension("sig")).unwrap(),
        before
    );
}
