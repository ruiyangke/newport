//! Opt-in runtime acceptance using a real, locally built NSIS candidate.
//! Never point this at an existing installation or run with production keys.
#![cfg(windows)]

use anyhow::{Context, Result, bail, ensure};
use newport_protocol::product::Variant;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read},
    os::windows::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::Shell::{CSIDL_DESKTOPDIRECTORY, CSIDL_PROGRAMS, SHGetFolderPathW};
use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_64KEY},
};

const PRODUCT: &str = r"Software\newport\Newport";
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Newport";
const CANARY: &[u8] = b"Preserve this co-located profile, including on uninstall.\n";
const METADATA_LIMIT: u64 = 64 * 1024;
const AGENT_NOTICES: &str = "agents/licenses/";

fn expected_variant(value: Option<&str>) -> Result<Variant> {
    match value {
        None | Some("core") => Ok(Variant::Core),
        Some("full") => Ok(Variant::Full),
        Some(_) => bail!("NEWPORT_TEST_INSTALLER_VARIANT must be core or full"),
    }
}

/// The installed files: the list `newport-package.json` carries, which must
/// fit the composition the caller expects, and the uninstaller.
fn listed(bytes: &[u8], expected: Variant) -> Result<Vec<String>> {
    validate_metadata_variant(bytes, expected)?;
    let metadata: serde_json::Value = serde_json::from_slice(bytes)?;
    let mut files: Vec<String> = serde_json::from_value(
        metadata
            .get("files")
            .cloned()
            .context("Package metadata lists no files")?,
    )
    .context("Package files must be a list of paths")?;
    let mut unique = BTreeSet::new();
    for file in &files {
        ensure!(
            !file.is_empty()
                && !file.contains('\\')
                && file
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")
                && unique.insert(file.as_str()),
            "Invalid or repeated package file: {file}"
        );
    }
    for required in [
        "newport.exe",
        "newport-package.json",
        "grammars/catalog.json",
    ] {
        ensure!(
            unique.contains(required),
            "Package does not list {required}"
        );
    }
    ensure!(
        files
            .iter()
            .any(|file| file.starts_with("grammars/") && file.ends_with(".dll")),
        "Package lists no grammar libraries"
    );
    let notices = files
        .iter()
        .filter(|file| file.starts_with(AGENT_NOTICES))
        .count();
    ensure!(
        (notices > 0) == (expected == Variant::Full),
        "Package agent notices do not match the expected variant"
    );
    files.push("uninstall.exe".into());
    Ok(files)
}

fn payload(destination: &Path, expected: Variant) -> Result<Vec<String>> {
    let metadata = destination.join("newport-package.json");
    ensure!(
        unlinked_metadata(&metadata)?.is_file(),
        "Expected regular package metadata"
    );
    let mut bytes = Vec::new();
    File::open(metadata)?
        .take(METADATA_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= METADATA_LIMIT,
        "Package metadata exceeds its limit"
    );
    listed(&bytes, expected)
}

fn unlinked_metadata(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    ensure!(
        !metadata.file_type().is_symlink()
            && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "Installed payload contains a link or reparse point"
    );
    Ok(metadata)
}

fn validate_metadata_variant(bytes: &[u8], expected: Variant) -> Result<()> {
    let metadata: serde_json::Value = serde_json::from_slice(bytes)?;
    let metadata = metadata
        .as_object()
        .context("Expected package metadata object")?;
    let actual = match metadata.get("remoteApps") {
        None | Some(serde_json::Value::Bool(false)) => Variant::Core,
        Some(serde_json::Value::Bool(true)) => Variant::Full,
        Some(_) => bail!("Package remoteApps must be a boolean"),
    };
    ensure!(
        actual == expected,
        "Installed package has the wrong expected variant"
    );
    Ok(())
}

/// The installed agent notices are exactly the listed ones; Core has none.
fn validate_installed_variant(destination: &Path, files: &[String]) -> Result<()> {
    let expected: BTreeSet<&str> = files
        .iter()
        .filter_map(|file| file.strip_prefix(AGENT_NOTICES))
        .collect();
    let directory = destination.join("agents/licenses");
    if expected.is_empty() {
        match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("Inspect Core agent notices"),
            Ok(_) => bail!("Core installation contains Full agent notices"),
        }
    }
    ensure!(
        unlinked_metadata(&directory)?.is_dir(),
        "Expected Full agent notice directory"
    );
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        ensure!(
            unlinked_metadata(&entry.path())?.is_file(),
            "Expected regular Full agent notice"
        );
        actual.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid Full notice filename"))?,
        );
    }
    ensure!(
        actual
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied()),
        "Full agent notice inventory mismatch"
    );
    Ok(())
}

fn registration(key: &str) -> Result<Option<RegKey>> {
    match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
    {
        Ok(key) => Ok(Some(key)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Read current-user installation registration"),
    }
}

fn unregistered() -> Result<bool> {
    Ok(registration(PRODUCT)?.is_none() && registration(UNINSTALL)?.is_none())
}

fn no_existing_shortcuts() -> Result<()> {
    for folder in [CSIDL_DESKTOPDIRECTORY, CSIDL_PROGRAMS] {
        let mut path = [0u16; 260];
        // SAFETY: SHGetFolderPathW requires a writable MAX_PATH buffer. Null
        // window/token select the current user's folder, matching NSIS.
        let result = unsafe {
            SHGetFolderPathW(
                std::ptr::null_mut(),
                folder as i32,
                std::ptr::null_mut(),
                0,
                path.as_mut_ptr(),
            )
        };
        ensure!(result == 0, "Cannot locate shortcut folder: {result:#x}");
        let end = path
            .iter()
            .position(|&value| value == 0)
            .context("Unterminated shortcut folder")?;
        let link = PathBuf::from(std::ffi::OsString::from_wide(&path[..end])).join("Newport.lnk");
        match fs::symlink_metadata(&link) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Inspect existing Newport shortcut"),
            Ok(_) => bail!(
                "Refusing to touch an existing Newport shortcut: {}",
                link.display()
            ),
        }
    }
    Ok(())
}

fn wait(command: &mut Command) -> Result<ExitStatus> {
    let mut child = command
        .spawn()
        .with_context(|| format!("Start {command:?}"))?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            // Do not terminate an installer in the middle of a journaled exchange.
            bail!(
                "Process {} timed out; retained for inspection: {command:?}",
                child.id()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn install_command(installer: &Path, destination: &Path, update: bool) -> Command {
    let mut command = Command::new(installer);
    command.args(["/S", "/NS"]);
    if update {
        command.arg("/UPDATE");
    }
    // NSIS requires /D last and unquoted, including when the path has spaces.
    // The destination is generated here, never supplied by an external request.
    use std::os::windows::process::CommandExt;
    command.raw_arg(format!("/D={}", destination.display()));
    command
}

fn install(installer: &Path, destination: &Path, update: bool) -> Result<ExitStatus> {
    wait(&mut install_command(installer, destination, update))
}

fn interrupted_upgrade(installer: &Path, destination: &Path) -> Result<()> {
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(destination.join("grammars/newport_grammar_json.dll"))?;
    let mut child = install_command(installer, destination, true).spawn()?;
    let backup = destination.join(".newport-install-backup/newport.exe");
    let deadline = Instant::now() + Duration::from_secs(90);
    // /UPDATE keeps retrying the locked DLL, leaving a deterministic
    // window after the executable was backed up but before promotion begins.
    while !backup.is_file() || destination.join("newport.exe").exists() {
        ensure!(
            child.try_wait()?.is_none(),
            "Upgrade exited before crash checkpoint"
        );
        ensure!(
            Instant::now() < deadline,
            "Upgrade did not reach crash checkpoint; process {} retained",
            child.id()
        );
        thread::sleep(Duration::from_millis(20));
    }
    child
        .kill()
        .context("Terminate only this test's interrupted installer")?;
    child.wait()?;
    drop(lock);
    ensure!(
        backup.is_file(),
        "Interrupted upgrade lost its executable backup"
    );
    ensure!(
        destination
            .join(".newport-install-stage/transaction.ini")
            .is_file(),
        "Interrupted upgrade lost its recovery journal"
    );
    ensure!(
        install(installer, destination, false)?.success(),
        "Interrupted upgrade did not recover on retry"
    );
    intact(destination)
}

fn fingerprint(destination: &Path, variant: Variant) -> Result<BTreeMap<String, [u8; 32]>> {
    let files = payload(destination, variant)?;
    validate_installed_variant(destination, &files)?;
    files
        .into_iter()
        .map(|name| {
            ensure!(
                unlinked_metadata(&destination.join(&name))?.is_file(),
                "Expected regular payload: {name}"
            );
            let mut file = File::open(destination.join(&name)).with_context(|| name.clone())?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            Ok((name, hash.finalize().into()))
        })
        .collect()
}

fn intact(destination: &Path) -> Result<()> {
    ensure!(
        fs::read(destination.join("profile-canary/keep.txt"))? == CANARY,
        "Profile changed"
    );
    for name in [".newport-install-stage", ".newport-install-backup"] {
        ensure!(!destination.join(name).exists(), "Installer left {name}");
    }
    let product = registration(PRODUCT)?.context("Missing product registration")?;
    ensure!(
        PathBuf::from(product.get_value::<String, _>("")?) == destination,
        "Wrong registered path"
    );
    let uninstall = registration(UNINSTALL)?.context("Missing uninstall registration")?;
    ensure!(
        uninstall.get_value::<String, _>("InstallLocation")?
            == format!("\"{}\"", destination.display()),
        "Wrong uninstall path"
    );
    Ok(())
}

fn exercise(installer: &Path, destination: &Path, variant: Variant) -> Result<()> {
    fs::create_dir_all(destination.join("profile-canary"))?;
    fs::write(destination.join("profile-canary/keep.txt"), CANARY)?;
    ensure!(
        install(installer, destination, false)?.success(),
        "Fresh install failed"
    );
    intact(destination)?;
    ensure!(
        wait(Command::new(destination.join("newport.exe")).arg("--check-grammars"))?.success(),
        "Installed grammars failed"
    );
    let original = fingerprint(destination, variant)?;
    // Locking the DLL fails after the old executable has already moved to the
    // backup. That case verifies actual rollback, not just refusal at entry.
    for name in ["newport.exe", "grammars/newport_grammar_json.dll"] {
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(destination.join(name))?;
        let status = install(installer, destination, false)?;
        drop(lock);
        ensure!(!status.success(), "Installer replaced locked {name}");
        intact(destination)?;
        ensure!(
            fingerprint(destination, variant)? == original,
            "Failed install modified payload after locking {name}"
        );
        ensure!(
            install(installer, destination, true)?.success(),
            "Upgrade retry failed after locking {name}"
        );
        intact(destination)?;
    }
    interrupted_upgrade(installer, destination)?;
    ensure!(
        fingerprint(destination, variant)? == original,
        "Recovery changed payload bytes"
    );
    ensure!(
        wait(Command::new(destination.join("newport.exe")).arg("--check-grammars"))?.success(),
        "Upgraded grammars failed"
    );
    let files = payload(destination, variant)?;
    ensure!(
        wait(Command::new(destination.join("uninstall.exe")).arg("/S"))?.success(),
        "Uninstaller failed"
    );
    // NSIS relaunches the uninstaller outside $INSTDIR. Wait for that child too.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !unregistered()? || files.iter().any(|name| destination.join(name).exists()) {
        ensure!(
            Instant::now() < deadline,
            "Uninstall did not remove files and registration"
        );
        thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        fs::read(destination.join("profile-canary/keep.txt"))? == CANARY,
        "Uninstall changed profile"
    );
    no_existing_shortcuts()?;
    for name in [
        "Fonts",
        "agents",
        "grammars",
        "Licenses",
        ".newport-install-stage",
        ".newport-install-backup",
    ] {
        ensure!(!destination.join(name).exists(), "Uninstall left {name}");
    }
    Ok(())
}

#[test]
#[ignore = "executes NEWPORT_TEST_INSTALLER (Core unless NEWPORT_TEST_INSTALLER_VARIANT=full); requires no Newport installation"]
fn native_installer_preserves_profiles_and_recovers_failed_replacements() -> Result<()> {
    // The caller selects the expected composition; the installed package's
    // own list must fit it, and names the files whose bytes and removal this
    // test verifies.
    let requested = std::env::var_os("NEWPORT_TEST_INSTALLER_VARIANT");
    let requested = requested
        .as_deref()
        .map(|value| value.to_str().context("Installer variant must be UTF-8"))
        .transpose()?;
    let variant = expected_variant(requested)?;
    let installer = PathBuf::from(
        std::env::var_os("NEWPORT_TEST_INSTALLER")
            .context("Set NEWPORT_TEST_INSTALLER to the native NSIS candidate")?,
    );
    ensure!(
        installer.is_absolute() && installer.is_file(),
        "Installer must be an absolute file path"
    );
    ensure!(
        unregistered()?,
        "Refusing to touch an existing Newport installation"
    );
    no_existing_shortcuts()?;
    // Keep the directory on any failure for recovery and diagnosis. Successful
    // cleanup removes only this test's freshly created, unique directory.
    let root = tempfile::Builder::new()
        .prefix("newport-installer-")
        .tempdir()?
        .keep();
    let destination = root.join("Newport test install");
    eprintln!("Installer acceptance workspace: {}", root.display());
    exercise(&installer, &destination, variant).with_context(|| {
        format!(
            "Installer test failed; files retained at {}",
            root.display()
        )
    })?;
    fs::remove_dir_all(root)?;
    Ok(())
}

/// A package list in the shape packaging writes, for the runtime checks.
fn fixture_files(variant: Variant) -> Vec<String> {
    let mut files: Vec<String> = [
        "newport.exe",
        "grammars/catalog.json",
        "grammars/newport_grammar_json.dll",
        "agents/newport-agent-x86_64.gz",
        "newport-package.json",
    ]
    .map(str::to_owned)
    .into();
    if variant == Variant::Full {
        files.extend(
            ["README.md", "newport-core.txt", "newport-full.txt"]
                .map(|name| format!("{AGENT_NOTICES}{name}")),
        );
    }
    files
}

fn fixture_metadata(variant: Variant, files: &[String]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "remoteApps": variant == Variant::Full,
        "files": files,
    }))
    .unwrap()
}

#[test]
fn runtime_inventory_is_the_packages_own_list_for_the_callers_variant() -> Result<()> {
    assert_eq!(expected_variant(None)?, Variant::Core);
    assert_eq!(expected_variant(Some("core"))?, Variant::Core);
    assert_eq!(expected_variant(Some("full"))?, Variant::Full);
    assert!(expected_variant(Some("automatic")).is_err());
    for variant in [Variant::Core, Variant::Full] {
        let files = fixture_files(variant);
        let mut installed = files.clone();
        installed.push("uninstall.exe".into());
        assert_eq!(
            listed(&fixture_metadata(variant, &files), variant)?,
            installed
        );
    }
    assert!(
        listed(
            &fixture_metadata(Variant::Core, &fixture_files(Variant::Full)),
            Variant::Core
        )
        .is_err()
    );
    assert!(
        listed(
            &fixture_metadata(Variant::Full, &fixture_files(Variant::Core)),
            Variant::Full
        )
        .is_err()
    );
    for missing in [
        "newport.exe",
        "newport-package.json",
        "grammars/newport_grammar_json.dll",
    ] {
        let files: Vec<String> = fixture_files(Variant::Core)
            .into_iter()
            .filter(|file| file != missing)
            .collect();
        assert!(
            listed(&fixture_metadata(Variant::Core, &files), Variant::Core).is_err(),
            "{missing}"
        );
    }
    for invalid in ["../outside", "agents\\licenses\\x", "newport.exe"] {
        let mut files = fixture_files(Variant::Core);
        files.push(invalid.into());
        assert!(
            listed(&fixture_metadata(Variant::Core, &files), Variant::Core).is_err(),
            "{invalid}"
        );
    }
    validate_metadata_variant(br#"{}"#, Variant::Core)?;
    validate_metadata_variant(br#"{"remoteApps":false}"#, Variant::Core)?;
    validate_metadata_variant(br#"{"remoteApps":true}"#, Variant::Full)?;
    for bytes in [
        br#"{}"#.as_slice(),
        br#"{"remoteApps":false,"files":[]}"#,
        br#"{"remoteApps":"full"}"#,
    ] {
        assert!(validate_metadata_variant(bytes, Variant::Full).is_err());
    }
    assert!(validate_metadata_variant(br#"{"remoteApps":true}"#, Variant::Core).is_err());
    assert!(listed(br#"{"remoteApps":false}"#, Variant::Core).is_err());
    Ok(())
}

#[test]
fn full_notice_bytes_are_required_and_fingerprinted_for_installer_recovery() -> Result<()> {
    let root = tempfile::tempdir()?;
    let files = fixture_files(Variant::Full);
    for name in &files {
        let path = root.path().join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, name.as_bytes())?;
    }
    fs::write(root.path().join("uninstall.exe"), b"uninstall.exe")?;
    fs::write(
        root.path().join("newport-package.json"),
        fixture_metadata(Variant::Full, &files),
    )?;
    let original = fingerprint(root.path(), Variant::Full)?;
    for name in files.iter().filter(|name| name.starts_with(AGENT_NOTICES)) {
        let path = root.path().join(name);
        let bytes = fs::read(&path)?;
        fs::write(&path, b"Changed Full notice")?;
        assert_ne!(fingerprint(root.path(), Variant::Full)?, original, "{name}");
        fs::remove_file(&path)?;
        assert!(fingerprint(root.path(), Variant::Full).is_err(), "{name}");
        fs::create_dir(&path)?;
        assert!(fingerprint(root.path(), Variant::Full).is_err(), "{name}");
        fs::remove_dir(&path)?;
        fs::write(path, bytes)?;
    }
    assert_eq!(fingerprint(root.path(), Variant::Full)?, original);
    fs::write(
        root.path().join("agents/licenses/unlisted.txt"),
        b"Unexpected notice",
    )?;
    assert!(fingerprint(root.path(), Variant::Full).is_err());
    Ok(())
}
