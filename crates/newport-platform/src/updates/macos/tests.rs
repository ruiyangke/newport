use super::*;
use std::io::Write;

fn metadata(identifier: &str, version: &str, executable: &str) -> Vec<u8> {
    let mut dict = plist::Dictionary::new();
    for (key, value) in [
        ("CFBundleIdentifier", identifier),
        ("CFBundleShortVersionString", version),
        ("CFBundleExecutable", executable),
        ("CFBundlePackageType", "APPL"),
    ] {
        dict.insert(key.into(), plist::Value::String(value.into()));
    }
    let mut bytes = Vec::new();
    plist::Value::Dictionary(dict)
        .to_writer_xml(&mut bytes)
        .unwrap();
    bytes
}
fn macho(cpu: u32) -> Vec<u8> {
    let mut header = vec![0; 32];
    header[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
    header[4..8].copy_from_slice(&cpu.to_le_bytes());
    header[12..16].copy_from_slice(&2u32.to_le_bytes());
    header
}
fn archive(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (path, kind, bytes, target) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        header.set_entry_type(*kind);
        header.set_size(bytes.len() as u64);
        // Build malicious traversal entries too, without the writer's path guard.
        let raw = header.as_mut_bytes();
        raw[..100].fill(0);
        raw[..path.len()].copy_from_slice(path.as_bytes());
        if let Some(target) = target {
            header.set_link_name(target).unwrap();
        }
        header.set_cksum();
        builder.append(&header, *bytes).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}
pub(super) fn write_bundle(
    root: &Path,
    identifier: &str,
    version: &str,
    executable: &str,
    cpu: u32,
) -> PathBuf {
    let bundle = root.join("Newport.app");
    fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
    fs::write(
        bundle.join("Contents/Info.plist"),
        metadata(identifier, version, executable),
    )
    .unwrap();
    let binary = bundle.join("Contents/MacOS/newport");
    fs::File::create(&binary)
        .unwrap()
        .write_all(&macho(cpu))
        .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(binary, fs::Permissions::from_mode(0o755)).unwrap();
    bundle
}

#[test]
fn accepts_bundle_layout_and_internal_framework_aliases() {
    let directory = tempfile::tempdir().unwrap();
    let plist = metadata(IDENTIFIER, "2.0.0", "newport");
    let binary = macho(0x0100000c);
    let bytes = archive(&[
        (
            "Newport.app/Contents/Info.plist",
            tar::EntryType::Regular,
            &plist,
            None,
        ),
        (
            "Newport.app/Contents/MacOS/newport",
            tar::EntryType::Regular,
            &binary,
            None,
        ),
        (
            "Newport.app/Contents/Frameworks/F/Versions/A/file",
            tar::EntryType::Regular,
            b"framework",
            None,
        ),
        (
            "Newport.app/Contents/Frameworks/F/Versions/Current",
            tar::EntryType::Symlink,
            b"",
            Some("A"),
        ),
    ]);
    extract(&bytes, directory.path()).unwrap();
    let bundle = directory.path().join("Newport.app");
    assert_eq!(bundle_version(&bundle).unwrap(), version("2.0.0").unwrap());
    validate_executable(&bundle).unwrap();
    assert_eq!(
        fs::read(bundle.join("Contents/Frameworks/F/Versions/Current/file")).unwrap(),
        b"framework"
    );
}

#[test]
fn refuses_archive_traversal_alternate_roots_duplicates_and_special_entries() {
    for entries in [
        vec![(
            "../outside",
            tar::EntryType::Regular,
            b"bad".as_slice(),
            None,
        )],
        vec![(
            "Newport.app/../../outside",
            tar::EntryType::Regular,
            b"bad".as_slice(),
            None,
        )],
        vec![(
            "Other.app/file",
            tar::EntryType::Regular,
            b"bad".as_slice(),
            None,
        )],
        vec![
            (
                "Newport.app/file",
                tar::EntryType::Regular,
                b"one".as_slice(),
                None,
            ),
            (
                "Newport.app/file",
                tar::EntryType::Regular,
                b"two".as_slice(),
                None,
            ),
        ],
        vec![(
            "Newport.app/device",
            tar::EntryType::Fifo,
            b"".as_slice(),
            None,
        )],
        vec![(
            "Newport.app/hardlink",
            tar::EntryType::Link,
            b"".as_slice(),
            Some("Newport.app/file"),
        )],
    ] {
        let directory = tempfile::tempdir().unwrap();
        assert!(extract(&archive(&entries), directory.path()).is_err());
        assert!(!directory.path().join("outside").exists());
    }
}

#[test]
fn refuses_escaping_symlinks_and_writes_through_internal_aliases() {
    for target in ["/tmp", "../../outside"] {
        let directory = tempfile::tempdir().unwrap();
        let bytes = archive(&[(
            "Newport.app/link",
            tar::EntryType::Symlink,
            b"",
            Some(target),
        )]);
        assert!(extract(&bytes, directory.path()).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    let bytes = archive(&[
        (
            "Newport.app/Contents/actual",
            tar::EntryType::Directory,
            b"",
            None,
        ),
        (
            "Newport.app/Contents/alias",
            tar::EntryType::Symlink,
            b"",
            Some("actual"),
        ),
        (
            "Newport.app/Contents/alias/file",
            tar::EntryType::Regular,
            b"bad",
            None,
        ),
    ]);
    assert!(extract(&bytes, directory.path()).is_err());
    assert!(
        !directory
            .path()
            .join("Newport.app/Contents/actual/file")
            .exists()
    );
}

#[test]
fn rejects_wrong_identity_architecture_version_and_executable_escape() {
    let root = tempfile::tempdir().unwrap();
    let bundle = write_bundle(root.path(), "another.app", "2.0.0", "newport", 0x0100000c);
    assert!(bundle_version(&bundle).is_err());
    let bundle = write_bundle(root.path(), IDENTIFIER, "2.0.0", "newport", 0x01000007);
    assert!(validate_executable(&bundle).is_err());
    assert!(
        validate_candidate(&bundle, &version("3.0.0").unwrap(), "TEST")
            .err()
            .unwrap()
            .contains("version")
    );
    let bundle = write_bundle(root.path(), IDENTIFIER, "2.0.0", "../escape", 0x0100000c);
    assert!(validate_executable(&bundle).is_err());
}

#[test]
fn unsigned_bundles_are_not_eligible_and_preparation_preserves_the_installed_app() {
    let root = tempfile::tempdir().unwrap();
    let bundle = write_bundle(root.path(), IDENTIFIER, "1.0.0", "newport", 0x0100000c);
    assert!(signing_team(&bundle).is_err());
    let before = fs::read(bundle.join("Contents/Info.plist")).unwrap();
    let update = VerifiedUpdate {
        artifact_name: None,
        version: version("2.0.0").unwrap(),
        target: Target::MacArm64,
        bytes: Vec::new(),
    };
    assert!(PreparedBundle::prepare(&update, &bundle).is_err());
    assert_eq!(
        fs::read(bundle.join("Contents/Info.plist")).unwrap(),
        before
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn rejects_oversized_extension_records_before_decoding_their_paths() {
    let mut header = tar::Header::new_gnu();
    header.set_path("././@LongLink").unwrap();
    header.set_entry_type(tar::EntryType::GNULongName);
    header.set_size(2 * 1024 * 1024);
    header.set_cksum();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(header.as_bytes()).unwrap();
    let bytes = encoder.finish().unwrap();
    let root = tempfile::tempdir().unwrap();
    assert!(
        extract(&bytes, root.path())
            .unwrap_err()
            .contains("metadata exceeds")
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn dropping_preparation_cleans_only_its_private_candidate() {
    let root = tempfile::tempdir().unwrap();
    let destination = write_bundle(root.path(), IDENTIFIER, "1.0.0", "newport", 0x0100000c);
    let staging = tempfile::tempdir_in(root.path()).unwrap();
    let candidate_root = staging.path().to_owned();
    write_bundle(staging.path(), IDENTIFIER, "2.0.0", "newport", 0x0100000c);
    let prepared = PreparedBundle {
        staging,
        destination: destination.clone(),
        expected_version: version("2.0.0").unwrap(),
        installed_version: version("1.0.0").unwrap(),
        team: "TEST".into(),
    };
    assert!(prepared.revalidate().is_err());
    drop(prepared);
    assert!(destination.exists());
    assert!(!candidate_root.exists());
}
