//! macOS cargo runner: bundle, sign and launch `newport` through LaunchServices.
//!
//! Restricted Keychain entitlements need a provisioned app bundle, including in
//! development. LaunchServices gives the app its own responsible application:
//! run as a child of an IDE or terminal, 1Password attributes SSH requests to
//! that ancestor and suppresses Newport's foreground approval prompt.
use crate::{
    build_info,
    signing::{self, DevSigning},
    util,
};
use anyhow::{Context, Result};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    thread,
    time::Duration,
};
use walkdir::WalkDir;

/// Flags that print and exit. They run the bundle's binary directly for its
/// stdout and exit status, and never request GUI SSH approval.
pub const PROBES: &[&str] = &[
    "--build-info",
    "--check-backend",
    "--check-update-health",
    "--check-needle",
    "--check-grammars",
    "--check-pdf",
];

/// Variables passed through LaunchServices, which does not inherit the shell's
/// environment. Signing secrets and the launching terminal's identity stay out.
const LAUNCH_ENV: &[&str] = &[
    "NEWPORT_DATA_DIR",
    "NEWPORT_ACCOUNT_URL",
    "SSH_AUTH_SOCK",
    "RUST_LOG",
    "RUST_BACKTRACE",
];

/// How to start a signed development bundle.
#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The application log `open` writes stdout and stderr to.
    pub log: Option<PathBuf>,
}

/// Probes run `Contents/MacOS/newport`; anything else runs through
/// `/usr/bin/open -n -W` with an allowlisted environment, in the background for `--autostart`.
pub fn development_launch(
    bundle: &Path,
    args: &[OsString],
    var: impl Fn(&str) -> Option<OsString>,
) -> Result<Launch> {
    let bundle = std::path::absolute(bundle)?;
    if args.iter().any(|a| PROBES.iter().any(|p| a == p)) {
        return Ok(Launch {
            program: bundle.join("Contents/MacOS/newport"),
            args: args.to_vec(),
            log: None,
        });
    }
    let log = bundle
        .parent()
        .context("The bundle has no parent directory")?
        .join("newport-dev.log");
    let mut launch: Vec<OsString> = vec!["-n".into(), "-W".into()];
    for stream in ["--stdout", "--stderr"] {
        launch.extend([stream.into(), log.clone().into()]);
    }
    for name in LAUNCH_ENV {
        if let Some(mut value) = var(name) {
            if *name == "NEWPORT_DATA_DIR" && !value.is_empty() {
                value = std::path::absolute(&value)?.into();
            }
            let mut assignment = OsString::from(format!("{name}="));
            assignment.push(value);
            launch.extend(["--env".into(), assignment]);
        }
    }
    if args.iter().any(|a| a == "--autostart") {
        launch.push("-g".into());
    }
    launch.extend([bundle.into_os_string(), "--args".into()]);
    launch.extend(args.iter().cloned());
    Ok(Launch {
        program: "/usr/bin/open".into(),
        args: launch,
        log: Some(log),
    })
}

pub fn run(binary: &Path, args: &[OsString]) -> Result<()> {
    let direct = || {
        let mut c = Command::new(binary);
        c.args(args);
        c
    };
    if binary.file_name() != Some(OsStr::new("newport")) {
        return replace(&mut direct());
    }
    let signing = if cfg!(target_os = "macos") {
        DevSigning::load()?
    } else {
        None
    };
    let Some(signing) = signing else {
        if cfg!(target_os = "macos") {
            eprintln!(
                "Running newport unsigned; Keychain access needs NEWPORT_SIGNING_IDENTITY, \
                 NEWPORT_PROVISIONING_PROFILE and NEWPORT_SIGNING_ENTITLEMENTS or .cargo/signing.toml"
            );
        }
        return replace(util::without_release_secrets(&mut direct()));
    };
    let bundle = assemble(binary, &signing)?;
    let launch = development_launch(&bundle, args, |name| env::var_os(name))?;
    let mut command = Command::new(&launch.program);
    util::without_release_secrets(command.args(&launch.args));
    match &launch.log {
        Some(log) => {
            let status = follow(&mut command, log, &mut io::stderr())?;
            std::process::exit(status.code().unwrap_or(1))
        }
        None => replace(&mut command),
    }
}

/// Run `open -W` and copy the application log to `out` as it grows, so the
/// terminal shows the app's output like a direct `cargo run`.
fn follow(c: &mut Command, log: &Path, out: &mut dyn Write) -> Result<ExitStatus> {
    create_log(log)?;
    let mut file = fs::File::open(log).with_context(|| format!("Cannot open {}", log.display()))?;
    let mut child = c
        .spawn()
        .with_context(|| format!("Cannot start {}", c.get_program().to_string_lossy()))?;
    let mut buffer = [0; 8192];
    loop {
        // Check first, then drain, so output written just before exit is still shown.
        let status = child.try_wait()?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            out.write_all(&buffer[..read])?;
        }
        if let Some(status) = status {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Replace this process with `c`, so its exit status becomes ours.
fn replace(c: &mut Command) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = c.exec();
        Err(error).with_context(|| format!("Cannot start {}", c.get_program().to_string_lossy()))
    }
    #[cfg(not(unix))]
    {
        let status = c
            .status()
            .with_context(|| format!("Cannot start {}", c.get_program().to_string_lossy()))?;
        std::process::exit(status.code().unwrap_or(1))
    }
}

/// An empty, owner-only log file.
fn create_log(path: &Path) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .with_context(|| format!("Cannot create {}", path.display()))?;
    Ok(())
}

/// Copy over a fresh file: rewriting a signed Mach-O in place invalidates the
/// kernel's cached signature for it.
fn copy(from: &Path, to: &Path) -> Result<()> {
    if to.symlink_metadata().is_ok() {
        fs::remove_file(to).with_context(|| format!("Cannot replace {}", to.display()))?;
    }
    fs::copy(from, to)
        .with_context(|| format!("Cannot copy {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in WalkDir::new(from) {
        let entry = entry?;
        let target = to.join(entry.path().strip_prefix(from)?);
        let kind = entry.file_type();
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &target)?;
            #[cfg(not(unix))]
            fs::copy(entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Build and sign `Newport.app` beside the binary with its agents, grammars,
/// provisioning profile, icon and fonts.
fn assemble(binary: &Path, signing: &DevSigning) -> Result<PathBuf> {
    let root = util::root();
    signing::check(
        &signing.entitlements,
        &signing.provisioning_profile,
        &util::app()?.identifier,
    )?;
    let info = build_info::read(
        binary,
        build_info::Expect {
            os: Some("macos"),
            ..Default::default()
        },
    )?;
    let info_plist = build_info::mac_info_plist(&info);
    let directory = binary
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", binary.display()))?;
    let bundle = directory.join("Newport.app");
    let contents = bundle.join("Contents");
    let resources = contents.join("Resources");
    let agents: Vec<PathBuf> = ["x86_64", "aarch64"]
        .iter()
        .map(|arch| {
            directory
                .join("agents")
                .join(format!("newport-agent-{arch}.gz"))
        })
        .collect();
    for agent in &agents {
        anyhow::ensure!(
            agent.is_file(),
            "Missing {}; build newport-app with the agent binaries in resources/agents",
            agent.display()
        );
    }
    let grammars = directory.join("grammars");
    anyhow::ensure!(
        grammars.is_dir(),
        "Missing {}; run `cargo xtask grammars` first",
        grammars.display()
    );
    let icon = root.join("resources/icons/icon.icns");
    let font = root.join("resources/fonts/SymbolsNerdFontMono-Regular.ttf");
    let font_license = root.join("resources/fonts/NERD-FONTS-LICENSE");
    let mut inputs = vec![
        binary.to_owned(),
        signing.provisioning_profile.clone(),
        signing.entitlements.clone(),
        icon.clone(),
        font.clone(),
        font_license.clone(),
    ];
    inputs.extend(agents.iter().cloned());
    for entry in WalkDir::new(&grammars).sort_by_file_name() {
        inputs.push(entry?.into_path());
    }
    // Copying the debug binary and re-signing every library takes seconds, and a
    // newly signed app is assessed again on first launch; reuse an unchanged bundle.
    let stamp = directory.join("Newport.app.inputs");
    let current = inputs_stamp(&signing.identity, &info_plist, &inputs)?;
    if fs::read_to_string(&stamp).ok().as_deref() == Some(current.as_str())
        && signing::verify(&bundle).is_ok()
    {
        return Ok(bundle);
    }
    let _ = fs::remove_file(&stamp);
    for dir in [
        contents.join("MacOS"),
        resources.join("Fonts"),
        resources.join("agents"),
    ] {
        fs::create_dir_all(&dir).with_context(|| format!("Cannot create {}", dir.display()))?;
    }
    copy(binary, &contents.join("MacOS/newport"))?;
    for agent in &agents {
        copy(
            agent,
            &resources.join("agents").join(agent.file_name().unwrap()),
        )?;
    }
    let staged = resources.join("grammars");
    if staged.symlink_metadata().is_ok() {
        fs::remove_dir_all(&staged)
            .with_context(|| format!("Cannot replace {}", staged.display()))?;
    }
    copy_tree(&grammars, &staged)?;
    let mut libraries: Vec<PathBuf> = fs::read_dir(&staged)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    libraries.retain(|p| p.extension() == Some(OsStr::new("dylib")));
    libraries.sort();
    for library in &libraries {
        signing::codesign(library, &signing.identity, &["--timestamp=none"])?;
    }
    copy(
        &signing.provisioning_profile,
        &contents.join("embedded.provisionprofile"),
    )?;
    copy(&icon, &resources.join("icon.icns"))?;
    copy(
        &font,
        &resources.join("Fonts/SymbolsNerdFontMono-Regular.ttf"),
    )?;
    copy(&font_license, &resources.join("NERD-FONTS-LICENSE"))?;
    fs::write(contents.join("Info.plist"), format!("{info_plist}\n"))?;
    let entitlements = signing
        .entitlements
        .to_str()
        .context("The entitlements path is not valid UTF-8")?;
    signing::codesign(
        &bundle,
        &signing.identity,
        &["--entitlements", entitlements, "--timestamp=none"],
    )?;
    signing::verify(&bundle)?;
    fs::write(&stamp, current).with_context(|| format!("Cannot write {}", stamp.display()))?;
    Ok(bundle)
}

/// The identity, Info.plist, and each input's path, size and modification time.
fn inputs_stamp(identity: &str, info_plist: &str, inputs: &[PathBuf]) -> Result<String> {
    let mut stamp = format!("{identity}\n{info_plist}\n");
    for input in inputs {
        let metadata =
            fs::metadata(input).with_context(|| format!("Cannot read {}", input.display()))?;
        let modified = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        stamp.push_str(&format!(
            "{} {} {}\n",
            input.display(),
            metadata.len(),
            modified.as_nanos()
        ));
    }
    Ok(stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn follow_streams_the_log_until_the_launcher_exits() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("newport-dev.log");
        let script = format!(
            "printf 'first\\n' >> '{0}'; sleep 0.3; printf 'last\\n' >> '{0}'; exit 3",
            log.display()
        );
        let mut out = Vec::new();
        let status = follow(Command::new("sh").args(["-c", &script]), &log, &mut out).unwrap();
        assert_eq!(status.code(), Some(3));
        assert_eq!(String::from_utf8(out).unwrap(), "first\nlast\n");
    }
    use std::collections::HashMap;

    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn interactive_dev_launches_have_an_independent_app_identity_and_retain_their_profile() {
        let bundle = std::path::absolute("target/test space/Newport.app").unwrap();
        let launch = development_launch(
            &bundle,
            &os(&["--setup-preview"]),
            vars(&[
                ("NEWPORT_DATA_DIR", "test profile & data"),
                ("SSH_AUTH_SOCK", "/tmp/agent socket"),
                ("RUST_BACKTRACE", "1"),
                ("TERM_PROGRAM", "Launching IDE"),
                ("NEWPORT_SIGNING_PRIVATE_KEY", "must not propagate"),
            ]),
        )
        .unwrap();
        let log = launch.log.clone().unwrap();
        assert_eq!(log, bundle.parent().unwrap().join("newport-dev.log"));
        assert_eq!(launch.program, Path::new("/usr/bin/open"));
        let data = std::path::absolute("test profile & data").unwrap();
        let mut expected = os(&["-n", "-W", "--stdout"]);
        expected.extend([log.clone().into(), "--stderr".into(), log.into()]);
        expected.extend(os(&[
            "--env",
            &format!("NEWPORT_DATA_DIR={}", data.display()),
            "--env",
            "SSH_AUTH_SOCK=/tmp/agent socket",
            "--env",
            "RUST_BACKTRACE=1",
        ]));
        expected.push(bundle.into());
        expected.extend(os(&["--args", "--setup-preview"]));
        assert_eq!(launch.args, expected);
    }

    #[test]
    fn health_probes_retain_direct_execution_output_and_process_exit_status() {
        for probe in PROBES {
            let launch =
                development_launch(Path::new("target/Newport.app"), &os(&[probe]), vars(&[]))
                    .unwrap();
            assert_eq!(
                launch,
                Launch {
                    program: std::path::absolute("target/Newport.app/Contents/MacOS/newport")
                        .unwrap(),
                    args: os(&[probe]),
                    log: None,
                }
            );
        }
    }

    #[test]
    fn login_launch_remains_background_and_empty_profile_overrides_remain_invalid() {
        let launch = development_launch(
            Path::new("target/Newport.app"),
            &os(&["--autostart"]),
            vars(&[("NEWPORT_DATA_DIR", "")]),
        )
        .unwrap();
        assert!(launch.args.contains(&"-g".into()));
        assert!(launch.args.contains(&"NEWPORT_DATA_DIR=".into()));
        assert_eq!(
            launch.args[launch.args.len() - 2..],
            os(&["--args", "--autostart"])
        );
    }

    #[test]
    fn trees_copy_with_nested_files() {
        let from = tempfile::tempdir().unwrap();
        fs::create_dir(from.path().join("queries")).unwrap();
        fs::write(from.path().join("a.dylib"), "a").unwrap();
        fs::write(from.path().join("queries/b.scm"), "b").unwrap();
        let to = tempfile::tempdir().unwrap();
        let target = to.path().join("grammars");
        copy_tree(from.path(), &target).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("queries/b.scm")).unwrap(),
            "b"
        );
        copy(&from.path().join("a.dylib"), &target.join("queries/b.scm")).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("queries/b.scm")).unwrap(),
            "a"
        );
    }
}
