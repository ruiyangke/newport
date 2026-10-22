//! Pinned Zig and the patched Ghostty source used by libghostty-vt-sys.
//! Downloads live under `.cache/newport/ghostty`, outside the Cargo target
//! directory that cache pruning recursively strips. Nothing is installed
//! system-wide and the global `PATH` is left alone.
use crate::{
    command, root,
    util::{self, capture, Os, Platform},
};
use anyhow::{ensure, Context, Result};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{self, Command},
};

mod source;
mod zig;

pub const GHOSTTY_REVISION: &str = "3425025e585a3403340d4dc6d65132f42e05e605";
pub const ZIG_VERSION: &str = "0.16.0";
/// The `GHOSTTY_SOURCE_DIR` that `.cargo/config.toml` sets so a build outside
/// `cargo xtask env` stops on a missing directory; it never names a source.
const SOURCE_PLACEHOLDER: &str = "prepare-with-cargo-xtask-env";
const PATH_SEPARATOR: &str = if cfg!(windows) { ";" } else { ":" };

/// `<root>/.cache/newport/ghostty`.
pub fn cache() -> PathBuf {
    root().join(".cache").join("newport").join("ghostty")
}

/// Environment variables a child process needs to build the terminal engine.
#[derive(Debug, Clone, Default)]
pub struct BuildEnv {
    pub vars: Vec<(String, OsString)>,
}

impl BuildEnv {
    pub fn apply<'a>(&self, c: &'a mut Command) -> &'a mut Command {
        for (name, value) in &self.vars {
            c.env(name, value);
        }
        c
    }

    /// The value a child receives: the prepared variable, else the inherited one.
    pub fn get(&self, name: &str) -> Option<OsString> {
        self.vars
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .or_else(|| env::var_os(name))
    }

    pub fn set(&mut self, name: &str, value: impl AsRef<OsStr>) {
        let value = value.as_ref().to_owned();
        match self.vars.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = value,
            None => self.vars.push((name.to_owned(), value)),
        }
    }
}

/// `dir` ahead of an existing search path.
fn prepend_path(dir: &Path, path: Option<OsString>) -> OsString {
    let mut value = dir.as_os_str().to_owned();
    if let Some(path) = path.filter(|p| !p.is_empty()) {
        value.push(PATH_SEPARATOR);
        value.push(path);
    }
    value
}

/// The archived source has no `.git` directory, so Ghostty's version detection
/// must not discover Newport's checkout and embed its version and hash.
fn ceiling(prepared: &Path, inherited: Option<OsString>) -> Result<OsString> {
    let mut dirs = vec![prepared.parent().unwrap_or(prepared).to_path_buf()];
    for dir in inherited.iter().flat_map(env::split_paths) {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    Ok(env::join_paths(dirs)?)
}

/// An inherited `GHOSTTY_SOURCE_DIR`, unless it is empty or the config placeholder.
fn inherited_source(value: Option<OsString>) -> Option<PathBuf> {
    value
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.file_name() != Some(OsStr::new(SOURCE_PLACEHOLDER)))
}

/// `host: <triple>` from `rustc -vV`.
fn rustc_host(info: &str) -> Option<&str> {
    info.lines().find_map(|line| line.strip_prefix("host: "))
}

/// libghostty-rs passes `-Dtarget` only for a Rust cross build, so the x64 Zig
/// host on Windows ARM64 is paired with an x64 Rust host and an explicit ARM64
/// target; the produced library stays ARM64.
fn windows_arm_toolchain(env: &mut BuildEnv) -> Result<()> {
    let info = capture(env.apply(&mut command("rustc")).arg("-vV"))?;
    let host = "aarch64-pc-windows-msvc";
    if rustc_host(&info) != Some(host) {
        return Ok(());
    }
    let manifest: toml::Table =
        toml::from_str(&fs::read_to_string(root().join("rust-toolchain.toml"))?)?;
    let channel = manifest
        .get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(|c| c.as_str())
        .context("Missing pinned Rust toolchain")?;
    let toolchain = format!("{channel}-x86_64-pc-windows-msvc");
    util::run(env.apply(&mut command("rustup")).args([
        "toolchain",
        "install",
        &toolchain,
        "--profile",
        "minimal",
        "--component",
        "clippy,rustfmt",
        "--force-non-host",
    ]))?;
    env.set("RUSTUP_TOOLCHAIN", &toolchain);
    if env.get("CARGO_BUILD_TARGET").is_none() {
        env.set("CARGO_BUILD_TARGET", host);
    }
    let target = env.get("CARGO_BUILD_TARGET").unwrap_or_default();
    util::run(
        env.apply(&mut command("rustup"))
            .args(["target".as_ref(), "add".as_ref(), target.as_os_str()])
            .args(["--toolchain", &toolchain]),
    )
}

/// Install Zig, prepare the patched source and return the environment to build with.
pub fn prepare() -> Result<BuildEnv> {
    let cache = cache();
    fs::create_dir_all(&cache).with_context(|| format!("Cannot create {}", cache.display()))?;
    let mut env = BuildEnv::default();
    let windows = Platform::host().os == Os::Windows;
    if windows {
        windows_arm_toolchain(&mut env)?;
    }
    let zig = zig::ensure(&cache, &mut env)?;
    if windows {
        let output = capture(env.apply(&mut command(&zig)).arg("env"))?;
        let lib = zig::lib_dir(&output, &root())?;
        env.set("ZIG_LIB_DIR", zig::prepare_stdlib(&lib, &cache)?);
    }
    let source_dir = inherited_source(env.get("GHOSTTY_SOURCE_DIR"))
        .unwrap_or_else(|| cache.join(format!("source-{GHOSTTY_REVISION}")));
    if !source_dir.exists() {
        source::fetch(&source_dir, GHOSTTY_REVISION, &cache, &env)?;
    }
    let patches = root()
        .join("crates")
        .join("newport-terminal")
        .join("patches");
    let prepared = source::prepare(&source_dir, GHOSTTY_REVISION, &patches, &cache, &env)?;
    env.set(
        "GIT_CEILING_DIRECTORIES",
        ceiling(&prepared, env.get("GIT_CEILING_DIRECTORIES"))?,
    );
    env.set("GHOSTTY_SOURCE_DIR", &prepared);
    // Debug Ghostty adds expensive invariant checks to every parser operation.
    // Keep overflow and bounds checks while making interactive development usable.
    if env.get("LIBGHOSTTY_VT_SYS_OPTIMIZE").is_none() {
        env.set("LIBGHOSTTY_VT_SYS_OPTIMIZE", "ReleaseSafe");
    }
    Ok(env)
}

/// `GITHUB_ENV` lines and the `GITHUB_PATH` entry for a prepared environment;
/// optional variables are exported only when they differ from the step's own.
fn github_exports(
    get: impl Fn(&str) -> Option<String>,
    inherited: impl Fn(&str) -> Option<String>,
) -> (String, Option<String>) {
    let mut text = String::new();
    for name in [
        "GHOSTTY_SOURCE_DIR",
        "LIBGHOSTTY_VT_SYS_OPTIMIZE",
        "GIT_CEILING_DIRECTORIES",
    ] {
        text.push_str(&format!("{name}={}\n", get(name).unwrap_or_default()));
    }
    for name in ["ZIG_LIB_DIR", "RUSTUP_TOOLCHAIN", "CARGO_BUILD_TARGET"] {
        if let Some(value) =
            get(name).filter(|v| !v.is_empty() && Some(v) != inherited(name).as_ref())
        {
            text.push_str(&format!("{name}={value}\n"));
        }
    }
    let path = get("PATH")
        .filter(|p| Some(p) != inherited("PATH").as_ref())
        .map(|p| {
            p.split(PATH_SEPARATOR)
                .next()
                .unwrap_or_default()
                .to_owned()
        });
    (text, path)
}

/// `cargo xtask env [--github] [-- <cmd>…]`.
pub fn env_command(github: bool, command: &[OsString]) -> Result<()> {
    if github {
        ensure!(
            env::var_os("GITHUB_ENV").is_some() && env::var_os("GITHUB_PATH").is_some(),
            "--github requires a GitHub Actions environment"
        );
        let prepared = prepare()?;
        let lossy = |v: OsString| v.to_string_lossy().into_owned();
        let (text, path) = github_exports(
            |name| prepared.get(name).map(lossy),
            |name| env::var_os(name).map(lossy),
        );
        util::append_github_file("GITHUB_ENV", &text)?;
        if let Some(path) = path {
            util::append_github_file("GITHUB_PATH", &format!("{path}\n"))?;
        }
        return Ok(());
    }
    let (program, args) = command
        .split_first()
        .context("Usage: cargo xtask env [--github] [-- <command> [args...]]")?;
    let prepared = prepare()?;
    let status = prepared
        .apply(&mut crate::command(program))
        .args(args)
        .status()
        .with_context(|| format!("Cannot start {}", program.to_string_lossy()))?;
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn github_steps_receive_the_prepared_zig_library_as_well_as_ghostty() {
        let (text, path) = github_exports(
            vars(&[
                ("GHOSTTY_SOURCE_DIR", "prepared-source"),
                ("LIBGHOSTTY_VT_SYS_OPTIMIZE", "ReleaseSafe"),
                ("GIT_CEILING_DIRECTORIES", "ceiling"),
                ("ZIG_LIB_DIR", "prepared-stdlib"),
                ("PATH", "pinned-zig"),
                ("CARGO_BUILD_TARGET", "aarch64-pc-windows-msvc"),
                ("RUSTUP_TOOLCHAIN", "1.98.0-x86_64-pc-windows-msvc"),
            ]),
            vars(&[]),
        );
        let lines: Vec<_> = text.lines().collect();
        for line in [
            "ZIG_LIB_DIR=prepared-stdlib",
            "GHOSTTY_SOURCE_DIR=prepared-source",
            "CARGO_BUILD_TARGET=aarch64-pc-windows-msvc",
            "RUSTUP_TOOLCHAIN=1.98.0-x86_64-pc-windows-msvc",
        ] {
            assert!(lines.contains(&line), "{line}");
        }
        assert_eq!(path.as_deref(), Some("pinned-zig"));
    }

    #[test]
    fn github_steps_skip_variables_they_already_have() {
        let both = [("ZIG_LIB_DIR", "lib"), ("PATH", "a")];
        let (text, path) = github_exports(vars(&both), vars(&both));
        assert!(!text.contains("ZIG_LIB_DIR"));
        assert_eq!(path, None);
        let (_, path) = github_exports(
            vars(&[("PATH", &format!("zig{PATH_SEPARATOR}a"))]),
            vars(&[("PATH", "a")]),
        );
        assert_eq!(path.as_deref(), Some("zig"));
    }

    #[test]
    fn the_ceiling_stops_discovery_above_the_prepared_source() {
        let prepared = Path::new("/cache/patched-x");
        let inherited = env::join_paths(["/other", "/cache", ""]).unwrap();
        let value = ceiling(prepared, Some(inherited)).unwrap();
        let dirs: Vec<_> = env::split_paths(&value).collect();
        assert_eq!(dirs, [PathBuf::from("/cache"), PathBuf::from("/other")]);
    }

    #[test]
    fn the_config_placeholder_is_not_a_source() {
        let placeholder = cache().join(SOURCE_PLACEHOLDER);
        assert_eq!(inherited_source(Some(placeholder.into())), None);
        assert_eq!(inherited_source(Some("".into())), None);
        assert_eq!(
            inherited_source(Some("/src".into())),
            Some(PathBuf::from("/src"))
        );
    }

    #[test]
    fn zig_is_prepended_without_an_empty_entry() {
        let dir = Path::new("/zig");
        assert_eq!(prepend_path(dir, None), "/zig");
        assert_eq!(
            prepend_path(dir, Some("/bin".into())),
            format!("/zig{PATH_SEPARATOR}/bin").as_str()
        );
    }

    #[test]
    fn rustc_reports_its_host() {
        assert_eq!(
            rustc_host("rustc 1.98.0\nhost: aarch64-pc-windows-msvc\nrelease: 1.98.0"),
            Some("aarch64-pc-windows-msvc")
        );
    }

    #[test]
    fn build_prerequisites_stay_outside_the_pruned_target_directory() {
        assert!(!cache().starts_with(root().join("target")));
        // A path inside: `/.cache/newport/` matches only an existing folder,
        // and a fresh checkout has none.
        let ignored = command("git")
            .args(["check-ignore", "--quiet", "--no-index"])
            .arg(root().join(".cache/newport/zig"))
            .status()
            .unwrap();
        assert!(
            ignored.success(),
            "downloaded tools and source must remain untracked"
        );
    }
}
