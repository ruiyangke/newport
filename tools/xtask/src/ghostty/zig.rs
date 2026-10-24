//! The pinned Zig compiler and, on Windows, its patched standard library.
use super::{prepend_path, BuildEnv, ZIG_VERSION};
use crate::{
    command,
    util::{capture, sha256_hex, Arch, Os, Platform},
};
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

/// The release archive platform and its SHA-256. The native Windows ARM64 Zig
/// 0.16 compiler crashes in `zig build` on hosted ARM runners (0xc0000005), so
/// the x64 compiler runs under emulation; Ghostty still receives the native
/// aarch64 target from Cargo.
pub fn archive(os: Os, arch: Arch) -> (&'static str, &'static str) {
    match (os, arch) {
        (Os::Macos, Arch::Aarch64) => (
            "aarch64-macos",
            "b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489",
        ),
        (Os::Macos, Arch::X86_64) => (
            "x86_64-macos",
            "0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7",
        ),
        (Os::Windows, _) => (
            "x86_64-windows",
            "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e",
        ),
        (Os::Linux, Arch::Aarch64) => (
            "aarch64-linux",
            "ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17",
        ),
        (Os::Linux, Arch::X86_64) => (
            "x86_64-linux",
            "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00",
        ),
    }
}

fn version(env: &BuildEnv, binary: &Path) -> Result<String> {
    capture(env.apply(&mut command(binary)).arg("version"))
}

/// The Zig program to run: `zig` from `PATH` when it is the pinned version,
/// else the cached pinned compiler, which is then prepended to `PATH`.
pub fn ensure(cache: &Path, env: &mut BuildEnv) -> Result<PathBuf> {
    let host = Platform::host();
    let windows = host.os == Os::Windows;
    let available = env
        .apply(&mut Command::new("zig"))
        .arg("version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let pinned = matches!(&available, Ok(o) if o.status.success()
        && String::from_utf8_lossy(&o.stdout).trim() == ZIG_VERSION);
    if pinned && !(windows && host.arch == Arch::Aarch64) {
        return Ok("zig".into());
    }
    let (platform, checksum) = archive(host.os, host.arch);
    let name = format!("zig-{platform}-{ZIG_VERSION}");
    let install = cache.join(&name);
    let executable_name = if windows { "zig.exe" } else { "zig" };
    let executable = install.join(executable_name);
    if !executable.exists() {
        let staging = tempfile::Builder::new()
            .prefix("zig-download-")
            .tempdir_in(cache)
            .context("Cannot create the Zig download directory")?;
        let extension = if windows { "zip" } else { "tar.xz" };
        eprintln!("Downloading Zig {ZIG_VERSION} into {}", cache.display());
        let data = download(&format!(
            "https://ziglang.org/download/{ZIG_VERSION}/{name}.{extension}"
        ))?;
        ensure!(
            sha256_hex(&data) == checksum,
            "Zig archive checksum mismatch"
        );
        extract(&data, windows, staging.path())?;
        install_directory(
            &staging.path().join(&name),
            &install,
            executable_name,
            &|binary| version(env, binary),
            windows,
            &mut |from, to| fs::rename(from, to),
            &mut thread::sleep,
        )?;
    }
    ensure!(
        version(env, &executable)? == ZIG_VERSION,
        "Cached Zig version does not match the build pin"
    );
    env.set("PATH", prepend_path(&install, env.get("PATH")));
    Ok(executable)
}

fn download(url: &str) -> Result<Vec<u8>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent.get(url).call().context("Zig download failed")?;
    let status = response.status();
    ensure!(
        status.is_success(),
        "Zig download failed: HTTP {}",
        status.as_u16()
    );
    response
        .body_mut()
        .with_config()
        .limit(1 << 30)
        .read_to_vec()
        .context("Zig download failed")
}

fn extract(data: &[u8], zip: bool, destination: &Path) -> Result<()> {
    if zip {
        zip::ZipArchive::new(io::Cursor::new(data))?.extract(destination)?;
    } else {
        tar::Archive::new(xz2::read::XzDecoder::new_multi_decoder(data)).unpack(destination)?;
    }
    Ok(())
}

/// Move a compiler from the checksum-verified archive into place before running
/// it. Windows can retain an executable handle after `zig version` exits,
/// preventing a directory rename (including x64 emulation on ARM64).
/// A restored cache can keep the library without the executable: require the
/// replacement binary before discarding that partial install, and keep complete
/// installs. Other transient Windows locks are retried for at most five seconds.
pub fn install_directory(
    staged: &Path,
    install: &Path,
    executable: &str,
    version: &dyn Fn(&Path) -> Result<String>,
    windows: bool,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
    wait: &mut dyn FnMut(Duration),
) -> Result<()> {
    ensure!(
        staged.join(executable).is_file(),
        "Downloaded Zig archive has no compiler"
    );
    if install.join(executable).exists() {
        ensure!(
            version(&install.join(executable))? == ZIG_VERSION,
            "Cached Zig version does not match the build pin"
        );
        return Ok(());
    }
    remove_all(install, wait).with_context(|| format!("Cannot remove {}", install.display()))?;
    let mut attempt = 0;
    loop {
        match rename(staged, install) {
            Ok(()) => break,
            Err(error) if windows && attempt < 50 && transient(&error) => {
                attempt += 1;
                wait(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Cannot install Zig from {} to {}",
                        staged.display(),
                        install.display()
                    )
                })
            }
        }
    }
    ensure!(
        version(&install.join(executable))? == ZIG_VERSION,
        "Downloaded Zig version does not match the build pin"
    );
    Ok(())
}

/// EPERM, EACCES and EBUSY, including Windows sharing and lock violations.
fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy
    ) || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)))
}

/// Remove a file or directory tree if present, retrying up to five times.
fn remove_all(path: &Path, wait: &mut dyn FnMut(Duration)) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        let result = match fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => Err(e),
            Ok(m) if m.is_dir() => fs::remove_dir_all(path),
            Ok(_) => fs::remove_file(path),
        };
        match result {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt >= 5 => return Err(e),
            Err(_) => {
                attempt += 1;
                wait(Duration::from_millis(100 * attempt));
            }
        }
    }
}

/// `lib_dir` from `zig env`, which Zig 0.16 prints as ZON with a JSON-compatible
/// quoted string. A relative path (from an inherited `ZIG_LIB_DIR`) is resolved
/// against the repository root, because Cargo runs Zig from the Ghostty source.
pub fn lib_dir(zig_env: &str, root: &Path) -> Result<PathBuf> {
    let lib = zig_env
        .lines()
        .find_map(|line| {
            let quoted = line
                .trim_start()
                .strip_prefix(".lib_dir = ")?
                .strip_suffix(',')?;
            if !quoted.starts_with('"') {
                return None;
            }
            serde_json::from_str::<String>(quoted).ok()
        })
        .context("Cannot locate the pinned Zig standard library")?;
    Ok(root.join(lib))
}

const STDLIB_FILE: &str = "std/debug/SelfInfo/Windows.zig";
const STDLIB_ORIGINAL: &str = "b7a7b689399c854700fa08d6541956e8d9ae72c05f7009d36a438384a22de1fb";
const STDLIB_FIXED: &str = "e28bab3539d3e6b21b260bd78b555fee8af40a989313095efef1255042d1bf51";

/// A private, versioned copy of the Zig standard library whose dynamically
/// loaded ntdll function pointer carries the alignment assertion Windows ARM64
/// needs. The compiler installation is left intact and unknown contents are
/// rejected; an already prepared library is returned unchanged.
pub fn prepare_stdlib(lib: &Path, cache: &Path) -> Result<PathBuf> {
    let source = fs::read(lib.join(STDLIB_FILE))
        .with_context(|| format!("Cannot read {}", lib.join(STDLIB_FILE).display()))?;
    let source_hash = sha256_hex(&source);
    if source_hash == STDLIB_FIXED {
        return Ok(lib.to_path_buf());
    }
    ensure!(
        source_hash == STDLIB_ORIGINAL,
        "Unrecognized Zig 0.16.0 Windows standard library"
    );
    let destination = cache.join(format!("zig-stdlib-0.16.0-{}", &STDLIB_FIXED[..16]));
    let validate = || -> Result<PathBuf> {
        if sha256_hex(&fs::read(destination.join(STDLIB_FILE))?) != STDLIB_FIXED {
            bail!("Prepared Zig standard library was modified");
        }
        Ok(destination.clone())
    };
    if destination.exists() {
        return validate();
    }
    fs::create_dir_all(cache)?;
    let staging = tempfile::Builder::new()
        .prefix("zig-stdlib-")
        .tempdir_in(cache)?;
    let staged = staging.path().join("lib");
    copy_tree(lib, &staged)?;
    let fixed = String::from_utf8_lossy(&source).replacen(
        "@ptrCast(try si.loadNtdllProc(@tagName(proc)))",
        "@ptrCast(@alignCast(try si.loadNtdllProc(@tagName(proc))))",
        1,
    );
    ensure!(
        sha256_hex(fixed.as_bytes()) == STDLIB_FIXED,
        "Zig Windows alignment patch did not match"
    );
    fs::write(staged.join(STDLIB_FILE), fixed)?;
    if let Err(error) = fs::rename(&staged, &destination) {
        if !destination.exists() {
            return Err(error).context("Cannot store the prepared Zig standard library");
        }
    }
    validate()
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry?;
        let target = to.join(entry.path().strip_prefix(from)?);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("Cannot copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn read_version(binary: &Path) -> Result<String> {
        Ok(fs::read_to_string(binary)?)
    }

    fn install(
        staged: &Path,
        install: &Path,
        executable: &str,
        windows: bool,
        rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
        wait: &mut dyn FnMut(Duration),
    ) -> Result<()> {
        install_directory(
            staged,
            install,
            executable,
            &read_version,
            windows,
            rename,
            wait,
        )
    }

    fn no_wait(_: Duration) {
        panic!("unexpected wait");
    }

    #[test]
    fn windows_arm_uses_the_pinned_x64_compiler() {
        assert_eq!(archive(Os::Windows, Arch::Aarch64).0, "x86_64-windows");
        assert_eq!(archive(Os::Windows, Arch::X86_64).0, "x86_64-windows");
        assert_eq!(archive(Os::Macos, Arch::Aarch64).0, "aarch64-macos");
    }

    #[test]
    fn a_partial_restored_cache_is_repaired() {
        for executable in ["zig", "zig.exe"] {
            let temp = tempfile::tempdir().unwrap();
            let staged = temp.path().join("staged");
            let target = temp.path().join("install");
            fs::create_dir_all(target.join("lib")).unwrap();
            fs::write(target.join("lib/leftover"), "partial cache").unwrap();
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join(executable), ZIG_VERSION).unwrap();
            install(
                &staged,
                &target,
                executable,
                false,
                &mut |a, b| fs::rename(a, b),
                &mut no_wait,
            )
            .unwrap();
            assert_eq!(read_version(&target.join(executable)).unwrap(), ZIG_VERSION);
            assert!(!target.join("lib/leftover").exists());
            assert!(!staged.exists());
        }
    }

    #[test]
    fn a_missing_replacement_is_rejected_before_removing_a_partial_cache() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join("staged");
        let target = temp.path().join("install");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("sentinel"), "keep").unwrap();
        fs::create_dir(&staged).unwrap();
        let error = install(
            &staged,
            &target,
            "zig",
            false,
            &mut |a, b| fs::rename(a, b),
            &mut no_wait,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Downloaded Zig archive has no compiler"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(target.join("sentinel")).unwrap(), "keep");
    }

    #[test]
    fn an_incorrect_version_at_the_final_path_is_rejected() {
        for cached in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let staged = temp.path().join("staged");
            let target = temp.path().join("install");
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join("zig.exe"), "wrong version").unwrap();
            if cached {
                fs::create_dir(&target).unwrap();
                fs::write(target.join("zig.exe"), "wrong version").unwrap();
            }
            let error = install(
                &staged,
                &target,
                "zig.exe",
                true,
                &mut |a, b| fs::rename(a, b),
                &mut no_wait,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("version does not match"),
                "{error}"
            );
            assert_eq!(staged.exists(), cached);
        }
    }

    #[test]
    fn an_installation_completed_during_the_download_is_kept() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join("staged");
        let target = temp.path().join("install");
        for directory in [&staged, &target] {
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("zig"), ZIG_VERSION).unwrap();
        }
        fs::write(target.join("sentinel"), "keep").unwrap();
        install(
            &staged,
            &target,
            "zig",
            false,
            &mut |a, b| fs::rename(a, b),
            &mut no_wait,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(target.join("sentinel")).unwrap(), "keep");
        assert!(staged.exists());
    }

    #[test]
    fn installation_waits_for_a_transient_windows_lock() {
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::ResourceBusy] {
            let temp = tempfile::tempdir().unwrap();
            let staged = temp.path().join("staged");
            let target = temp.path().join("install");
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join("zig.exe"), ZIG_VERSION).unwrap();
            let attempts = Cell::new(0);
            let mut delays = Vec::new();
            install(
                &staged,
                &target,
                "zig.exe",
                true,
                &mut |a, b| {
                    attempts.set(attempts.get() + 1);
                    if attempts.get() <= 2 {
                        return Err(io::Error::new(kind, "compiler still in use"));
                    }
                    fs::rename(a, b)
                },
                &mut |delay| delays.push(delay),
            )
            .unwrap();
            assert_eq!(attempts.get(), 3);
            assert_eq!(delays, [Duration::from_millis(100); 2]);
            assert_eq!(read_version(&target.join("zig.exe")).unwrap(), ZIG_VERSION);
            assert!(!staged.exists());
        }
    }

    #[test]
    fn installation_does_not_launch_the_compiler_before_moving_it() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join("staged");
        let target = temp.path().join("install");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("zig.exe"), ZIG_VERSION).unwrap();
        let staged_was_launched = Cell::new(false);
        let probes = Cell::new(0);
        install_directory(
            &staged,
            &target,
            "zig.exe",
            &|binary| {
                probes.set(probes.get() + 1);
                if binary.starts_with(&staged) {
                    staged_was_launched.set(true);
                }
                read_version(binary)
            },
            true,
            &mut |from, to| {
                if staged_was_launched.get() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "the staged compiler still has an executable handle",
                    ));
                }
                fs::rename(from, to)
            },
            &mut |_| {},
        )
        .unwrap();
        assert!(!staged_was_launched.get());
        assert_eq!(probes.get(), 1);
        assert_eq!(read_version(&target.join("zig.exe")).unwrap(), ZIG_VERSION);
        assert!(!staged.exists());
    }

    #[test]
    fn a_failed_installation_keeps_the_staging_directory() {
        for (windows, kind, expected) in [
            (true, io::ErrorKind::PermissionDenied, 51),
            (true, io::ErrorKind::Other, 1),
            (false, io::ErrorKind::PermissionDenied, 1),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let staged = temp.path().join("staged");
            let target = temp.path().join("install");
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join("zig.exe"), ZIG_VERSION).unwrap();
            let mut attempts = 0;
            let mut waited = Duration::ZERO;
            let error = install(
                &staged,
                &target,
                "zig.exe",
                windows,
                &mut |_, _| {
                    attempts += 1;
                    Err(io::Error::new(kind, "cannot move compiler"))
                },
                &mut |delay| waited += delay,
            )
            .unwrap_err();
            let cause = error.downcast_ref::<io::Error>().unwrap();
            assert_eq!(cause.kind(), kind);
            assert_eq!(cause.to_string(), "cannot move compiler");
            assert_eq!(attempts, expected);
            assert_eq!(waited, Duration::from_millis(100) * (expected - 1));
            assert_eq!(read_version(&staged.join("zig.exe")).unwrap(), ZIG_VERSION);
            assert!(!target.exists());
        }
    }

    #[test]
    fn a_relative_zig_lib_dir_survives_cargo_changing_directory() {
        let root = Path::new("/repo");
        let output =
            "  .zig_exe = \"zig\",\n  .lib_dir = \".cache/newport/ghostty/prepared stdlib\",\n";
        let lib = lib_dir(output, root).unwrap();
        assert!(lib.is_absolute());
        assert_eq!(lib, root.join(".cache/newport/ghostty/prepared stdlib"));
        let windows = ".{\r\n    .lib_dir = \"C:\\\\zig\\\\lib\",\r\n}";
        assert!(lib_dir(windows, root).unwrap().ends_with("C:\\zig\\lib"));
        assert!(lib_dir(".lib_dir = unquoted,", root).is_err());
    }

    #[test]
    fn an_unknown_zig_standard_library_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let lib = temp.path().join("lib");
        fs::create_dir_all(lib.join("std/debug/SelfInfo")).unwrap();
        fs::write(lib.join(STDLIB_FILE), "unknown").unwrap();
        let error = prepare_stdlib(&lib, &temp.path().join("cache")).unwrap_err();
        assert!(error.to_string().contains("Unrecognized"), "{error}");
        assert!(!temp.path().join("cache").exists());
    }
}
