//! Opt-in runtime acceptance using a real, locally built NSIS candidate.
//! Never point this at an existing installation or run with production keys.
#![cfg(windows)]

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read},
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt},
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
const PAYLOAD: &[&str] = &[
    "newport.exe",
    "grammars/catalog.json",
    "grammars/newport_grammar_astro.dll",
    "grammars/newport_grammar_bash.dll",
    "grammars/newport_grammar_c.dll",
    "grammars/newport_grammar_cmake.dll",
    "grammars/newport_grammar_cpp.dll",
    "grammars/newport_grammar_csharp.dll",
    "grammars/newport_grammar_css.dll",
    "grammars/newport_grammar_diff.dll",
    "grammars/newport_grammar_ejs.dll",
    "grammars/newport_grammar_elixir.dll",
    "grammars/newport_grammar_erb.dll",
    "grammars/newport_grammar_go.dll",
    "grammars/newport_grammar_graphql.dll",
    "grammars/newport_grammar_html.dll",
    "grammars/newport_grammar_java.dll",
    "grammars/newport_grammar_javascript.dll",
    "grammars/newport_grammar_jsdoc.dll",
    "grammars/newport_grammar_json.dll",
    "grammars/newport_grammar_kotlin.dll",
    "grammars/newport_grammar_lua.dll",
    "grammars/newport_grammar_make.dll",
    "grammars/newport_grammar_markdown.dll",
    "grammars/newport_grammar_markdown_inline.dll",
    "grammars/newport_grammar_php.dll",
    "grammars/newport_grammar_proto.dll",
    "grammars/newport_grammar_python.dll",
    "grammars/newport_grammar_ruby.dll",
    "grammars/newport_grammar_rust.dll",
    "grammars/newport_grammar_scala.dll",
    "grammars/newport_grammar_sql.dll",
    "grammars/newport_grammar_svelte.dll",
    "grammars/newport_grammar_swift.dll",
    "grammars/newport_grammar_toml.dll",
    "grammars/newport_grammar_tsx.dll",
    "grammars/newport_grammar_typescript.dll",
    "grammars/newport_grammar_yaml.dll",
    "grammars/newport_grammar_zig.dll",
    "agents/newport-agent-x86_64.gz",
    "agents/newport-agent-aarch64.gz",
    "pdfium/pdfium.dll",
    "pdfium/LICENSE-PDFIUM",
    "Fonts/SymbolsNerdFontMono-Regular.ttf",
    "Fonts/NERD-FONTS-LICENSE",
    "Fonts/README.md",
    "LICENSE-GPUI-KIT",
    "Licenses/terminal/LICENSE-Alacritty",
    "Licenses/terminal/LICENSE-VTE",
    "Licenses/terminal/LICENSE-unicode-segmentation",
    "Licenses/terminal/LICENSE-unicode-width",
    "Licenses/terminal/README.md",
    "icon.ico",
    "newport-package.json",
    "uninstall.exe",
];

fn payload() -> Result<Vec<String>> {
    let mut files: Vec<String> = PAYLOAD.iter().map(|name| (*name).to_owned()).collect();
    let icons: Vec<String> =
        serde_json::from_str(include_str!("../../../resources/licenses/icons/FILES.json"))?;
    files.extend([
        "Licenses/icons/FILES.json".into(),
        "Licenses/icons/README.md".into(),
        "Licenses/grammars/SOURCES.json".into(),
        "Licenses/grammars/README.md".into(),
    ]);
    files.extend(icons.iter().map(|path| format!("Licenses/icons/{path}")));
    let grammars: serde_json::Value = serde_json::from_str(include_str!(
        "../../../resources/licenses/grammars/SOURCES.json"
    ))?;
    for package in grammars["packages"]
        .as_array()
        .context("Grammar packages")?
    {
        let name = package["name"].as_str().context("Grammar name")?;
        for notice in package["notices"].as_array().context("Grammar notices")? {
            let file = notice["file"].as_str().context("Grammar notice file")?;
            files.push(format!("Licenses/grammars/{name}/{file}"));
        }
    }
    Ok(files)
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

fn fingerprint(destination: &Path) -> Result<BTreeMap<String, [u8; 32]>> {
    payload()?
        .into_iter()
        .map(|name| {
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

fn exercise(installer: &Path, destination: &Path) -> Result<()> {
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
    let original = fingerprint(destination)?;
    ensure!(
        wait(Command::new(destination.join("newport.exe")).arg("--check-pdf"))?.success(),
        "Installed PDF renderer failed"
    );
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
            fingerprint(destination)? == original,
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
        fingerprint(destination)? == original,
        "Recovery changed payload bytes"
    );
    ensure!(
        wait(Command::new(destination.join("newport.exe")).arg("--check-grammars"))?.success(),
        "Upgraded grammars failed"
    );
    ensure!(
        wait(Command::new(destination.join("uninstall.exe")).arg("/S"))?.success(),
        "Uninstaller failed"
    );
    // NSIS relaunches the uninstaller outside $INSTDIR. Wait for that child too.
    let deadline = Instant::now() + Duration::from_secs(30);
    let files = payload()?;
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
        "grammars",
        "pdfium",
        "Licenses",
        ".newport-install-stage",
        ".newport-install-backup",
    ] {
        ensure!(!destination.join(name).exists(), "Uninstall left {name}");
    }
    Ok(())
}

#[test]
#[ignore = "executes NEWPORT_TEST_INSTALLER; requires an account with no Newport installation"]
fn native_installer_preserves_profiles_and_recovers_failed_replacements() -> Result<()> {
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
    exercise(&installer, &destination).with_context(|| {
        format!(
            "Installer test failed; files retained at {}",
            root.display()
        )
    })?;
    fs::remove_dir_all(root)?;
    Ok(())
}
