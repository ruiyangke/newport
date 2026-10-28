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
    "--agent-identity",
    "--check-grammars",
    "--check-pdf",
];

/// Variables passed through LaunchServices, which does not inherit the shell's
/// environment. Signing secrets and the launching terminal's identity stay out.
const LAUNCH_ENV: &[&str] = &[
    "NEWPORT_DATA_DIR",
    "NEWPORT_ACCOUNT_URL",
    "SSH_AUTH_SOCK",
    "RUST_BACKTRACE",
];

const ISOLATED_IDENTIFIER: &str = "app.newport.dev";
const ISOLATED_NAME: &str = "Newport Dev";

/// Resolve absent leaf paths through their nearest existing ancestor, so a
/// symlinked parent cannot disguise a production profile or application path.
fn resolved_location(path: &Path) -> Result<PathBuf> {
    anyhow::ensure!(path.is_absolute(), "Expected an absolute isolation path");
    anyhow::ensure!(
        !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir)),
        "Isolation paths must not contain '..'"
    );
    let mut existing = path;
    let mut tail = Vec::new();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                tail.push(existing.file_name().context("Invalid isolation path")?);
                existing = existing.parent().context("Invalid isolation path")?;
            }
            Err(error) => return Err(error).context("Cannot inspect isolation path"),
        }
    }
    let mut resolved = existing
        .canonicalize()
        .context("Cannot resolve isolation path")?;
    for component in tail.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn overlaps(path: &Path, other: &Path) -> bool {
    path.starts_with(other) || other.starts_with(path)
}

/// This initial qualification mode deliberately cannot reopen a populated
/// profile. A future reuse policy must authenticate its isolated ownership;
/// guessing from an existing directory or erasing it is not safe.
fn isolated_profile(
    binary: &Path,
    profile: Option<&OsStr>,
    home: &Path,
    old_dev: &Path,
) -> Result<PathBuf> {
    let profile = profile.filter(|value| !value.is_empty()).context(
        "--isolated-bundle requires an explicit absolute NEWPORT_DATA_DIR naming a fresh empty directory; existing profiles are never erased or migrated",
    )?;
    let profile = Path::new(profile);
    anyhow::ensure!(
        profile.is_absolute(),
        "The isolated profile must be an absolute path"
    );
    let metadata = fs::symlink_metadata(profile)
        .context("Create a fresh empty isolated profile directory first")?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "The isolated profile must be a real directory, not a symlink"
    );
    let profile = resolved_location(profile)?;
    let executable =
        fs::symlink_metadata(binary).context("Cannot inspect the development executable")?;
    anyhow::ensure!(
        executable.is_file() && !executable.file_type().is_symlink(),
        "The isolated executable must be a real file, not a symlink"
    );
    let directory = binary
        .canonicalize()
        .context("Cannot resolve the development executable")?
        .parent()
        .context("The development executable has no directory")?
        .to_owned();
    let applications = [PathBuf::from("/Applications"), home.join("Applications")];
    for path in applications {
        let path = resolved_location(&path)?;
        anyhow::ensure!(
            !directory.starts_with(&path) && !profile.starts_with(&path),
            "Isolated bundles and profiles must stay outside application installation directories"
        );
    }
    anyhow::ensure!(
        !directory.ancestors().any(|part| part
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))),
        "The isolated bundle destination must not be inside an existing application bundle"
    );
    for forbidden in [
        home.join("Library/Application Support/Newport"),
        home.join("Library/Application Support/app.newport"),
        old_dev.to_owned(),
    ] {
        let forbidden = resolved_location(&forbidden)?;
        anyhow::ensure!(
            !overlaps(&profile, &forbidden),
            "The isolated profile must be separate from production, legacy and existing development profiles"
        );
        anyhow::ensure!(
            !directory.starts_with(&forbidden),
            "The isolated bundle must not be staged inside a production or existing profile"
        );
    }
    anyhow::ensure!(
        !overlaps(&profile, &directory.join("Newport Dev.app")),
        "The isolated profile and bundle must be separate"
    );
    let bundle = directory.join("Newport Dev.app");
    match fs::symlink_metadata(&bundle) {
        Ok(meta) => anyhow::ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "The isolated bundle destination must not be a symlink or file"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error).context("Cannot inspect the isolated bundle destination"),
    }
    if bundle.exists() {
        let plist = util::read_plist(&bundle.join("Contents/Info.plist"))?;
        anyhow::ensure!(
            plist
                .as_dictionary()
                .and_then(|value| value.get("CFBundleIdentifier"))
                .and_then(plist::Value::as_string)
                == Some(ISOLATED_IDENTIFIER),
            "Refusing to replace a bundle without the exact development identity"
        );
    }
    anyhow::ensure!(
        fs::read_dir(&profile)?.next().is_none(),
        "The isolated profile must be fresh and empty; populated profiles cannot be reused yet and are never erased or migrated"
    );
    Ok(profile)
}

fn isolated_entitlements(entitlements: &plist::Dictionary) -> Result<()> {
    let id = entitlements
        .get("com.apple.application-identifier")
        .or_else(|| entitlements.get("application-identifier"))
        .and_then(plist::Value::as_string)
        .context("Development entitlements need an application identifier")?;
    anyhow::ensure!(
        id.strip_suffix(".app.newport.dev")
            .is_some_and(|team| !team.is_empty() && !team.contains('*')),
        "--isolated-bundle requires separately prepared app.newport.dev entitlements; production entitlements are not rewritten"
    );
    anyhow::ensure!(
        entitlements
            .get("application-identifier")
            .is_none_or(|value| value.as_string() == Some(id)),
        "Conflicting application identifiers in development entitlements"
    );
    let groups = entitlements
        .get("keychain-access-groups")
        .and_then(plist::Value::as_array);
    anyhow::ensure!(
        groups.is_some_and(|groups| groups.len() == 1 && groups[0].as_string() == Some(id)),
        "The isolated bundle must request exactly its own development Keychain group, with no production or legacy groups"
    );
    Ok(())
}

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
    run_with_identity(binary, args, false)
}

pub fn run_isolated(binary: &Path, args: &[OsString]) -> Result<()> {
    run_with_identity(binary, args, true)
}

fn run_with_identity(binary: &Path, args: &[OsString], isolated: bool) -> Result<()> {
    anyhow::ensure!(
        !isolated || cfg!(target_os = "macos"),
        "--isolated-bundle is only supported on macOS"
    );
    anyhow::ensure!(
        !isolated || binary.file_name() == Some(OsStr::new("newport")),
        "--isolated-bundle requires the Newport desktop executable"
    );
    let profile = if isolated {
        let home =
            env::var_os("HOME").context("HOME is required to check development isolation")?;
        Some(isolated_profile(
            binary,
            env::var_os("NEWPORT_DATA_DIR").as_deref(),
            Path::new(&home),
            &util::root().join("target/dev-profile"),
        )?)
    } else {
        None
    };
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
        anyhow::ensure!(
            !isolated,
            "The isolated bundle requires configured signing identity, profile and development entitlements; unsigned fallback is forbidden"
        );
        if cfg!(target_os = "macos") {
            eprintln!(
                "Running newport unsigned; Keychain access needs NEWPORT_SIGNING_IDENTITY, \
                 NEWPORT_PROVISIONING_PROFILE and NEWPORT_SIGNING_ENTITLEMENTS or .cargo/signing.toml"
            );
        }
        return replace(util::without_release_secrets(&mut direct()));
    };
    let probe = args.iter().any(|a| PROBES.iter().any(|p| a == p));
    if !probe {
        // Core/backend tests can replace the shared target/<profile>/agents
        // after a full desktop build. Refuse the mismatch before stopping the
        // working app or opening a development instance with unusable payloads.
        validate_agents(binary)?;
        // Assembling replaces the bundle a running instance executes from.
        stop_running(&bundle_path(binary, isolated)?)?;
    }
    let bundle = assemble(binary, &signing, isolated)?;
    let launch = development_launch(&bundle, args, |name| {
        if name == "NEWPORT_DATA_DIR" && profile.is_some() {
            profile.clone().map(PathBuf::into_os_string)
        } else {
            env::var_os(name)
        }
    })?;
    if launch.log.is_some() {
        validate_agents(&bundle.join("Contents/MacOS/newport"))?;
    }
    let mut command = Command::new(&launch.program);
    util::without_release_secrets(command.args(&launch.args));
    if let Some(profile) = profile {
        command.env("NEWPORT_DATA_DIR", profile);
    }
    match &launch.log {
        Some(log) => {
            let status = follow(&mut command, log, &mut io::stderr())?;
            std::process::exit(status.code().unwrap_or(1))
        }
        None => replace(&mut command),
    }
}

fn validate_agents(executable: &Path) -> Result<()> {
    util::run(util::without_release_secrets(
        Command::new(executable).arg("--check-needle"),
    ))
    .context("Development agent resources differ from this desktop build; rebuild the desktop with the intended agent resources")
}

/// Whether a process command line runs an executable inside `bundle`, the
/// development bundle this run assembles. An installed Newport, such as the
/// one in /Applications, another profile's or another checkout's bundle never
/// matches.
fn runs_bundle(command: &str, bundle: &Path) -> bool {
    Path::new(command)
        .strip_prefix(bundle)
        .ok()
        .and_then(|rest| rest.to_str())
        .is_some_and(|rest| rest.starts_with("Contents/MacOS/"))
}

/// Process ids of Newports running from `bundle`.
fn running_bundle(bundle: &Path) -> Result<Vec<u32>> {
    let output = Command::new("ps")
        .args(["-axww", "-o", "pid=,command="])
        .output()
        .context("Cannot list processes")?;
    let own = std::process::id();
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, command) = line.trim_start().split_once(' ')?;
            let pid: u32 = pid.parse().ok()?;
            (pid != own && runs_bundle(command.trim_start(), bundle)).then_some(pid)
        })
        .collect())
}

/// Quits a development Newport still running from `bundle` and waits for it
/// to exit. Otherwise assembling would replace the bundle under it, and
/// LaunchServices only brings the old window forward, or refuses to start the
/// new one while the old one is still shutting down.
fn stop_running(bundle: &Path) -> Result<()> {
    let running = running_bundle(bundle)?;
    if running.is_empty() {
        return Ok(());
    }
    eprintln!("Quitting the development Newport that is running");
    for pid in &running {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
    for _ in 0..600 {
        if running_bundle(bundle)?.is_empty() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("The running development Newport did not quit within a minute")
}

/// The bundle `assemble` writes beside `binary`.
fn bundle_path(binary: &Path, isolated: bool) -> Result<PathBuf> {
    let directory = binary
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", binary.display()))?;
    Ok(directory.join(if isolated {
        "Newport Dev.app"
    } else {
        "Newport.app"
    }))
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
fn assemble(binary: &Path, signing: &DevSigning, isolated: bool) -> Result<PathBuf> {
    let root = util::root();
    let product = util::app()?;
    let identifier = if isolated {
        ISOLATED_IDENTIFIER
    } else {
        &product.identifier
    };
    if isolated {
        let requested = util::read_plist(&signing.entitlements)?;
        isolated_entitlements(
            requested
                .as_dictionary()
                .context("Signing entitlements must be a dictionary")?,
        )?;
    }
    signing::check(
        &signing.entitlements,
        &signing.provisioning_profile,
        identifier,
    )?;
    let info = build_info::read(
        binary,
        build_info::Expect {
            os: Some("macos"),
            ..Default::default()
        },
    )?;
    let info_plist = if isolated {
        build_info::mac_info_plist_for_bundle(&info, ISOLATED_IDENTIFIER, ISOLATED_NAME)
    } else {
        build_info::mac_info_plist(&info)
    };
    let bundle = bundle_path(binary, isolated)?;
    let directory = bundle.parent().unwrap().to_owned();
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
    let agent_notices = directory.join("agents/licenses");
    if agent_notices.is_dir() {
        for entry in WalkDir::new(&agent_notices).sort_by_file_name() {
            inputs.push(entry?.into_path());
        }
    }
    for entry in WalkDir::new(&grammars).sort_by_file_name() {
        inputs.push(entry?.into_path());
    }
    // Copying the debug binary and re-signing every library takes seconds, and a
    // newly signed app is assessed again on first launch; reuse an unchanged bundle.
    let stamp = directory.join(if isolated {
        "Newport Dev.app.inputs"
    } else {
        "Newport.app.inputs"
    });
    let current = inputs_stamp(&signing.identity, &info_plist, &inputs)?;
    if fs::read_to_string(&stamp).ok().as_deref() == Some(current.as_str())
        && signing::verify(&bundle).is_ok()
    {
        return Ok(bundle);
    }
    // Never mutate an existing isolated bundle with mismatched inputs, including
    // its contents or links. Use a fresh staging directory for a changed build.
    anyhow::ensure!(
        !isolated || !bundle.exists(),
        "The existing isolated bundle is not an unchanged verified build; use a fresh staging directory (nothing was replaced)"
    );
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
    let staged_notices = resources.join("agents/licenses");
    if staged_notices.symlink_metadata().is_ok() {
        fs::remove_dir_all(&staged_notices)
            .with_context(|| format!("Cannot replace {}", staged_notices.display()))?;
    }
    if agent_notices.is_dir() {
        copy_tree(&agent_notices, &staged_notices)?;
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

    fn development_entitlements(id: &str, groups: &[&str]) -> plist::Dictionary {
        let mut result = plist::Dictionary::new();
        result.insert("com.apple.application-identifier".into(), id.into());
        result.insert(
            "keychain-access-groups".into(),
            plist::Value::Array(
                groups
                    .iter()
                    .map(|group| plist::Value::String((*group).into()))
                    .collect(),
            ),
        );
        result
    }

    #[test]
    fn isolated_entitlements_require_only_the_exact_development_group_and_profile_authority() {
        let good = development_entitlements("TEAM.app.newport.dev", &["TEAM.app.newport.dev"]);
        let wildcard = development_entitlements("TEAM.*", &["TEAM.*"]);
        isolated_entitlements(&good).unwrap();
        signing::validate(&good, &wildcard, ISOLATED_IDENTIFIER).unwrap();
        for bad in [
            development_entitlements("TEAM.app.newport", &["TEAM.app.newport"]),
            development_entitlements("TEAM.app.newport.dev", &["TEAM.app.newport"]),
            development_entitlements(
                "TEAM.app.newport.dev",
                &["TEAM.app.newport.dev", "TEAM.app.newport"],
            ),
            development_entitlements(
                "TEAM.app.newport.dev",
                &["TEAM.app.newport.dev", "TEAM.com.porthop.desktop"],
            ),
            development_entitlements("TEAM.app.newport.dev", &["TEAM.*"]),
            development_entitlements("TEAM.app.newport.dev", &[]),
            development_entitlements("*.app.newport.dev", &["*.app.newport.dev"]),
        ] {
            assert!(isolated_entitlements(&bad).is_err());
        }
        let mut conflict = good.clone();
        conflict.insert("application-identifier".into(), "TEAM.app.newport".into());
        assert!(isolated_entitlements(&conflict).is_err());
        let production_profile =
            development_entitlements("TEAM.app.newport", &["TEAM.app.newport"]);
        assert!(signing::validate(&good, &production_profile, ISOLATED_IDENTIFIER).is_err());
        assert!(signing::validate(&good, &wildcard, "app.newport").is_err());
    }

    #[test]
    fn isolated_plist_and_stamp_keep_product_identity_separate_and_do_not_reuse_production_inputs()
    {
        let product = build_info::BuildInfo {
            schema: 1,
            product_name: "Newport".into(),
            identifier: "app.newport".into(),
            version: "0.5.0".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            debug_assertions: true,
        };
        let production = build_info::mac_info_plist(&product);
        let isolated =
            build_info::mac_info_plist_for_bundle(&product, ISOLATED_IDENTIFIER, ISOLATED_NAME);
        let parse = |text: &str| {
            plist::Value::from_reader(io::Cursor::new(text))
                .unwrap()
                .into_dictionary()
                .unwrap()
        };
        assert_eq!(
            parse(&production)["CFBundleIdentifier"].as_string(),
            Some("app.newport")
        );
        assert_eq!(
            parse(&isolated)["CFBundleIdentifier"].as_string(),
            Some(ISOLATED_IDENTIFIER)
        );
        assert_eq!(
            parse(&isolated)["CFBundleName"].as_string(),
            Some(ISOLATED_NAME)
        );
        assert_eq!(product.identifier, "app.newport");
        assert_eq!(product.product_name, "Newport");
        let original = inputs_stamp("fixture identity", &production, &[]).unwrap();
        let different = inputs_stamp("fixture identity", &isolated, &[]).unwrap();
        assert_ne!(original, different);
        assert_eq!(
            different,
            inputs_stamp("fixture identity", &isolated, &[]).unwrap()
        );
        assert_ne!(
            different,
            inputs_stamp("different identity", &isolated, &[]).unwrap()
        );
    }

    #[test]
    fn isolated_profile_checks_require_empty_separate_paths_and_never_erase_existing_profiles() {
        let owned = tempfile::tempdir().unwrap();
        let owned = owned.path().canonicalize().unwrap();
        let home = owned.join("home");
        let old_dev = owned.join("old-development");
        let stage = owned.join("candidate");
        let fresh = owned.join("fresh");
        let production = home.join("Library/Application Support/Newport");
        let legacy = home.join("Library/Application Support/app.newport");
        for path in [&home, &old_dev, &stage, &fresh, &production, &legacy] {
            fs::create_dir_all(path).unwrap();
        }
        let binary = stage.join("newport");
        fs::write(&binary, b"source-only fixture").unwrap();
        for path in [&old_dev, &production, &legacy] {
            fs::write(path.join("keep"), b"untouched fixture").unwrap();
        }
        let check = |profile: Option<&OsStr>| isolated_profile(&binary, profile, &home, &old_dev);
        assert_eq!(check(Some(fresh.as_os_str())).unwrap(), fresh);
        for profile in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("relative")),
            Some(production.as_os_str()),
            Some(legacy.as_os_str()),
            Some(old_dev.as_os_str()),
            Some(home.as_os_str()),
        ] {
            assert!(check(profile).is_err());
        }
        for path in [&old_dev, &production, &legacy] {
            assert_eq!(fs::read(path.join("keep")).unwrap(), b"untouched fixture");
        }
        fs::write(fresh.join("existing-profile"), b"never reset").unwrap();
        assert!(check(Some(fresh.as_os_str())).is_err());
        assert_eq!(
            fs::read(fresh.join("existing-profile")).unwrap(),
            b"never reset"
        );
        assert_eq!(fs::read(binary).unwrap(), b"source-only fixture");
    }

    #[cfg(unix)]
    #[test]
    fn isolated_destination_checks_reject_application_paths_foreign_bundles_and_symlink_aliases() {
        use std::os::unix::fs::symlink;
        let owned = tempfile::tempdir().unwrap();
        let owned = owned.path().canonicalize().unwrap();
        let home = owned.join("home");
        let old_dev = owned.join("old-development");
        let stage = owned.join("candidate");
        let fresh = owned.join("fresh");
        for path in [&home, &stage, &fresh] {
            fs::create_dir_all(path).unwrap();
        }
        let binary = stage.join("newport");
        fs::write(&binary, b"candidate").unwrap();
        let check = |binary: &Path, profile: &Path| {
            isolated_profile(binary, Some(profile.as_os_str()), &home, &old_dev)
        };
        let alias = owned.join("alias");
        symlink(&fresh, &alias).unwrap();
        assert!(check(&binary, &alias).is_err());
        let installed = home.join("Applications/Newport.app/Contents/MacOS");
        fs::create_dir_all(&installed).unwrap();
        let installed_binary = installed.join("newport");
        fs::write(&installed_binary, b"preserve installed").unwrap();
        assert!(check(&installed_binary, &fresh).is_err());
        let link = stage.join("linked-newport");
        symlink(&installed_binary, &link).unwrap();
        assert!(check(&link, &fresh).is_err());
        // Resolve the staging path as well as the target: a link located in
        // Applications must not authorize writes there using an outside target.
        let installed_link = home.join("Applications/linked-newport");
        symlink(&binary, &installed_link).unwrap();
        assert!(check(&installed_link, &fresh).is_err());
        let bundle = stage.join("Newport Dev.app");
        symlink(home.join("Applications/Newport.app"), &bundle).unwrap();
        assert!(check(&binary, &fresh).is_err());
        fs::remove_file(&bundle).unwrap();
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        let plist = bundle.join("Contents/Info.plist");
        let mut value = plist::Dictionary::new();
        value.insert("CFBundleIdentifier".into(), "app.newport".into());
        plist::Value::Dictionary(value).to_file_xml(&plist).unwrap();
        let before = fs::read(&plist).unwrap();
        assert!(check(&binary, &fresh).is_err());
        assert_eq!(fs::read(&plist).unwrap(), before);
        assert_eq!(fs::read(installed_binary).unwrap(), b"preserve installed");
        assert!(fs::read_dir(fresh).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn development_bundle_validation_rejects_mismatched_agent_payloads() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("Contents/MacOS/newport");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        for (status, valid) in [(3, false), (0, true)] {
            fs::write(
                &binary,
                format!("#!/bin/sh\n[ \"$1\" = --check-needle ] || exit 97\nexit {status}\n"),
            )
            .unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(validate_agents(&binary).is_ok(), valid);
        }
    }

    #[test]
    fn only_the_bundle_this_run_assembles_is_stopped() {
        let bundle = Path::new("/Users/me/Projects/Newport/target/debug/Newport Dev.app");
        let command = |path: &str| format!("{path}/Contents/MacOS/newport --flag");
        assert!(runs_bundle(&command(&bundle.display().to_string()), bundle));
        for other in [
            "/Users/me/Projects/Newport/target/debug/Newport.app",
            "/Users/me/Projects/Newport/target/release/Newport Dev.app",
            "/Users/me/Projects/Newport/target/dev-profile/Newport Dev.app",
            "/Users/me/Projects/Newport/target/debug/Newport Dev.app.inputs",
            "/Users/me/Projects/Newport/target/debug/Newport Dev.app2",
            "/Users/me/Projects/Newport/.worktrees/x/target/debug/Newport Dev.app",
            "/Users/me/Projects/Newport2/target/debug/Newport Dev.app",
            "/Applications/Newport.app",
        ] {
            assert!(!runs_bundle(&command(other), bundle), "{other}");
        }
        for other in [
            "/Users/me/Projects/Newport/target/debug/Newport Dev.app/Contents/Resources/x",
            "/Users/me/Projects/Newport/target/debug/newport --flag",
            "cargo xtask dev /Users/me/Projects/Newport/target/debug/Newport Dev.app/Contents/MacOS/newport",
        ] {
            assert!(!runs_bundle(other, bundle), "{other}");
        }
    }

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
