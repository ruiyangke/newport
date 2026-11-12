//! Build and run native examples on an explicitly selected interactive Windows desktop.
use crate::util;
use anyhow::{Context, Result, bail, ensure};
use clap::{Args, ValueEnum};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Example {
    Window,
    Input,
    ProviderWindow,
    ProviderInput,
    Drag,
    Gesture(GestureMode),
    FileDrop(FileDropScenario),
    DropExtraction,
}
impl Example {
    fn name(self) -> &'static str {
        match self {
            Self::Window => "window_lifecycle",
            Self::Input => "input_lifecycle",
            Self::ProviderWindow => "windows_window_contract",
            Self::ProviderInput => "windows_input_contract",
            Self::Drag => "windows_drag_contract",
            Self::Gesture(_) => "windows_drag_gesture",
            Self::FileDrop(_) => "windows_file_drop",
            Self::DropExtraction => "windows_drop_extraction",
        }
    }
    fn package(self) -> &'static str {
        match self {
            Self::Window | Self::Input => "gpui-pre-windows",
            Self::ProviderWindow
            | Self::ProviderInput
            | Self::Drag
            | Self::Gesture(_)
            | Self::FileDrop(_)
            | Self::DropExtraction => "newport-remote-devtools",
        }
    }
    /// GPUI's fixtures are examples; the provider's are desktop devtools bins.
    fn kind(self) -> &'static str {
        if self.uses_provider() {
            "bin"
        } else {
            "example"
        }
    }
    fn source(self) -> &'static str {
        match self {
            Self::Window => "crates/gpui_windows/examples/window_lifecycle.rs",
            Self::Input => "crates/gpui_windows/examples/input_lifecycle.rs",
            Self::ProviderWindow => "tools/devtools/src/desktop/windows_window_contract.rs",
            Self::ProviderInput => "tools/devtools/src/desktop/windows_input_contract.rs",
            Self::Drag => "tools/devtools/src/desktop/windows_drag_contract.rs",
            Self::Gesture(_) => "tools/devtools/src/desktop/windows_drag_gesture.rs",
            Self::FileDrop(_) => "tools/devtools/src/desktop/windows_file_drop.rs",
            Self::DropExtraction => "tools/devtools/src/desktop/windows_drop_extraction.rs",
        }
    }
    fn environment(self) -> &'static str {
        match self {
            Self::Window => "GPUI_WINDOW_LIFECYCLE_FIXTURE",
            Self::Input => "GPUI_INPUT_LIFECYCLE_FIXTURE",
            Self::ProviderWindow => "NEWPORT_WINDOWS_WINDOW_FIXTURE",
            Self::ProviderInput => "NEWPORT_WINDOWS_INPUT_FIXTURE",
            Self::Drag => "NEWPORT_WINDOWS_DRAG_FIXTURE",
            Self::Gesture(_) => "NEWPORT_WINDOWS_GESTURE_FIXTURE",
            Self::FileDrop(_) => "NEWPORT_WINDOWS_FILE_DROP_FIXTURE",
            Self::DropExtraction => "NEWPORT_WINDOWS_DROP_EXTRACTION_FIXTURE",
        }
    }
    fn uses_provider(self) -> bool {
        matches!(
            self,
            Self::ProviderWindow
                | Self::ProviderInput
                | Self::Drag
                | Self::Gesture(_)
                | Self::FileDrop(_)
                | Self::DropExtraction
        )
    }
    fn requires_license(self) -> bool {
        matches!(self, Self::ProviderWindow | Self::ProviderInput)
    }
}
struct Sources {
    workspace: PathBuf,
    gpui: PathBuf,
    config: Option<PathBuf>,
}
const REAP_TIMEOUT: Duration = Duration::from_secs(5);
const BUILD_TIMEOUT: Duration = Duration::from_secs(1800);
const MAX_SOURCE_FILES: usize = 50_000;
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const FILE_DROP_CLEANUP_ALLOWANCE: Duration = Duration::from_secs(10);
const INTERACTIVE_CHECK: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
public static class NativeFixtureToken {
    [DllImport("advapi32.dll", SetLastError=true)]
    static extern bool GetTokenInformation(IntPtr token, int kind, IntPtr data, uint length, out uint needed);
    public static int Value(IntPtr token, int kind) {
        uint needed;
        GetTokenInformation(token, kind, IntPtr.Zero, 0, out needed);
        if (needed == 0 || needed > 65536) throw new Win32Exception(Marshal.GetLastWin32Error());
        IntPtr data = Marshal.AllocHGlobal((int)needed);
        try {
            if (!GetTokenInformation(token, kind, data, needed, out needed)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (kind == 20) return Marshal.ReadInt32(data);
            string sid = new SecurityIdentifier(Marshal.ReadIntPtr(data)).Value;
            return int.Parse(sid.Substring(sid.LastIndexOf('-') + 1));
        } finally { Marshal.FreeHGlobal(data); }
    }
}
'@
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$session = [Diagnostics.Process]::GetCurrentProcess().SessionId
$interactive = [Environment]::UserInteractive
$elevated = [NativeFixtureToken]::Value($identity.Token, 20) -ne 0
$integrity = [NativeFixtureToken]::Value($identity.Token, 25)
$ok = !$identity.IsSystem -and $interactive -and $session -ne 0 -and !$elevated -and $integrity -ge 8192 -and $integrity -lt 12288
@{ ok = $ok; isSystem = $identity.IsSystem; interactive = $interactive; session = $session; elevated = $elevated; integrity = $integrity } | ConvertTo-Json -Compress
if (!$ok) { exit 2 }
"#;

#[derive(Args, Debug)]
pub struct Options {
    /// Absolute local GPUI checkout used by the native example.
    #[arg(long)]
    gpui_source: PathBuf,
    /// Fresh absolute directory outside the source tree for evidence and temporary files.
    #[arg(long)]
    output_directory: PathBuf,
    /// Deadline for the native fixture, excluding its build.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(30..=300))]
    timeout_seconds: u32,
}

#[derive(Args, Debug)]
pub struct DragOptions {
    #[command(flatten)]
    common: Options,
    /// Absolute newport-remote workspace (Newport's remote/) containing the selected fixture.
    #[arg(long)]
    provider_source: PathBuf,
    /// Explicit Cargo config selecting this GPUI checkout for all GPUI dependencies.
    #[arg(long)]
    gpui_config: PathBuf,
}

#[derive(Args, Debug)]
pub struct LicensedOptions {
    #[command(flatten)]
    provider: DragOptions,
    /// Existing isolated Newport profile whose running app supplies a grant over local IPC.
    #[arg(
        long,
        required_unless_present = "development_license",
        conflicts_with = "development_license"
    )]
    newport_profile: Option<PathBuf>,
    /// Explicitly use NEWPORT_REMOTE_LICENSE_TOKEN instead of a running Newport profile.
    #[arg(long, required_unless_present = "newport_profile")]
    development_license: bool,
}

enum LicenseSelection {
    Profile(PathBuf),
    Development,
}
impl LicenseSelection {
    fn validate(self, sources: &Sources, output: &Path) -> Result<Self> {
        let path = match self {
            Self::Profile(path) => path,
            Self::Development => return Ok(Self::Development),
        };
        ensure!(path.is_absolute(), "Newport profile must be absolute");
        let metadata = fs::symlink_metadata(&path).context("Newport profile must already exist")?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Newport profile must be a real directory"
        );
        let path = path.canonicalize()?;
        for other in [output, sources.workspace.as_path(), sources.gpui.as_path()] {
            ensure!(
                !path.starts_with(other) && !other.starts_with(&path),
                "Newport profile must be separate from source and evidence trees"
            );
        }
        Ok(Self::Profile(path))
    }
    fn apply(&self, command: &mut Command) {
        match self {
            Self::Profile(path) => {
                command
                    .arg("--newport-profile")
                    .arg(path)
                    .env_remove("NEWPORT_REMOTE_LICENSE_TOKEN");
            }
            Self::Development => {
                command.arg("--development-license");
            }
        }
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Profile(_) => "newportProfile",
            Self::Development => "developmentEnvironment",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum GestureMode {
    CancelMode,
    ReleaseCapture,
}
impl GestureMode {
    fn name(self) -> &'static str {
        match self {
            Self::CancelMode => "cancel-mode",
            Self::ReleaseCapture => "release-capture",
        }
    }
}

#[derive(Args, Debug)]
pub struct GestureOptions {
    #[command(flatten)]
    provider: DragOptions,
    /// Native cancellation operation; each mode requires a separate evidence directory.
    #[arg(long, value_enum)]
    mode: GestureMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum FileDropScenario {
    Plain,
    Ctrl,
    Shift,
}
impl FileDropScenario {
    fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
        }
    }
}

#[derive(Args, Debug)]
pub struct FileDropOptions {
    /// Absolute newport-remote workspace (Newport's remote/) containing the file-drop fixture.
    #[arg(long)]
    provider_source: PathBuf,
    /// Absolute local GPUI checkout selected by the Cargo configuration.
    #[arg(long)]
    gpui_source: PathBuf,
    /// Explicit Cargo config selecting this GPUI checkout for all GPUI dependencies.
    #[arg(long)]
    gpui_config: PathBuf,
    /// Fresh absolute directory outside both source trees for retained evidence.
    #[arg(long)]
    output_directory: PathBuf,
    /// Requested manual gesture; this label does not establish a native copy or move effect.
    #[arg(long, value_enum)]
    scenario: FileDropScenario,
    /// Child gesture deadline; the supervisor adds a bounded cleanup allowance.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(10..=300))]
    timeout_seconds: u32,
}

fn checkout(path: &Path, description: &str) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "{description} must be absolute");
    let path = path
        .canonicalize()
        .with_context(|| format!("Cannot resolve {description}"))?;
    ensure!(
        path.join("Cargo.toml").is_file(),
        "{description} needs a workspace manifest"
    );
    Ok(path)
}
fn validate(
    options: &Options,
    example: Example,
    provider: Option<(&Path, &Path)>,
) -> Result<(Sources, PathBuf)> {
    let gpui = checkout(&options.gpui_source, "GPUI source")?;
    let (workspace, config) = if let Some((provider, config)) = provider {
        ensure!(
            example.uses_provider(),
            "Provider source is only valid for provider examples"
        );
        ensure!(
            config.is_absolute() && config.is_file(),
            "GPUI config must be an absolute existing file"
        );
        (
            checkout(provider, "Provider source")?,
            Some(config.canonicalize()?),
        )
    } else {
        ensure!(
            !example.uses_provider(),
            "Provider example requires provider source and GPUI config"
        );
        (gpui.clone(), None)
    };
    ensure!(
        workspace.join(example.source()).is_file(),
        "Workspace does not contain {}",
        example.source()
    );
    ensure!(
        options.output_directory.is_absolute() && !options.output_directory.exists(),
        "Select a fresh absolute evidence directory"
    );
    let parent = options
        .output_directory
        .parent()
        .context("Evidence directory needs a parent")?
        .canonicalize()
        .context("Evidence parent must already exist")?;
    let name = options
        .output_directory
        .file_name()
        .context("Evidence directory needs a name")?;
    let output = parent.join(name);
    ensure!(
        !output.starts_with(&gpui) && !output.starts_with(&workspace),
        "Evidence must be outside both source trees"
    );
    ensure!(!output.exists(), "Evidence directory must be fresh");
    Ok((
        Sources {
            workspace,
            gpui,
            config,
        },
        output,
    ))
}

fn cargo_command(sources: &Sources) -> Command {
    let mut command = crate::command("cargo");
    command.current_dir(&sources.workspace);
    if let Some(config) = &sources.config {
        command.arg("--config").arg(config);
    }
    command
}
fn build_command(sources: &Sources, example: Example) -> Command {
    let mut command = cargo_command(sources);
    command
        .args(["build", "--locked", "--manifest-path"])
        .arg(sources.workspace.join("Cargo.toml"))
        .args([
            "-p",
            example.package(),
            &format!("--{}", example.kind()),
            example.name(),
            "--message-format=json-render-diagnostics",
        ]);
    if example.uses_provider() {
        // `desktop` enables the provider's and GPUI's test support, which the
        // licensed fixtures and the window contract's scene readback use.
        command.args(["--no-default-features", "--features", "desktop"]);
    }
    if example.requires_license() {
        command.env_remove("NEWPORT_REMOTE_LICENSE_TOKEN");
    }
    command
}

fn fixture_command(
    example: Example,
    executable: &Path,
    temporary: &Path,
    profile: &Path,
    timeout_seconds: u32,
) -> Command {
    let mut command = Command::new(executable);
    command
        .current_dir(temporary)
        .arg("--isolated-profile")
        .arg(profile)
        .env(example.environment(), "1")
        .env("TEMP", temporary)
        .env("TMP", temporary)
        .env("USERPROFILE", temporary.join("user"))
        .env("APPDATA", temporary.join("user/AppData/Roaming"))
        .env("LOCALAPPDATA", temporary.join("user/AppData/Local"))
        .env("NEWPORT_DATA_DIR", profile);
    if let Example::Gesture(mode) = example {
        command.args(["--mode", mode.name()]);
    }
    if let Example::FileDrop(scenario) = example {
        command
            .args(["--scenario", scenario.name(), "--timeout-seconds"])
            .arg(timeout_seconds.to_string());
    }
    command
}

fn fixture_timeout(example: Example, timeout_seconds: u32) -> Duration {
    let deadline = Duration::from_secs(u64::from(timeout_seconds));
    if matches!(example, Example::FileDrop(_)) {
        deadline + FILE_DROP_CLEANUP_ALLOWANCE
    } else {
        deadline
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("Writing {}", path.display()))
}

fn wait(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        ensure!(
            Instant::now() < deadline,
            "Native example process exceeded {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// File redirection avoids pipe blockage while the exact child has a deadline.
fn logged(command: &mut Command, output: &Path, name: &str, timeout: Duration) -> Result<Value> {
    command
        .stdin(Stdio::null())
        .stdout(File::create_new(output.join(format!("{name}.stdout.log")))?)
        .stderr(File::create_new(output.join(format!("{name}.stderr.log")))?);
    util::without_release_secrets(command);
    let mut child = command.spawn().context("Starting native example process")?;
    let pid = child.id();
    let started = Instant::now();
    let (status, error, cleanup) = match wait(&mut child, timeout) {
        Ok(status) => (Some(status), None, None),
        Err(error) => {
            let killed = child.kill();
            let reaped = wait(&mut child, REAP_TIMEOUT);
            let cleanup = reaped.as_ref().err().map(|reap| {
                format!("Exact process exit unconfirmed: {reap:#}; termination: {killed:?}")
            });
            (reaped.ok(), Some(format!("{error:#}")), cleanup)
        }
    };
    // std::process::Child does not wait in Drop. An unconfirmed child cannot
    // hold up this command or authorize deletion of its temporary profile.
    let record = json!({
        "pid": pid, "elapsedMs": started.elapsed().as_millis(),
        "exitCode": status.and_then(|status| status.code()),
        "exitConfirmed": status.is_some(),
        "ok": error.is_none() && status.is_some_and(|status| status.success()),
        "waitError": error, "cleanupError": cleanup,
    });
    write_json(&output.join(format!("{name}.process.json")), &record)?;
    Ok(record)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn source_hashes(source: &Path) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0 || (entry.file_name() != ".git" && entry.file_name() != "target")
        })
    {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        ensure!(
            entry.file_type().is_file(),
            "Source snapshot contains a non-regular file: {}",
            entry.path().display()
        );
        bytes = bytes
            .checked_add(entry.metadata()?.len())
            .context("Source byte count overflow")?;
        ensure!(
            hashes.len() < MAX_SOURCE_FILES && bytes <= MAX_SOURCE_BYTES,
            "Source snapshot exceeds its file/size limit"
        );
        let relative = entry
            .path()
            .strip_prefix(source)?
            .to_str()
            .context("Source filename is not UTF-8")?
            .replace('\\', "/");
        hashes.insert(relative, hash_file(entry.path())?);
    }
    ensure!(
        hashes.contains_key("Cargo.lock"),
        "Native example build requires its pinned Cargo.lock"
    );
    Ok(hashes)
}

/// Cargo owns target-directory layout; only the fixture's own artifact is executable.
fn artifact(
    mut messages: impl BufRead,
    example: Example,
    expected_source: &Path,
) -> Result<PathBuf> {
    let expected_source = expected_source
        .canonicalize()
        .context("Cannot resolve the expected fixture source")?;
    let mut executables = BTreeSet::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let size = messages
            .by_ref()
            .take(MAX_JSON_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if size == 0 {
            break;
        }
        ensure!(
            size as u64 <= MAX_JSON_BYTES,
            "Cargo JSON record is too large"
        );
        let message: Value = serde_json::from_slice(&line).context("Invalid Cargo JSON output")?;
        if message["reason"] != "compiler-artifact"
            || message["target"]["name"] != example.name()
            || !message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == example.kind()))
            || message["profile"]["test"] != false
        {
            continue;
        }
        let source = message["target"]["src_path"]
            .as_str()
            .and_then(|path| Path::new(path).canonicalize().ok());
        if source.as_deref() != Some(expected_source.as_path()) {
            continue;
        }
        if let Some(path) = message["executable"].as_str() {
            executables.insert(PathBuf::from(path));
        }
    }
    ensure!(
        executables.len() == 1,
        "Expected exactly one real {} {} artifact, found {}",
        example.name(),
        example.kind(),
        executables.len()
    );
    Ok(executables.pop_first().unwrap())
}

fn read_json(path: &Path) -> Result<Value> {
    read_json_limit(path, MAX_JSON_BYTES)
}
fn read_json_limit(path: &Path, limit: u64) -> Result<Value> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "JSON exceeds its output limit");
    serde_json::from_slice(&bytes).context("Fixture did not emit a single JSON result")
}

fn parse_file_drop_events(bytes: &[u8]) -> Result<Value> {
    ensure!(
        bytes.len() as u64 <= MAX_JSON_BYTES,
        "File-drop event log exceeds its limit"
    );
    let mut ready = None;
    let mut complete = None;
    let mut heartbeats = Vec::new();
    for line in std::str::from_utf8(bytes)
        .context("File-drop event log is not UTF-8")?
        .lines()
    {
        ensure!(
            complete.is_none(),
            "File-drop events appeared after completion"
        );
        let event: Value = serde_json::from_str(line).context("Invalid file-drop JSON line")?;
        ensure!(
            event["schema_version"] == 1,
            "Unknown file-drop event schema"
        );
        match event["event"].as_str() {
            Some("ready") => {
                ensure!(ready.is_none(), "File-drop readiness was reported twice");
                ready = Some(event);
            }
            Some("heartbeat") => {
                ensure!(ready.is_some(), "File-drop heartbeat preceded readiness");
                heartbeats.push(event);
            }
            Some("complete") => {
                ensure!(ready.is_some(), "File-drop completion preceded readiness");
                complete = Some(event);
            }
            _ => bail!("Unknown file-drop event"),
        }
    }
    Ok(json!({
        "ready": ready.context("File-drop readiness is missing")?,
        "heartbeats": heartbeats,
        "complete": complete.context("File-drop completion is missing")?,
    }))
}

fn read_file_drop_events(path: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    parse_file_drop_events(&bytes)
}

fn ordinary_fixture_path(path: &Path) -> Result<PathBuf> {
    let text = path
        .to_str()
        .context("File-drop evidence requires Unicode paths")?;
    Ok(if let Some(tail) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{tail}"))
    } else {
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text))
    })
}

fn drop_coordinates<const N: usize>(value: &Value) -> Result<[i64; N]> {
    let values = value
        .as_array()
        .filter(|values| values.len() == N)
        .context("File-drop coordinate shape is invalid")?;
    let mut result = [0; N];
    for (destination, value) in result.iter_mut().zip(values) {
        *destination = value
            .as_i64()
            .filter(|value| i32::try_from(*value).is_ok())
            .context("File-drop coordinate is outside native integer bounds")?;
    }
    Ok(result)
}

fn qualify_file_drop(
    scenario: FileDropScenario,
    events: &Value,
    profile: &Path,
    pid: u64,
    timeout_seconds: u32,
) -> Result<()> {
    let ready = &events["ready"];
    let report = &events["complete"];
    ensure!(
        ready["event"] == "ready"
            && report["event"] == "complete"
            && ready["schema_version"] == 1
            && report["schema_version"] == 1
            && ready["scenario"] == scenario.name()
            && report["scenario"] == scenario.name()
            && pid > 0
            && ready["pid"].as_u64() == Some(pid)
            && ready["hwnd"].as_u64().is_some_and(|hwnd| hwnd > 0)
            && ready["timeout_seconds"].as_u64() == Some(u64::from(timeout_seconds))
            && ready["ole_registered"] == false
            && ready["expected_drops"] == 1,
        "File-drop readiness does not match the owned invocation"
    );
    let expected_profile = ordinary_fixture_path(profile)?;
    let samples = expected_profile.join("samples");
    for (field, expected) in [
        ("profile", &expected_profile),
        ("sample_directory", &samples),
    ] {
        let reported = ready[field]
            .as_str()
            .context("File-drop readiness omitted an owned path")?;
        ensure!(
            ordinary_fixture_path(Path::new(reported))? == *expected,
            "File-drop {field} escaped its owned profile"
        );
    }
    for flag in [
        "passed",
        "source_files_present_and_unchanged_before_cleanup",
        "clipboard_unchanged",
        "window_destroyed_and_class_unregistered",
        "thread_dpi_restored",
        "profile_removed",
    ] {
        ensure!(
            report[flag] == true,
            "File-drop fixture did not confirm {flag}"
        );
    }
    for field in ["failure", "window_cleanup_error", "profile_cleanup_error"] {
        ensure!(
            report.get(field).is_some_and(Value::is_null),
            "File-drop fixture reported {field}"
        );
    }
    ensure!(
        report["drops_received"] == 1,
        "File-drop fixture did not receive exactly one drop"
    );
    let clipboard = report["clipboard_before"]
        .as_u64()
        .filter(|sequence| (1..=u64::from(u32::MAX)).contains(sequence))
        .context("File-drop clipboard identity is unavailable")?;
    ensure!(
        report["clipboard_after"].as_u64() == Some(clipboard),
        "File-drop clipboard identity changed"
    );
    let elapsed_ms = report["elapsed_ms"]
        .as_u64()
        .context("File-drop elapsed time is missing")?;
    let mut last_elapsed = 0;
    let mut last_drops = 0;
    for heartbeat in events["heartbeats"]
        .as_array()
        .context("File-drop heartbeat log is missing")?
    {
        let elapsed = heartbeat["elapsed_seconds"]
            .as_u64()
            .context("File-drop heartbeat time is missing")?;
        let drops = heartbeat["drops"]
            .as_u64()
            .context("File-drop heartbeat count is missing")?;
        ensure!(
            heartbeat["schema_version"] == 1
                && heartbeat["event"] == "heartbeat"
                && elapsed >= last_elapsed
                && elapsed <= u64::from(timeout_seconds)
                && elapsed <= elapsed_ms / 1_000
                && drops >= last_drops
                && drops <= 1,
            "File-drop heartbeat contradicts the final receipt"
        );
        last_elapsed = elapsed;
        last_drops = drops;
    }
    let drop = &report["drop"];
    for flag in [
        "accepted",
        "owned_sample_paths",
        "drag_finish_returned",
        "client",
        "point_inside_client",
    ] {
        ensure!(
            drop[flag] == true,
            "File-drop metadata did not confirm {flag}"
        );
    }
    ensure!(drop["error"].is_null(), "File-drop metadata capture failed");
    let owned = BTreeSet::from([
        samples.clone(),
        samples.join("folder"),
        samples.join("plain.txt"),
        samples.join("café-你好.txt"),
        samples.join("folder").join("item.txt"),
    ]);
    let paths = drop["paths"]
        .as_array()
        .context("File-drop paths are missing")?;
    ensure!(
        !paths.is_empty() && drop["count"].as_u64() == Some(paths.len() as u64),
        "File-drop count differs from its paths"
    );
    let mut observed = BTreeSet::new();
    for path in paths {
        let path = ordinary_fixture_path(Path::new(
            path.as_str()
                .context("File-drop path is not Unicode text")?,
        ))?;
        ensure!(
            owned.contains(&path) && observed.insert(path),
            "File-drop paths contain foreign or repeated samples"
        );
    }
    let [x, y] = drop_coordinates::<2>(&drop["point"])?;
    let [left, top, right, bottom] = drop_coordinates::<4>(&drop["client_bounds"])?;
    ensure!(
        left <= x && x < right && top <= y && y < bottom,
        "Final file-drop point is outside the receiving client area"
    );
    drop_coordinates::<2>(&drop["header_point"])?;
    ensure!(
        drop["header_non_client"].is_boolean()
            && drop["dpi"]
                .as_u64()
                .is_some_and(|dpi| (1..=u64::from(u32::MAX)).contains(&dpi))
            && drop["metadata_bytes"]
                .as_u64()
                .is_some_and(|bytes| (24..=256 * 1024).contains(&bytes))
            && drop["capture_us"].as_u64().is_some()
            && drop["coordinate_basis"]
                .as_str()
                .is_some_and(|basis| !basis.is_empty())
            && report["effect_scope"]
                .as_str()
                .is_some_and(|scope| !scope.is_empty())
            && report["scope"]
                .as_str()
                .is_some_and(|scope| !scope.is_empty()),
        "File-drop receipt omitted native observations or their scope"
    );
    match fs::symlink_metadata(profile) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Cannot verify file-drop profile removal"),
        Ok(_) => bail!("File-drop fixture left its profile behind"),
    }
}

fn matching_counts(report: &Value, made: &str, released: &str) -> Result<()> {
    let made = report[made]
        .as_u64()
        .context("Fixture did not report created resources")?;
    ensure!(
        made > 0 && report[released].as_u64() == Some(made),
        "Native fixture resource cleanup was incomplete"
    );
    Ok(())
}
fn qualify_fixture(example: Example, report: &Value, profile_exists: bool) -> Result<()> {
    ensure!(
        matches!(
            example,
            Example::Window
                | Example::Input
                | Example::ProviderWindow
                | Example::ProviderInput
                | Example::Drag
        ),
        "This example requires its dedicated receipt validator"
    );
    ensure!(
        report["schemaVersion"] == 1 && report["ok"] == true,
        "Native fixture did not succeed"
    );
    ensure!(
        report["failures"].as_array().is_some_and(Vec::is_empty),
        "Native fixture reported failures"
    );
    ensure!(!profile_exists, "Fixture left its profile guard behind");
    if example == Example::Drag {
        for (section, made, released) in [
            ("dataObjects", "created", "destroyed"),
            ("media", "created", "freed"),
            ("windows", "created", "destroyed"),
            ("targets", "registered", "revoked"),
            ("storage", "workers", "cleaned"),
        ] {
            matching_counts(&report[section], made, released)?;
        }
        for flag in [
            "profileRemoved",
            "foregroundUnchanged",
            "captureUnchanged",
            "cursorUnchanged",
            "clipboardUnchanged",
        ] {
            ensure!(report[flag] == true, "Drag fixture did not confirm {flag}");
        }
        ensure!(
            report["records"]
                .as_array()
                .is_some_and(|records| !records.is_empty())
                && report["scope"]
                    .as_str()
                    .is_some_and(|scope| !scope.is_empty()),
            "Drag fixture omitted its contract evidence or scope"
        );
    } else {
        ensure!(
            report["completed"] == true,
            "Native lifecycle fixture did not complete"
        );
        matching_counts(report, "created", "destroyed")?;
        if example.requires_license() {
            let (windows, observations) = if example == Example::ProviderWindow {
                (6, 16)
            } else {
                (4, 16)
            };
            ensure!(
                report["created"] == windows && report["licenseVerified"] == true,
                "Provider fixture did not verify its grant or exercise the expected windows"
            );
            ensure!(
                report["passed"]
                    .as_array()
                    .is_some_and(|passed| passed.len() == observations
                        && passed
                            .iter()
                            .all(|item| item.as_str().is_some_and(|text| !text.is_empty())))
                    && report["unavailable"].is_array()
                    && report["scope"]
                        .as_str()
                        .is_some_and(|scope| !scope.is_empty()),
                "Provider fixture omitted observations or coverage limits"
            );
            if example == Example::ProviderWindow {
                qualify_non_client_probe(&report["nonClientProbe"])?;
            }
        }
        if example == Example::Input {
            ensure!(
                report["observations"]
                    .as_array()
                    .is_some_and(|observations| !observations.is_empty())
                    && report["unavailable"].is_array()
                    && report["scope"].is_string(),
                "Input fixture omitted observations or coverage limitations"
            );
        }
    }
    Ok(())
}

fn qualify_non_client_probe(report: &Value) -> Result<()> {
    for flag in ["policyRestored", "backdropDestroyed", "frameReset"] {
        ensure!(
            report[flag] == true,
            "Appearance probe did not confirm {flag}"
        );
    }
    ensure!(
        report["dwellMs"] == 9_000,
        "Appearance observation dwell changed"
    );
    let alpha = &report["alpha"];
    ensure!(
        alpha["width"].as_u64().is_some_and(|value| value > 2)
            && alpha["height"].as_u64().is_some_and(|value| value > 2)
            && alpha["corners"] == json!([[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]])
            && alpha["center"] == json!([248, 248, 248, 255]),
        "Appearance probe did not establish transparent GPU scene corners"
    );
    let phases = report["phases"]
        .as_array()
        .context("Missing appearance phases")?;
    ensure!(
        phases.len() == 3,
        "Appearance probe omitted a native policy phase"
    );
    for (phase, name) in phases.iter().zip(["baseline", "disabled", "restored"]) {
        ensure!(phase["phase"] == name, "Appearance phases are not ordered");
        let native = &phase["native"];
        for flag in [
            "alive",
            "shown",
            "outerAvailable",
            "clientAvailable",
            "visibleAvailable",
            "directComposition",
        ] {
            ensure!(
                native[flag] == true,
                "Appearance phase {name} did not confirm {flag}"
            );
        }
        ensure!(
            native["nonClientRenderingEnabled"].is_boolean()
                && native["style"].is_u64()
                && native["extendedStyle"].is_u64()
                && native["extendedStyle"]
                    .as_u64()
                    .is_some_and(|style| style & 0x0020_0000 != 0)
                && native["dpi"].as_u64().is_some_and(|value| value > 0),
            "Appearance phase {name} omitted native policy/style/DPI observations"
        );
        for bounds in ["outer", "client", "visible", "clientScreen"] {
            ensure!(
                native[bounds]
                    .as_array()
                    .is_some_and(|values| values.len() == 4 && values.iter().all(Value::is_i64)),
                "Appearance phase {name} omitted {bounds} bounds"
            );
        }
    }
    ensure!(
        phases[1]["native"]["nonClientRenderingEnabled"] == false
            && phases[2]["native"]["nonClientRenderingEnabled"]
                == phases[0]["native"]["nonClientRenderingEnabled"],
        "Appearance native policy did not disable and restore"
    );
    for field in [
        "outer",
        "client",
        "visible",
        "clientScreen",
        "style",
        "extendedStyle",
        "dpi",
    ] {
        ensure!(
            phases[2]["native"][field] == phases[0]["native"][field],
            "Restored appearance phase changed native {field}"
        );
    }
    ensure!(
        report["backdrop"]["created"] == 1 && report["backdrop"]["destroyed"] == 1,
        "Appearance backdrop lifetime was not qualified separately"
    );
    // Disabled bounds may change. Native receipts do not qualify visual appearance.
    Ok(())
}

fn qualify_gesture(mode: GestureMode, report: &Value, profile_exists: bool) -> Result<()> {
    const DRAGDROP_S_CANCEL: i64 = 0x0004_0101;
    const CANCEL_BOUND_MS: u64 = 1_000;
    ensure!(
        report["schemaVersion"] == 1 && report["ok"] == true && report["mode"] == mode.name(),
        "Genuine gesture receipt did not succeed in the requested mode"
    );
    ensure!(
        report["failures"].as_array().is_some_and(Vec::is_empty)
            && report["closeRequested"] == false,
        "Genuine gesture was interrupted or reported failures"
    );
    ensure!(
        !profile_exists,
        "Gesture fixture left its profile guard behind"
    );
    for pointer in [
        "/genuineMouseDown",
        "/profileRemoved",
        "/clipboardUnchanged",
        "/cancelRequest/physicalButtonHeld",
        "/cancelRequest/pointerUnchanged",
        "/nativeCancelOperation/executed",
        "/nativeCancelOperation/physicalButtonHeld",
        "/nativeCancelOperation/foregroundIsSource",
        "/nativeReturn/physicalButtonHeld",
        "/nativeReturn/pointerUnchanged",
        "/nativeReturn/idleCancellationQualified",
        "/cleanup/sourceWindowDestroyed",
        "/cleanup/deadlineThreadJoined",
        "/cleanup/captureClear",
    ] {
        ensure!(
            report.pointer(pointer).and_then(Value::as_bool) == Some(true),
            "Genuine gesture did not confirm {pointer}"
        );
    }
    let number = |pointer: &str| {
        report
            .pointer(pointer)
            .and_then(Value::as_u64)
            .with_context(|| format!("Gesture receipt omitted numeric {pointer}"))
    };
    let requested = number("/cancelRequest/elapsedMs")?;
    let executed = number("/nativeCancelOperation/elapsedMs")?;
    let returned = number("/nativeReturn/elapsedMs")?;
    let latency = number("/nativeReturn/cancelToReturnMs")?;
    // Canceled tracking does not define the effect. Restoring the earlier
    // foreground window is observed separately from native cancellation.
    ensure!(
        report["foregroundRestored"].is_boolean()
            && number("/nativeReturn/effect")? <= u64::from(u32::MAX),
        "Gesture omitted its foreground or native effect observations"
    );
    ensure!(
        report["nativeReturn"]["hresult"].as_i64() == Some(DRAGDROP_S_CANCEL)
            && requested <= executed && executed <= returned
            && latency <= CANCEL_BOUND_MS
            // Each elapsed duration is independently truncated to milliseconds.
            && returned.saturating_sub(requested).abs_diff(latency) <= 1,
        "Gesture did not establish bounded native cancellation after revocation"
    );
    let operation = &report["nativeCancelOperation"];
    for flag in [
        "capturePresent",
        "captureSameThread",
        "captureSameProcess",
        "captureIsSource",
    ] {
        ensure!(
            operation[flag].is_boolean(),
            "Gesture omitted capture evidence {flag}"
        );
    }
    let timer_callbacks = number("/dispatch/timerCallbacks")?;
    ensure!(
        number("/dispatch/timerCallbacksDuringOle")? <= timer_callbacks,
        "Gesture dispatch evidence is inconsistent"
    );
    let cancel_modes = number("/dispatch/cancelModeMessages")?;
    match mode {
        GestureMode::CancelMode => ensure!(cancel_modes > 0, "WM_CANCELMODE was not dispatched"),
        GestureMode::ReleaseCapture => {
            for flag in ["capturePresent", "captureSameThread", "captureSameProcess"] {
                ensure!(
                    operation[flag] == true,
                    "ReleaseCapture lacked owned capture: {flag}"
                );
            }
        }
    }
    let query = &report["queryContinueDrag"];
    let count = number("/queryContinueDrag/count")?;
    let records = query["records"]
        .as_array()
        .context("Gesture query records are missing")?;
    ensure!(
        records.len() as u64 <= count
            && (count == 0 || !records.is_empty())
            && query["recordsTruncated"].as_bool() == Some(count > records.len() as u64),
        "Gesture callback count disagrees with its recorded observations"
    );
    // Native cancellation can finish without a QueryContinueDrag callback.
    for record in records {
        ensure!(
            record["elapsedMs"]
                .as_u64()
                .is_some_and(|elapsed| elapsed <= returned)
                && record["result"].as_i64().is_some(),
            "Gesture callback omitted its time or native result"
        );
        for flag in [
            "revoked",
            "escape",
            "buttonParameterHeld",
            "physicalButtonHeld",
        ] {
            ensure!(record[flag].is_boolean(), "Gesture callback omitted {flag}");
        }
    }
    for (made, released) in [
        ("dataCreated", "dataDestroyed"),
        ("mediaCreated", "mediaFreed"),
        ("sourcesCreated", "sourcesDestroyed"),
    ] {
        matching_counts(&report["cleanup"], made, released)?;
    }
    let prefetched = number("/localPrefetchedMedia")?;
    ensure!(
        prefetched > 0
            && prefetched <= number("/cleanup/mediaCreated")?
            && report["scope"]
                .as_str()
                .is_some_and(|scope| !scope.is_empty()),
        "Gesture omitted its independent medium or experiment scope"
    );
    Ok(())
}

fn qualify_extraction(report: &Value, expected_hash: &str) -> Result<()> {
    for flag in [
        "completed",
        "all_children_reaped",
        "foreground_unchanged",
        "clipboard_unchanged",
        "profile_retained_for_evidence",
    ] {
        ensure!(
            report[flag] == true,
            "Extraction experiment did not confirm {flag}"
        );
    }
    ensure!(
        report["executable_sha256"] == expected_hash,
        "Extraction artifact identity changed"
    );
    ensure!(
        report["scope"]
            .as_str()
            .is_some_and(|scope| !scope.is_empty()),
        "Extraction scope is missing"
    );
    let cases = report["cases"]
        .as_array()
        .context("Extraction cases are missing")?;
    ensure!(
        cases.len() == 3,
        "Extraction experiment must cover normal, cancel and stall"
    );
    for (case, mode) in cases.iter().zip(["normal", "cancel", "stall"]) {
        ensure!(case["mode"] == mode, "Extraction case order changed");
        let cleanup = case["cleanup_order"]
            .as_array()
            .context("Exact cleanup receipts are missing")?;
        let roles: &[&str] = if mode == "cancel" {
            &["handoff", "source"]
        } else {
            &["extractor", "handoff", "source"]
        };
        ensure!(
            cleanup.len() == roles.len(),
            "Extraction child count changed"
        );
        let mut pids = BTreeSet::new();
        for (child, role) in cleanup.iter().zip(roles) {
            let pid = child["pid"]
                .as_u64()
                .filter(|pid| *pid > 0)
                .context("Missing exact child PID")?;
            ensure!(
                pids.insert(pid) && child["role"] == *role && child["reaped"] == true,
                "Extraction cleanup was incomplete or out of order"
            );
            if mode != "stall" {
                ensure!(
                    child["success"] == true && child["kill_requested"] == false,
                    "Normal and cancellation cases require graceful COM cleanup"
                );
            }
        }
        if mode == "cancel" {
            ensure!(
                case["handoff"]["unused_packet_released"] == true,
                "Unused marshal packet was not released"
            );
        }
        if mode == "normal" {
            ensure!(
                case["extractor"]["medium_released"] == true
                    && case["extractor"]["path_count"] == 1,
                "Normal extraction did not release its medium or return the owned file"
            );
        }
        if mode == "stall" {
            ensure!(
                case["stalled_source_killed_last"] == true
                    && cleanup[0]["kill_requested"] == true
                    && cleanup[2]["kill_requested"] == true,
                "Stalled source must survive extractor and handoff cleanup"
            );
            let bounded = case["handoff_teardown_bounded"]
                .as_bool()
                .context("Missing handoff teardown observation")?;
            ensure!(
                cleanup[1]["kill_requested"] == !bounded,
                "Handoff teardown observation disagrees with exact cleanup"
            );
            if bounded {
                ensure!(cleanup[1]["success"] == true, "Handoff teardown failed");
            }
        } else {
            ensure!(
                case["source"]["object_dropped"] == true,
                "Source object remains referenced"
            );
        }
    }
    Ok(())
}

fn qualify_extraction_evidence(report: &Value, expected_hash: &str, profile: &Path) -> Result<()> {
    qualify_extraction(report, expected_hash)?;
    ensure!(profile.is_dir(), "Extraction evidence profile is missing");
    ensure!(
        read_json(&profile.join("result.json"))? == *report,
        "Retained extraction result differs from stdout"
    );
    for case in report["cases"].as_array().unwrap() {
        let mode = case["mode"].as_str().unwrap();
        let directory = profile.join(mode);
        ensure!(
            read_json(&directory.join("case-result.json"))? == *case,
            "Retained extraction case differs from stdout"
        );
        let roles: &[&str] = if mode == "normal" {
            &["source", "handoff", "extractor"]
        } else if mode == "cancel" {
            &["source", "handoff"]
        } else if case["handoff_teardown_bounded"] == true {
            &["handoff"]
        } else {
            &[]
        };
        for role in roles {
            ensure!(
                read_json(&directory.join(format!("{role}-done.json")))? == case[*role],
                "Retained child completion differs from its case receipt"
            );
        }
    }
    Ok(())
}

fn snapshot(sources: &Sources) -> Result<Value> {
    let files = source_hashes(&sources.workspace)?;
    if let Some(config) = &sources.config {
        Ok(
            json!({"files": files, "gpuiFiles": source_hashes(&sources.gpui)?, "gpuiConfig": {"path": config, "sha256": hash_file(config)?}}),
        )
    } else {
        Ok(json!({"files": files}))
    }
}

/// Registry and path copies of GPUI cannot coexist in one native fixture.
fn qualify_gpui(metadata: &Value, expected: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let packages = metadata["packages"]
        .as_array()
        .context("Cargo metadata has no packages")?;
    let resolved: BTreeSet<_> = metadata["resolve"]["nodes"]
        .as_array()
        .context("Cargo metadata has no resolved graph")?
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    let mut manifests = BTreeMap::new();
    for package in packages {
        let name = package["name"]
            .as_str()
            .context("Cargo package has no name")?;
        if !(name == "gpui-pre" || name.starts_with("gpui-pre-")) {
            continue;
        }
        let id = package["id"]
            .as_str()
            .context("Cargo package has no identity")?;
        if !resolved.contains(id) {
            continue;
        }
        let manifest = Path::new(
            package["manifest_path"]
                .as_str()
                .context("GPUI package has no manifest")?,
        )
        .canonicalize()?;
        ensure!(
            package["source"].is_null() && manifest.starts_with(expected),
            "GPUI dependency {name} does not come from the selected checkout"
        );
        ensure!(
            manifests.insert(name.to_owned(), manifest).is_none(),
            "Multiple resolved identities for {name}"
        );
    }
    for name in ["gpui-pre", "gpui-pre-platform", "gpui-pre-windows"] {
        ensure!(
            manifests.contains_key(name),
            "Resolved graph does not contain {name}"
        );
    }
    Ok(manifests)
}

fn cleanup_temporary(temporary: &Path, exit_confirmed: bool, summary: &mut Value) -> Result<()> {
    if !exit_confirmed {
        summary["temporaryRemoved"] = false.into();
        summary["temporaryRetained"] = json!(temporary);
        bail!("Fixture termination is unconfirmed; temporary directory retained");
    }
    // Only this invocation's reaped child received the owned directory.
    let cleanup = fs::remove_dir_all(temporary);
    summary["temporaryRemoved"] = cleanup.is_ok().into();
    cleanup.context("Cannot remove the owned fixture temporary directory")
}

fn execute(
    sources: &Sources,
    output: &Path,
    options: &Options,
    example: Example,
    license: Option<&LicenseSelection>,
    summary: &mut Value,
) -> Result<()> {
    let mut preflight = Command::new("powershell.exe");
    preflight.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        INTERACTIVE_CHECK,
    ]);
    let result = logged(&mut preflight, output, "session", Duration::from_secs(30))?;
    summary["sessionProcess"] = result.clone();
    ensure!(
        result["ok"] == true,
        "Run the fixture as the normal interactive Windows user, not SYSTEM or session zero"
    );
    let session = read_json(&output.join("session.stdout.log"))?;
    ensure!(
        session["ok"] == true && session["isSystem"] == false && session["interactive"] == true,
        "Native window fixture needs an interactive user desktop"
    );
    summary["session"] = session;

    let source_record = snapshot(sources)?;
    summary["sourceSha256"] = util::sha256_hex(&serde_json::to_vec(&source_record)?).into();
    write_json(&output.join("sources.json"), &source_record)?;
    if sources.config.is_some() {
        let mut command = cargo_command(sources);
        command
            .args([
                "metadata",
                "--locked",
                "--format-version=1",
                "--manifest-path",
            ])
            .arg(sources.workspace.join("Cargo.toml"));
        if example.requires_license() {
            command.env_remove("NEWPORT_REMOTE_LICENSE_TOKEN");
        }
        let metadata_process = logged(&mut command, output, "metadata", BUILD_TIMEOUT)?;
        summary["metadataProcess"] = metadata_process.clone();
        ensure!(
            metadata_process["ok"] == true,
            "Locked GPUI identity resolution failed; inspect metadata logs"
        );
        let metadata = read_json_limit(&output.join("metadata.stdout.log"), MAX_METADATA_BYTES)?;
        summary["gpuiIdentities"] = json!(qualify_gpui(&metadata, &sources.gpui)?);
    }
    ensure!(
        source_record == snapshot(sources)?,
        "Native example sources or GPUI config changed during resolution"
    );
    let built = logged(
        &mut build_command(sources, example),
        output,
        "build",
        BUILD_TIMEOUT,
    )?;
    summary["build"] = built.clone();
    ensure!(
        built["ok"] == true,
        "Native example build failed; inspect build.stderr.log"
    );
    ensure!(
        source_record == snapshot(sources)?,
        "Native example sources or GPUI config changed during the build"
    );
    let executable = artifact(
        BufReader::new(File::open(output.join("build.stdout.log"))?),
        example,
        &sources.workspace.join(example.source()),
    )?;
    ensure!(
        executable.is_absolute() && executable.is_file(),
        "Cargo's example executable is unavailable"
    );
    let hash = hash_file(&executable)?;
    summary["artifact"] = json!({"path": executable, "sha256": hash});
    write_json(&output.join("artifact.json"), &summary["artifact"])?;

    let temporary = output.join("temp");
    fs::create_dir(&temporary)?;
    fs::create_dir_all(temporary.join("user/AppData/Roaming"))?;
    fs::create_dir_all(temporary.join("user/AppData/Local"))?;
    let profile = temporary.join("profile");
    let mut exit_confirmed = true;
    let run = (|| -> Result<()> {
        ensure!(
            hash_file(&executable)? == hash,
            "Fixture artifact changed before execution"
        );
        if let Example::Gesture(mode) = example {
            eprintln!(
                "Gesture mode {}: wait for ready-for-real-press-and-hold in {}, then press and hold the owned probe window for three seconds without moving.",
                mode.name(),
                output.join("fixture.stderr.log").display()
            );
        }
        if let Example::FileDrop(scenario) = example {
            eprintln!(
                "File-drop scenario {}: read the flushed ready event in {}, open only its sample_directory in Explorer, and perform the requested manual gesture into the owned receiving window.",
                scenario.name(),
                output.join("fixture.stdout.log").display()
            );
        }
        let mut command = fixture_command(
            example,
            &executable,
            &temporary,
            &profile,
            options.timeout_seconds,
        );
        if let Some(license) = license {
            license.apply(&mut command);
        }
        exit_confirmed = false;
        let process = logged(
            &mut command,
            output,
            "fixture",
            fixture_timeout(example, options.timeout_seconds),
        )?;
        summary["fixtureProcess"] = process.clone();
        exit_confirmed = process["exitConfirmed"] == true;
        ensure!(
            exit_confirmed,
            "Fixture process termination was not confirmed"
        );
        summary["profileRemovedByFixture"] = (!profile.exists()).into();
        let report = if matches!(example, Example::FileDrop(_)) {
            read_file_drop_events(&output.join("fixture.stdout.log"))?
        } else {
            read_json(&output.join("fixture.stdout.log"))?
        };
        summary["fixture"] = report.clone();
        ensure!(
            process["ok"] == true,
            "Native example process failed; inspect fixture logs"
        );
        match example {
            Example::DropExtraction => {
                // A completed experiment records a blocked proxy teardown as an
                // observation. It never certifies arbitrary source code on a UI thread.
                summary["productionUiHandoffQualified"] = false.into();
                qualify_extraction_evidence(&report, &hash, &profile)
            }
            Example::Gesture(mode) => {
                qualify_gesture(mode, &report, profile.exists())?;
                summary["nativeIdleCancellationQualified"] = true.into();
                summary["foregroundRestored"] = report["foregroundRestored"].clone();
                summary["cleanupLimitations"] = if report["foregroundRestored"] == false {
                    json!(["The prior foreground window was not restored"])
                } else {
                    json!([])
                };
                Ok(())
            }
            Example::FileDrop(scenario) => {
                qualify_file_drop(
                    scenario,
                    &report,
                    &profile,
                    process["pid"]
                        .as_u64()
                        .context("Owned file-drop child PID is missing")?,
                    options.timeout_seconds,
                )?;
                summary["fileMetadataReceiptQualified"] = true.into();
                Ok(())
            }
            _ => qualify_fixture(example, &report, profile.exists()),
        }
    })();
    if example == Example::DropExtraction {
        // This example owns child processes and stores their individual COM
        // receipts in its profile. Preserve evidence even on parent failure;
        // parent termination alone cannot prove every descendant was reaped.
        summary["temporaryRemoved"] = false.into();
        summary["temporaryRetained"] = json!(temporary);
        return run;
    }
    if matches!(example, Example::FileDrop(_)) && run.is_err() {
        // The child can refuse cleanup when a sample path was replaced. Preserve
        // that evidence instead of bypassing its refusal with recursive deletion.
        summary["temporaryRemoved"] = false.into();
        summary["temporaryRetained"] = json!(temporary);
        return run;
    }
    cleanup_temporary(&temporary, exit_confirmed, summary)
        .with_context(|| format!("Temporary cleanup failed; fixture result: {run:?}"))?;
    run
}

pub fn run(options: Options) -> Result<()> {
    run_example(options, Example::Window, None)
}
pub fn run_input(options: Options) -> Result<()> {
    run_example(options, Example::Input, None)
}
pub fn run_provider_window(options: LicensedOptions) -> Result<()> {
    run_licensed(options, Example::ProviderWindow)
}
pub fn run_provider_input(options: LicensedOptions) -> Result<()> {
    run_licensed(options, Example::ProviderInput)
}
fn run_licensed(options: LicensedOptions, example: Example) -> Result<()> {
    let license = match (options.newport_profile, options.development_license) {
        (Some(profile), false) => LicenseSelection::Profile(profile),
        (None, true) => LicenseSelection::Development,
        _ => bail!("Choose exactly one explicit license source"),
    };
    run_example_with_license(
        options.provider.common,
        example,
        Some((
            options.provider.provider_source,
            options.provider.gpui_config,
        )),
        Some(license),
    )
}
pub fn run_drag(options: DragOptions) -> Result<()> {
    run_example(
        options.common,
        Example::Drag,
        Some((options.provider_source, options.gpui_config)),
    )
}
pub fn run_drag_gesture(options: GestureOptions) -> Result<()> {
    run_example(
        options.provider.common,
        Example::Gesture(options.mode),
        Some((
            options.provider.provider_source,
            options.provider.gpui_config,
        )),
    )
}
pub fn run_file_drop(options: FileDropOptions) -> Result<()> {
    run_example(
        Options {
            gpui_source: options.gpui_source,
            output_directory: options.output_directory,
            timeout_seconds: options.timeout_seconds,
        },
        Example::FileDrop(options.scenario),
        Some((options.provider_source, options.gpui_config)),
    )
}
pub fn run_drop_extraction(options: DragOptions) -> Result<()> {
    run_example(
        options.common,
        Example::DropExtraction,
        Some((options.provider_source, options.gpui_config)),
    )
}
fn run_example(
    options: Options,
    example: Example,
    provider: Option<(PathBuf, PathBuf)>,
) -> Result<()> {
    run_example_with_license(options, example, provider, None)
}
fn run_example_with_license(
    options: Options,
    example: Example,
    provider: Option<(PathBuf, PathBuf)>,
    license: Option<LicenseSelection>,
) -> Result<()> {
    ensure!(cfg!(windows), "Native examples require Windows");
    ensure!(
        example.requires_license() == license.is_some(),
        "Licensed provider examples require an explicit license source"
    );
    let (sources, output) = validate(
        &options,
        example,
        provider
            .as_ref()
            .map(|(source, config)| (source.as_path(), config.as_path())),
    )?;
    let license = license
        .map(|value| value.validate(&sources, &output))
        .transpose()?;
    fs::create_dir(&output).context("Evidence directory must be fresh")?;
    let mut summary = json!({"schemaVersion": 1, "source": sources.workspace, "gpuiSource": sources.gpui, "example": example.name(), "ok": false});
    if let Some(license) = &license {
        summary["licenseSource"] = license.name().into();
    }
    if example == Example::DropExtraction {
        summary["productionUiHandoffQualified"] = false.into();
    }
    if let Example::Gesture(mode) = example {
        summary["mode"] = mode.name().into();
        summary["nativeIdleCancellationQualified"] = false.into();
        summary["productionDragQualified"] = false.into();
    }
    if let Example::FileDrop(scenario) = example {
        summary["requestedGesture"] = scenario.name().into();
        summary["fileMetadataReceiptQualified"] = false.into();
        summary["nativeEffectQualified"] = false.into();
        summary["productionFileDropQualified"] = false.into();
    }
    let result = execute(
        &sources,
        &output,
        &options,
        example,
        license.as_ref(),
        &mut summary,
    );
    summary["ok"] = result.is_ok().into();
    if let Err(error) = &result {
        summary["error"] = format!("{error:#}").into();
    }
    write_json(&output.join("summary.json"), &summary)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_command_line_requires_one_explicit_license_source() {
        use clap::Parser;
        for example in ["windows-provider-window", "windows-provider-input"] {
            let common = [
                "xtask",
                "test",
                example,
                "--provider-source",
                "provider",
                "--gpui-source",
                "gpui",
                "--gpui-config",
                "config.toml",
                "--output-directory",
                "evidence",
            ];
            assert!(crate::Cli::try_parse_from(common).is_err());
            assert!(
                crate::Cli::try_parse_from(
                    common.into_iter().chain(["--newport-profile", "profile"])
                )
                .is_ok()
            );
            assert!(
                crate::Cli::try_parse_from(common.into_iter().chain(["--development-license"]))
                    .is_ok()
            );
            assert!(
                crate::Cli::try_parse_from(common.into_iter().chain([
                    "--newport-profile",
                    "profile",
                    "--development-license"
                ]))
                .is_err()
            );
            assert!(
                crate::Cli::try_parse_from(common.into_iter().chain([
                    "--newport-profile",
                    "profile",
                    "--newport-profile",
                    "other"
                ]))
                .is_err()
            );
        }
    }

    #[test]
    fn provider_builds_use_desktop_devtools_and_profile_mode_does_not_inherit_a_token() {
        let sources = Sources {
            workspace: "provider".into(),
            gpui: "gpui".into(),
            config: None,
        };
        for example in [Example::ProviderWindow, Example::ProviderInput] {
            let build = build_command(&sources, example);
            let args = build.get_args().collect::<Vec<_>>();
            for (flag, value) in [
                ("-p", "newport-remote-devtools"),
                ("--bin", example.name()),
                ("--features", "desktop"),
            ] {
                assert!(
                    args.windows(2)
                        .any(|pair| pair[0] == flag && pair[1] == value)
                );
            }
            assert!(args.contains(&std::ffi::OsStr::new("--no-default-features")));
            let mut fixture = fixture_command(
                example,
                Path::new("fixture.exe"),
                Path::new("scratch"),
                Path::new("scratch/profile"),
                120,
            );
            LicenseSelection::Profile("signed-in-profile".into()).apply(&mut fixture);
            let args = fixture.get_args().collect::<Vec<_>>();
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "--newport-profile" && pair[1] == "signed-in-profile")
            );
            assert!(!args.contains(&std::ffi::OsStr::new("--development-license")));
            let environment: BTreeMap<_, _> = fixture.get_envs().collect();
            assert_eq!(
                environment[std::ffi::OsStr::new("NEWPORT_REMOTE_LICENSE_TOKEN")],
                None
            );
            assert_eq!(
                environment[std::ffi::OsStr::new("NEWPORT_DATA_DIR")],
                Some(std::ffi::OsStr::new("scratch/profile"))
            );
        }
        let mut development = Command::new("fixture.exe");
        LicenseSelection::Development.apply(&mut development);
        assert_eq!(
            development.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new("--development-license")]
        );
    }

    #[test]
    fn provider_fixture_sources_are_in_the_remote_checkout() {
        let remote = crate::root().join("remote");
        // A clone without newport-remote has no provider fixtures.
        if !remote.join("tools/devtools").is_dir() {
            return;
        }
        for example in [
            Example::ProviderWindow,
            Example::ProviderInput,
            Example::Drag,
            Example::Gesture(GestureMode::CancelMode),
            Example::FileDrop(FileDropScenario::Plain),
            Example::DropExtraction,
        ] {
            assert!(
                remote.join(example.source()).is_file(),
                "{}",
                example.source()
            );
        }
    }

    #[test]
    fn signed_in_profile_cannot_overlap_evidence_or_source_trees() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let profile = root.join("signed-in");
        let workspace = root.join("provider");
        let gpui = root.join("gpui");
        for path in [&profile, &workspace, &gpui] {
            fs::create_dir(path).unwrap();
        }
        let sources = Sources {
            workspace,
            gpui,
            config: None,
        };
        let output = root.join("evidence");
        assert!(
            LicenseSelection::Profile(profile.clone())
                .validate(&sources, &output)
                .is_ok()
        );
        for path in [&root, &sources.workspace, &sources.gpui, &output] {
            assert!(
                LicenseSelection::Profile(path.clone())
                    .validate(&sources, &output)
                    .is_err()
            );
        }
        assert!(
            LicenseSelection::Profile(profile.clone())
                .validate(&sources, &profile.join("evidence"))
                .is_err()
        );
        assert!(
            LicenseSelection::Profile("relative".into())
                .validate(&sources, &output)
                .is_err()
        );
        assert!(
            profile.is_dir(),
            "Validation must not remove the app profile"
        );
    }

    #[test]
    fn provider_receipts_require_verified_license_window_counts_and_observations() {
        for (example, windows, observations) in [
            (Example::ProviderWindow, 6, 16),
            (Example::ProviderInput, 4, 16),
        ] {
            let mut success = json!({"schemaVersion":1,"ok":true,"completed":true,"licenseVerified":true,
                "created":windows,"destroyed":windows,"failures":[],"passed":vec!["observed"; observations],
                "scope":"Real provider with synthetic peer","unavailable":["Toolkit round trip"]});
            if example == Example::ProviderWindow {
                success["nonClientProbe"] = non_client_receipt();
            }
            assert!(qualify_fixture(example, &success, false).is_ok());
            assert!(qualify_fixture(example, &success, true).is_err());
            if example == Example::ProviderWindow {
                let mut missing_popups = success.clone();
                missing_popups["created"] = json!(1);
                missing_popups["destroyed"] = json!(1);
                missing_popups["passed"] = json!(vec!["observed"; 5]);
                assert!(qualify_fixture(example, &missing_popups, false).is_err());
                let mut missing_client = success.clone();
                missing_client["created"] = json!(3);
                missing_client["destroyed"] = json!(3);
                missing_client["passed"] = json!(vec!["observed"; 8]);
                assert!(qualify_fixture(example, &missing_client, false).is_err());
            }
            let mut missing_check = success.clone();
            missing_check["passed"] = json!(vec!["observed"; observations - 1]);
            assert!(qualify_fixture(example, &missing_check, false).is_err());
            for (field, value) in [
                ("licenseVerified", json!(false)),
                ("created", json!(0)),
                ("passed", json!([])),
                ("unavailable", Value::Null),
            ] {
                let mut failure = success.clone();
                failure[field] = value;
                assert!(qualify_fixture(example, &failure, false).is_err());
            }
        }
    }

    fn non_client_receipt() -> Value {
        let phases = ["baseline", "disabled", "restored"].map(|phase| {
            json!({
                "phase": phase,
                "native": {"alive": true, "shown": true, "outerAvailable": true,
                    "clientAvailable": true, "visibleAvailable": true, "directComposition": true,
                    "nonClientRenderingEnabled": phase != "disabled", "style": 1,
                    "extendedStyle": 0x00200000, "dpi": 192,
                    "outer": [-20,0,640,420], "visible": [-10,0,630,410],
                    "client": [0,0,640,400], "clientScreen": [-20,0,620,400]}
            })
        });
        json!({"policyRestored":true,"backdropDestroyed":true,"frameReset":true,"dwellMs":9000,
            "backdrop":{"created":1,"destroyed":1},
            "alpha":{"width":640,"height":400,"corners":[[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],"center":[248,248,248,255]},
            "phases":phases})
    }

    #[test]
    fn appearance_receipt_requires_composition_alpha_policy_and_cleanup_not_equal_bounds() {
        let success = non_client_receipt();
        assert!(qualify_non_client_probe(&success).is_ok());
        let mut changed_geometry = success.clone();
        changed_geometry["phases"][1]["native"]["clientScreen"] = json!([-18, 0, 620, 400]);
        assert!(qualify_non_client_probe(&changed_geometry).is_ok());
        for (pointer, value) in [
            ("/policyRestored", json!(false)),
            ("/backdropDestroyed", json!(false)),
            ("/frameReset", json!(false)),
            ("/alpha/corners/0", json!([0, 0, 0, 255])),
            ("/phases/1/native/directComposition", json!(false)),
            ("/phases/1/native/nonClientRenderingEnabled", json!(true)),
            ("/phases/2/native/nonClientRenderingEnabled", json!(false)),
            ("/phases/2/native/style", json!(0)),
            ("/phases/2/native/clientScreen", json!([-18, 0, 620, 400])),
            ("/backdrop/destroyed", json!(0)),
            ("/phases/0/native/clientScreen", Value::Null),
        ] {
            let mut failure = success.clone();
            *failure.pointer_mut(pointer).unwrap() = value;
            assert!(
                qualify_non_client_probe(&failure).is_err(),
                "Accepted {pointer}"
            );
        }
        let mut missing = success.clone();
        missing["phases"].as_array_mut().unwrap().remove(1);
        assert!(qualify_non_client_probe(&missing).is_err());
        assert!(qualify_non_client_probe(&Value::Null).is_err());
    }

    fn file_drop_report(profile: &Path, scenario: FileDropScenario) -> Value {
        let samples = profile.join("samples");
        json!({
            "ready": {"schema_version": 1, "event": "ready", "pid": 42, "hwnd": 123,
                "scenario": scenario.name(), "profile": profile, "sample_directory": samples,
                "timeout_seconds": 120, "ole_registered": false, "expected_drops": 1},
            "heartbeats": [{"schema_version": 1, "event": "heartbeat", "elapsed_seconds": 5, "drops": 0}],
            "complete": {"schema_version": 1, "event": "complete", "passed": true,
                "scenario": scenario.name(), "drops_received": 1, "failure": null,
                "source_files_present_and_unchanged_before_cleanup": true,
                "clipboard_unchanged": true, "clipboard_before": 17, "clipboard_after": 17,
                "window_destroyed_and_class_unregistered": true, "window_cleanup_error": null,
                "thread_dpi_restored": true, "profile_removed": true, "profile_cleanup_error": null,
                "elapsed_ms": 6_000, "effect_scope": "Requested gesture only; native effect unavailable",
                "scope": "Owned local metadata only",
                "drop": {"paths": [samples.join("café-你好.txt")], "count": 1, "metadata_bytes": 256,
                    "header_point": [42, 17], "header_non_client": false,
                    "point": [42, 17], "client": true, "client_bounds": [0, 0, 880, 460],
                    "point_inside_client": true, "dpi": 144, "coordinate_basis": "Receiving HWND client pixels",
                    "drag_finish_returned": true, "capture_us": 200,
                    "owned_sample_paths": true, "accepted": true}
            }
        })
    }

    fn file_drop_log(report: &Value) -> Vec<u8> {
        let mut events = vec![report["ready"].clone()];
        events.extend(report["heartbeats"].as_array().unwrap().iter().cloned());
        events.push(report["complete"].clone());
        events
            .iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn file_drop_jsonl_requires_one_versioned_ready_and_a_final_complete_record() {
        let report = file_drop_report(Path::new("owned-profile"), FileDropScenario::Plain);
        assert_eq!(
            parse_file_drop_events(&file_drop_log(&report)).unwrap(),
            report
        );
        let ready = &report["ready"];
        let heartbeat = &report["heartbeats"][0];
        let complete = &report["complete"];
        for invalid in [
            format!("{ready}\n{heartbeat}\n"),
            format!("{heartbeat}\n{ready}\n{complete}\n"),
            format!("{ready}\n{ready}\n{complete}\n"),
            format!("{ready}\n{complete}\n{heartbeat}\n"),
            format!("{ready}\n{complete}\n{complete}\n"),
            format!("{ready}\nnot json\n{complete}\n"),
            format!("{ready}\n\n{complete}\n"),
        ] {
            assert!(parse_file_drop_events(invalid.as_bytes()).is_err());
        }
        for pointer in [
            "/ready/schema_version",
            "/heartbeats/0/schema_version",
            "/complete/schema_version",
        ] {
            let mut future = report.clone();
            *future.pointer_mut(pointer).unwrap() = 2.into();
            assert!(parse_file_drop_events(&file_drop_log(&future)).is_err());
        }
        assert!(parse_file_drop_events(&vec![b' '; MAX_JSON_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn file_drop_paths_match_the_fixtures_ordinary_spelling_of_canonical_windows_paths() {
        for (canonical, ordinary) in [
            (r"\\?\C:\owned\café-你好", r"C:\owned\café-你好"),
            (r"\\?\UNC\server\share\owned", r"\\server\share\owned"),
        ] {
            assert_eq!(
                ordinary_fixture_path(Path::new(canonical)).unwrap(),
                PathBuf::from(ordinary)
            );
            assert_eq!(
                ordinary_fixture_path(Path::new(ordinary)).unwrap(),
                PathBuf::from(ordinary)
            );
        }
    }

    #[test]
    fn file_drop_qualification_requires_owned_identity_preserved_sources_and_native_cleanup() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = temporary.path().join("profile");
        for scenario in [
            FileDropScenario::Plain,
            FileDropScenario::Ctrl,
            FileDropScenario::Shift,
        ] {
            let report = file_drop_report(&profile, scenario);
            assert!(qualify_file_drop(scenario, &report, &profile, 42, 120).is_ok());
            assert!(qualify_file_drop(scenario, &report, &profile, 43, 120).is_err());
            assert!(qualify_file_drop(scenario, &report, &profile, 42, 30).is_err());
            for pointer in [
                "/complete/passed",
                "/complete/source_files_present_and_unchanged_before_cleanup",
                "/complete/clipboard_unchanged",
                "/complete/window_destroyed_and_class_unregistered",
                "/complete/thread_dpi_restored",
                "/complete/profile_removed",
                "/complete/drop/drag_finish_returned",
                "/complete/drop/accepted",
                "/complete/drop/owned_sample_paths",
                "/complete/drop/client",
            ] {
                let mut invalid = report.clone();
                *invalid.pointer_mut(pointer).unwrap() = false.into();
                assert!(
                    qualify_file_drop(scenario, &invalid, &profile, 42, 120).is_err(),
                    "{pointer}"
                );
            }
            for pointer in [
                "/ready/profile",
                "/ready/sample_directory",
                "/complete/drop/paths/0",
            ] {
                let mut foreign = report.clone();
                *foreign.pointer_mut(pointer).unwrap() = json!(temporary.path().join("unowned"));
                assert!(
                    qualify_file_drop(scenario, &foreign, &profile, 42, 120).is_err(),
                    "{pointer}"
                );
            }
            let mut moved_clipboard = report.clone();
            moved_clipboard["complete"]["clipboard_after"] = 18.into();
            assert!(qualify_file_drop(scenario, &moved_clipboard, &profile, 42, 120).is_err());
            let mut unavailable_clipboard = report.clone();
            unavailable_clipboard["complete"]["clipboard_before"] = 0.into();
            unavailable_clipboard["complete"]["clipboard_after"] = 0.into();
            assert!(
                qualify_file_drop(scenario, &unavailable_clipboard, &profile, 42, 120).is_err()
            );
            fs::create_dir(&profile).unwrap();
            assert!(qualify_file_drop(scenario, &report, &profile, 42, 120).is_err());
            fs::remove_dir(&profile).unwrap();
        }
    }

    #[test]
    fn file_drop_qualification_checks_final_client_geometry_and_does_not_infer_an_effect() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = temporary.path().join("profile");
        let scenario = FileDropScenario::Shift;
        let report = file_drop_report(&profile, scenario);
        assert!(qualify_file_drop(scenario, &report, &profile, 42, 120).is_ok());
        assert!(qualify_file_drop(FileDropScenario::Ctrl, &report, &profile, 42, 120).is_err());
        for (pointer, value) in [
            ("/complete/drop/point", json!([880, 17])),
            ("/complete/drop/point", json!([-1, 17])),
            ("/complete/drop/point", json!([42])),
            ("/complete/drop/point", json!([i64::MAX, 17])),
            ("/complete/drop/point_inside_client", json!(false)),
            ("/complete/drop/dpi", json!(0)),
            ("/complete/drop/header_point", json!(null)),
            ("/complete/drop/count", json!(2)),
            ("/heartbeats/0/elapsed_seconds", json!(7)),
            ("/heartbeats/0/drops", json!(2)),
            ("/complete/profile_cleanup_error", json!("replaced sample")),
        ] {
            let mut invalid = report.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(
                qualify_file_drop(scenario, &invalid, &profile, 42, 120).is_err(),
                "{pointer}"
            );
        }
        let mut duplicate = report;
        let path = duplicate["complete"]["drop"]["paths"][0].clone();
        duplicate["complete"]["drop"]["paths"]
            .as_array_mut()
            .unwrap()
            .push(path);
        duplicate["complete"]["drop"]["count"] = 2.into();
        assert!(qualify_file_drop(scenario, &duplicate, &profile, 42, 120).is_err());
    }

    fn gesture_report(mode: GestureMode) -> Value {
        json!({
            "schemaVersion": 1, "mode": mode.name(), "ok": true, "failures": [],
            "genuineMouseDown": true, "closeRequested": false,
            "cancelRequest": {"elapsedMs": 250, "physicalButtonHeld": true, "pointerUnchanged": true},
            "nativeCancelOperation": {
                "elapsedMs": 251, "physicalButtonHeld": true, "capturePresent": true,
                "captureSameThread": true, "captureSameProcess": true, "captureIsSource": false,
                "foregroundIsSource": true, "executed": true
            },
            "nativeReturn": {
                "hresult": 0x0004_0101, "effect": 0, "elapsedMs": 252, "cancelToReturnMs": 2,
                "physicalButtonHeld": true, "pointerUnchanged": true, "idleCancellationQualified": true
            },
            "queryContinueDrag": {"count": 0, "records": [], "recordsTruncated": false},
            "dispatch": {"timerCallbacks": 12, "timerCallbacksDuringOle": 10,
                "cancelModeMessages": if mode == GestureMode::CancelMode { 1 } else { 0 }},
            "cleanup": {
                "sourceWindowDestroyed": true, "deadlineThreadJoined": true, "captureClear": true,
                "dataCreated": 1, "dataDestroyed": 1, "mediaCreated": 1, "mediaFreed": 1,
                "sourcesCreated": 1, "sourcesDestroyed": 1
            },
            "foregroundRestored": true, "clipboardUnchanged": true, "profileRemoved": true,
            "scope": "Owned Win32 cancellation only", "localPrefetchedMedia": 1
        })
    }

    #[test]
    fn gesture_qualification_requires_observed_held_cancellation_and_exact_cleanup() {
        for mode in [GestureMode::CancelMode, GestureMode::ReleaseCapture] {
            let success = gesture_report(mode);
            assert!(qualify_gesture(mode, &success, false).is_ok());
            assert!(qualify_gesture(mode, &success, true).is_err());
            assert!(qualify_fixture(Example::Gesture(mode), &success, false).is_err());
            for pointer in [
                "/genuineMouseDown",
                "/cancelRequest/physicalButtonHeld",
                "/cancelRequest/pointerUnchanged",
                "/nativeCancelOperation/executed",
                "/nativeCancelOperation/physicalButtonHeld",
                "/nativeCancelOperation/foregroundIsSource",
                "/nativeReturn/physicalButtonHeld",
                "/nativeReturn/pointerUnchanged",
                "/nativeReturn/idleCancellationQualified",
                "/cleanup/sourceWindowDestroyed",
                "/cleanup/deadlineThreadJoined",
                "/cleanup/captureClear",
                "/clipboardUnchanged",
                "/profileRemoved",
            ] {
                let mut incomplete = success.clone();
                *incomplete.pointer_mut(pointer).unwrap() = false.into();
                assert!(
                    qualify_gesture(mode, &incomplete, false).is_err(),
                    "{pointer}"
                );
            }
            for counter in ["dataDestroyed", "mediaFreed", "sourcesDestroyed"] {
                let mut incomplete = success.clone();
                incomplete["cleanup"][counter] = 0.into();
                assert!(
                    qualify_gesture(mode, &incomplete, false).is_err(),
                    "{counter}"
                );
            }
            let mut interrupted = success.clone();
            interrupted["closeRequested"] = true.into();
            assert!(qualify_gesture(mode, &interrupted, false).is_err());
            let mut failure = success.clone();
            failure["failures"] = json!(["collision"]);
            assert!(qualify_gesture(mode, &failure, false).is_err());
            let mut no_medium = success;
            no_medium["localPrefetchedMedia"] = 0.into();
            assert!(qualify_gesture(mode, &no_medium, false).is_err());
        }
    }

    #[test]
    fn gesture_qualification_derives_the_time_bound_instead_of_trusting_a_success_flag() {
        let mode = GestureMode::CancelMode;
        let success = gesture_report(mode);
        for (pointer, value) in [
            ("/nativeReturn/hresult", json!(0x0004_0100)),
            ("/nativeReturn/effect", json!(null)),
            ("/nativeReturn/cancelToReturnMs", json!(null)),
            ("/nativeReturn/elapsedMs", json!(249)),
            ("/nativeReturn/elapsedMs", json!(260)),
            ("/nativeCancelOperation/elapsedMs", json!(249)),
            ("/nativeCancelOperation/elapsedMs", json!(253)),
        ] {
            let mut invalid = success.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(qualify_gesture(mode, &invalid, false).is_err(), "{pointer}");
        }
        let mut late = success.clone();
        late["nativeReturn"]["elapsedMs"] = 1_251.into();
        late["nativeReturn"]["cancelToReturnMs"] = 1_001.into();
        assert!(qualify_gesture(mode, &late, false).is_err());
        late["nativeReturn"]["elapsedMs"] = 1_250.into();
        late["nativeReturn"]["cancelToReturnMs"] = 1_000.into();
        assert!(qualify_gesture(mode, &late, false).is_ok());
        let mut truncated = success;
        truncated["nativeReturn"]["cancelToReturnMs"] = 1.into();
        assert!(qualify_gesture(mode, &truncated, false).is_ok());
    }

    #[test]
    fn canceled_gesture_records_effect_and_prior_foreground_without_requiring_restoration() {
        let mode = GestureMode::CancelMode;
        let mut report = gesture_report(mode);
        report["foregroundRestored"] = false.into();
        report["nativeReturn"]["effect"] = 1.into();
        assert!(qualify_gesture(mode, &report, false).is_ok());
        report["foregroundRestored"] = Value::Null;
        assert!(qualify_gesture(mode, &report, false).is_err());
    }

    #[test]
    fn gesture_modes_require_their_own_native_operation_evidence() {
        let cancel = gesture_report(GestureMode::CancelMode);
        let release = gesture_report(GestureMode::ReleaseCapture);
        assert!(qualify_gesture(GestureMode::ReleaseCapture, &cancel, false).is_err());
        assert!(qualify_gesture(GestureMode::CancelMode, &release, false).is_err());
        let mut undispatched = cancel.clone();
        undispatched["dispatch"]["cancelModeMessages"] = 0.into();
        assert!(qualify_gesture(GestureMode::CancelMode, &undispatched, false).is_err());
        for flag in ["capturePresent", "captureSameThread", "captureSameProcess"] {
            let mut foreign = release.clone();
            foreign["nativeCancelOperation"][flag] = false.into();
            assert!(qualify_gesture(GestureMode::ReleaseCapture, &foreign, false).is_err());
        }
        let mut no_capture = cancel;
        for flag in ["capturePresent", "captureSameThread", "captureSameProcess"] {
            no_capture["nativeCancelOperation"][flag] = false.into();
        }
        assert!(qualify_gesture(GestureMode::CancelMode, &no_capture, false).is_ok());
    }

    #[test]
    fn gesture_callbacks_can_be_absent_but_must_match_the_observation_receipt() {
        let mode = GestureMode::CancelMode;
        let mut report = gesture_report(mode);
        assert!(qualify_gesture(mode, &report, false).is_ok());
        report["queryContinueDrag"]["count"] = 1.into();
        assert!(qualify_gesture(mode, &report, false).is_err());
        report["queryContinueDrag"]["records"] = json!([{
            "elapsedMs": 251, "revoked": true, "escape": false,
            "buttonParameterHeld": true, "physicalButtonHeld": true, "result": 0x0004_0101
        }]);
        assert!(qualify_gesture(mode, &report, false).is_ok());
        report["queryContinueDrag"]["count"] = 2.into();
        assert!(qualify_gesture(mode, &report, false).is_err());
        report["queryContinueDrag"]["recordsTruncated"] = true.into();
        assert!(qualify_gesture(mode, &report, false).is_ok());
        report["queryContinueDrag"]["records"][0]["elapsedMs"] = 253.into();
        assert!(qualify_gesture(mode, &report, false).is_err());
        let mut malformed = gesture_report(mode);
        malformed["dispatch"]["timerCallbacksDuringOle"] = 13.into();
        assert!(qualify_gesture(mode, &malformed, false).is_err());
    }

    #[test]
    fn extraction_receipts_require_exact_cleanup_without_claiming_ui_safety() {
        let cases: Vec<_> = ["normal", "cancel", "stall"]
            .into_iter()
            .map(|mode| {
                let roles: &[&str] = if mode == "cancel" {
                    &["handoff", "source"]
                } else {
                    &["extractor", "handoff", "source"]
                };
                let cleanup: Vec<_> = roles
                    .iter()
                    .enumerate()
                    .map(|(index, role)| {
                        json!({
                            "role": role, "pid": 10 + index, "reaped": true,
                            "kill_requested": mode == "stall", "success": mode != "stall",
                        })
                    })
                    .collect();
                json!({"mode": mode, "cleanup_order": cleanup,
                "source": {"object_dropped": true}, "handoff": {"unused_packet_released": true},
                "extractor": {"medium_released": true, "path_count": 1},
                "stalled_source_killed_last": true, "handoff_teardown_bounded": false})
            })
            .collect();
        let report = json!({"completed": true, "all_children_reaped": true,
            "foreground_unchanged": true, "clipboard_unchanged": true,
            "profile_retained_for_evidence": true, "executable_sha256": "owned hash",
            "scope": "Owned standard marshaler experiment", "cases": cases});
        assert!(qualify_extraction(&report, "owned hash").is_ok());
        for pointer in [
            "/cases/0/cleanup_order/0/reaped",
            "/cases/1/handoff/unused_packet_released",
            "/cases/0/source/object_dropped",
            "/cases/0/extractor/medium_released",
        ] {
            let mut incomplete = report.clone();
            *incomplete.pointer_mut(pointer).unwrap() = false.into();
            assert!(qualify_extraction(&incomplete, "owned hash").is_err());
        }
        let mut reordered = report.clone();
        reordered["cases"][2]["cleanup_order"]
            .as_array_mut()
            .unwrap()
            .swap(0, 2);
        assert!(qualify_extraction(&reordered, "owned hash").is_err());
        assert!(qualify_extraction(&report, "different hash").is_err());
        let evidence = tempfile::tempdir().unwrap();
        assert!(qualify_extraction_evidence(&report, "owned hash", evidence.path()).is_err());
        write_json(&evidence.path().join("result.json"), &report).unwrap();
        for case in report["cases"].as_array().unwrap() {
            let directory = evidence.path().join(case["mode"].as_str().unwrap());
            fs::create_dir(&directory).unwrap();
            write_json(&directory.join("case-result.json"), case).unwrap();
            for role in ["source", "handoff", "extractor"] {
                write_json(&directory.join(format!("{role}-done.json")), &case[role]).unwrap();
            }
        }
        assert!(qualify_extraction_evidence(&report, "owned hash", evidence.path()).is_ok());
        fs::remove_file(evidence.path().join("cancel/handoff-done.json")).unwrap();
        assert!(qualify_extraction_evidence(&report, "owned hash", evidence.path()).is_err());
    }

    fn message(source: &Path, executable: &str, test: bool) -> String {
        json!({"reason": "compiler-artifact", "target": {"name": Example::Window.name(), "kind": ["example"], "src_path": source}, "profile": {"test": test}, "executable": executable}).to_string()
    }

    #[test]
    fn selects_the_example_executable_from_target_subdirectories_not_libtest() {
        let temporary = tempfile::tempdir().unwrap();
        let source_path = temporary.path().join("window_lifecycle.rs");
        fs::write(&source_path, "fn main() {}").unwrap();
        let source = source_path.as_path();
        let actual = "shared target/aarch64-pc-windows-msvc/debug/examples/window_lifecycle.exe";
        let output = format!(
            "{}\n{}\n{{\"reason\":\"build-finished\",\"success\":true}}\n",
            message(
                source,
                "target/debug/examples/window_lifecycle-test.exe",
                true
            ),
            message(source, actual, false)
        );
        assert_eq!(
            artifact(output.as_bytes(), Example::Window, source).unwrap(),
            PathBuf::from(actual)
        );
        assert!(
            artifact(
                message(Path::new("other/source.rs"), actual, false).as_bytes(),
                Example::Window,
                source
            )
            .is_err()
        );
        assert!(
            artifact(
                message(source, actual, true).as_bytes(),
                Example::Window,
                source
            )
            .is_err()
        );
        let ambiguous = format!(
            "{}\n{}",
            message(source, actual, false),
            message(source, "another.exe", false)
        );
        assert!(artifact(ambiguous.as_bytes(), Example::Window, source).is_err());
    }

    #[test]
    fn commands_preserve_paths_as_arguments_and_use_an_isolated_child_environment() {
        let source = Path::new("source with spaces");
        let sources = Sources {
            workspace: source.to_owned(),
            gpui: source.to_owned(),
            config: None,
        };
        let command = build_command(&sources, Example::Window);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args[3], source.join("Cargo.toml"));
        assert!(args.contains(&std::ffi::OsStr::new("build")));
        assert!(!args.contains(&std::ffi::OsStr::new("test")));
        let temporary = Path::new("temporary with spaces");
        let profile = temporary.join("profile");
        let command = fixture_command(
            Example::Window,
            Path::new("example.exe"),
            temporary,
            &profile,
            120,
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                std::ffi::OsStr::new("--isolated-profile"),
                profile.as_os_str()
            ]
        );
        let envs: BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            envs[std::ffi::OsStr::new("GPUI_WINDOW_LIFECYCLE_FIXTURE")],
            Some(std::ffi::OsStr::new("1"))
        );
        assert_eq!(
            envs[std::ffi::OsStr::new("TEMP")],
            Some(temporary.as_os_str())
        );
    }

    #[test]
    fn exit_success_needs_completed_windows_and_profile_cleanup_evidence() {
        let success = json!({"schemaVersion":1,"ok":true,"completed":true,"created":6,"destroyed":6,"failures":[]});
        assert!(qualify_fixture(Example::Window, &success, false).is_ok());
        assert!(qualify_fixture(Example::Window, &success, true).is_err());
        for (key, value) in [
            ("completed", json!(false)),
            ("destroyed", json!(5)),
            ("created", json!(0)),
            ("failures", json!(["unexpected activation"])),
        ] {
            let mut report = success.clone();
            report[key] = value;
            assert!(qualify_fixture(Example::Window, &report, false).is_err());
        }
    }

    #[test]
    fn an_unconfirmed_child_preserves_its_temporary_files_until_exit_is_known() {
        let parent = tempfile::tempdir().unwrap();
        let temporary = parent.path().join("owned");
        fs::create_dir(&temporary).unwrap();
        fs::write(temporary.join("profile"), "owned data").unwrap();
        let mut summary = json!({});
        assert!(cleanup_temporary(&temporary, false, &mut summary).is_err());
        assert_eq!(
            fs::read_to_string(temporary.join("profile")).unwrap(),
            "owned data"
        );
        assert_eq!(summary["temporaryRemoved"], false);
        cleanup_temporary(&temporary, true, &mut summary).unwrap();
        assert!(!temporary.exists());
        assert_eq!(summary["temporaryRemoved"], true);
    }

    #[test]
    fn command_line_requires_explicit_paths_and_bounds_the_deadline() {
        use clap::Parser;
        #[derive(Parser)]
        struct Arguments {
            #[command(flatten)]
            options: Options,
        }
        let args = [
            "fixture",
            "--gpui-source",
            "source",
            "--output-directory",
            "evidence",
        ];
        assert_eq!(
            Arguments::try_parse_from(args)
                .unwrap()
                .options
                .timeout_seconds,
            120
        );
        assert!(Arguments::try_parse_from(["fixture", "--gpui-source", "source"]).is_err());
        for timeout in ["0", "29", "301"] {
            assert!(
                Arguments::try_parse_from(args.into_iter().chain(["--timeout-seconds", timeout]))
                    .is_err()
            );
        }
    }

    #[test]
    fn source_and_evidence_paths_are_explicit_fresh_and_separate() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir_all(source.join("crates/gpui_windows/examples")).unwrap();
        fs::write(source.join("Cargo.toml"), "").unwrap();
        fs::write(source.join(Example::Window.source()), "").unwrap();
        let mut options = Options {
            gpui_source: source.clone(),
            output_directory: temporary.path().join("evidence"),
            timeout_seconds: 120,
        };
        assert!(validate(&options, Example::Window, None).is_ok());
        options.output_directory = source.join("evidence");
        assert!(validate(&options, Example::Window, None).is_err());
        options.output_directory = temporary.path().to_owned();
        assert!(validate(&options, Example::Window, None).is_err());
        options.gpui_source = PathBuf::from("relative/source");
        assert!(validate(&options, Example::Window, None).is_err());
    }
    #[test]
    fn examples_select_actual_main_and_matching_fixture_environment() {
        let sources = Sources {
            workspace: PathBuf::from("provider source"),
            gpui: PathBuf::from("gpui source"),
            config: Some(PathBuf::from("overrides with spaces.toml")),
        };
        for example in [
            Example::Input,
            Example::ProviderWindow,
            Example::ProviderInput,
            Example::Drag,
            Example::Gesture(GestureMode::CancelMode),
            Example::Gesture(GestureMode::ReleaseCapture),
            Example::FileDrop(FileDropScenario::Plain),
            Example::FileDrop(FileDropScenario::Ctrl),
            Example::FileDrop(FileDropScenario::Shift),
            Example::DropExtraction,
        ] {
            let command = build_command(&sources, example);
            let args: Vec<_> = command.get_args().collect();
            assert_eq!(args[0], "--config");
            assert_eq!(args[1], sources.config.as_ref().unwrap().as_os_str());
            assert!(args.contains(&std::ffi::OsStr::new("build")));
            assert!(args.contains(&std::ffi::OsStr::new("--locked")));
            assert!(args.contains(&std::ffi::OsStr::new(example.package())));
            assert!(args.contains(&std::ffi::OsStr::new(example.name())));
            assert!(!args.contains(&std::ffi::OsStr::new("test")));
            let command = fixture_command(
                example,
                Path::new("fixture.exe"),
                Path::new("temp"),
                Path::new("temp/profile"),
                120,
            );
            let environment: BTreeMap<_, _> = command.get_envs().collect();
            assert_eq!(
                environment[std::ffi::OsStr::new(example.environment())],
                Some(std::ffi::OsStr::new("1"))
            );
            if let Example::Gesture(mode) = example {
                assert_eq!(
                    command.get_args().collect::<Vec<_>>(),
                    vec![
                        std::ffi::OsStr::new("--isolated-profile"),
                        std::ffi::OsStr::new("temp/profile"),
                        std::ffi::OsStr::new("--mode"),
                        std::ffi::OsStr::new(mode.name()),
                    ]
                );
            }
            if let Example::FileDrop(scenario) = example {
                assert_eq!(
                    command.get_args().collect::<Vec<_>>(),
                    vec![
                        std::ffi::OsStr::new("--isolated-profile"),
                        std::ffi::OsStr::new("temp/profile"),
                        std::ffi::OsStr::new("--scenario"),
                        std::ffi::OsStr::new(scenario.name()),
                        std::ffi::OsStr::new("--timeout-seconds"),
                        std::ffi::OsStr::new("120"),
                    ]
                );
            }
            let temporary = tempfile::tempdir().unwrap();
            let source = temporary.path().join("fixture.rs");
            fs::write(&source, "fn main() {}").unwrap();
            let mut output: Value =
                serde_json::from_str(&message(&source, "fixture.exe", false)).unwrap();
            output["target"]["name"] = example.name().into();
            output["target"]["kind"] = json!([example.kind()]);
            assert_eq!(
                artifact(output.to_string().as_bytes(), example, &source).unwrap(),
                PathBuf::from("fixture.exe")
            );
            output["profile"]["test"] = true.into();
            assert!(artifact(output.to_string().as_bytes(), example, &source).is_err());
        }
    }

    #[test]
    fn genuine_gesture_command_requires_an_explicit_supported_cancellation_mode() {
        use clap::Parser;
        let common = [
            "xtask",
            "test",
            "windows-drag-gesture",
            "--gpui-source",
            "gpui",
            "--provider-source",
            "provider",
            "--gpui-config",
            "overrides.toml",
            "--output-directory",
            "evidence",
        ];
        assert!(crate::Cli::try_parse_from(common).is_err());
        for mode in [GestureMode::CancelMode, GestureMode::ReleaseCapture] {
            let parsed =
                crate::Cli::try_parse_from(common.into_iter().chain(["--mode", mode.name()]))
                    .unwrap();
            assert!(matches!(
                parsed.command,
                crate::Task::Test {
                    suite: crate::TestSuite::WindowsDragGesture(GestureOptions { mode: selected, .. })
                } if selected == mode
            ));
        }
        for invalid in ["both", "auto", "", "cancel-mode,release-capture"] {
            assert!(
                crate::Cli::try_parse_from(common.into_iter().chain(["--mode", invalid])).is_err()
            );
        }
    }

    #[test]
    fn file_drop_command_requires_a_manual_scenario_and_reserves_cleanup_time() {
        use clap::Parser;
        let common = [
            "xtask",
            "test",
            "windows-file-drop",
            "--gpui-source",
            "gpui",
            "--provider-source",
            "provider",
            "--gpui-config",
            "overrides.toml",
            "--output-directory",
            "evidence",
        ];
        assert!(crate::Cli::try_parse_from(common).is_err());
        for scenario in [
            FileDropScenario::Plain,
            FileDropScenario::Ctrl,
            FileDropScenario::Shift,
        ] {
            for timeout in [10, 120, 300] {
                let timeout_arg = timeout.to_string();
                let parsed = crate::Cli::try_parse_from(common.into_iter().chain([
                    "--scenario",
                    scenario.name(),
                    "--timeout-seconds",
                    &timeout_arg,
                ]))
                .unwrap();
                assert!(matches!(
                    parsed.command,
                    crate::Task::Test { suite: crate::TestSuite::WindowsFileDrop(FileDropOptions {
                        scenario: selected, timeout_seconds: requested, ..
                    }) } if selected == scenario && requested == timeout
                ));
                assert_eq!(
                    fixture_timeout(Example::FileDrop(scenario), timeout),
                    Duration::from_secs(u64::from(timeout)) + FILE_DROP_CLEANUP_ALLOWANCE,
                );
            }
        }
        for scenario in ["copy", "move", "all"] {
            assert!(
                crate::Cli::try_parse_from(common.into_iter().chain(["--scenario", scenario]))
                    .is_err()
            );
        }
        for timeout in ["0", "9", "301"] {
            assert!(
                crate::Cli::try_parse_from(common.into_iter().chain([
                    "--scenario",
                    "plain",
                    "--timeout-seconds",
                    timeout,
                ]))
                .is_err()
            );
        }
        assert_eq!(
            fixture_timeout(Example::Gesture(GestureMode::CancelMode), 120),
            Duration::from_secs(120)
        );
        assert_eq!(
            fixture_timeout(Example::DropExtraction, 120),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn drag_requires_explicit_gpui_configuration_and_both_source_roots() {
        use clap::Parser;
        #[derive(Parser)]
        struct Arguments {
            #[command(flatten)]
            options: DragOptions,
        }
        let common = [
            "fixture",
            "--gpui-source",
            "gpui",
            "--provider-source",
            "provider",
            "--output-directory",
            "evidence",
        ];
        assert!(Arguments::try_parse_from(common).is_err());
        let parsed = Arguments::try_parse_from(
            common
                .into_iter()
                .chain(["--gpui-config", "overrides.toml"]),
        )
        .unwrap();
        assert_eq!(parsed.options.gpui_config, PathBuf::from("overrides.toml"));
        let temporary = tempfile::tempdir().unwrap();
        let gpui = temporary.path().join("gpui");
        let provider = temporary.path().join("provider");
        for source in [&gpui, &provider] {
            fs::create_dir(source).unwrap();
            fs::write(source.join("Cargo.toml"), "").unwrap();
        }
        let example = provider.join(Example::Drag.source());
        fs::create_dir_all(example.parent().unwrap()).unwrap();
        fs::write(example, "fn main() {}").unwrap();
        let config = temporary.path().join("gpui-overrides.toml");
        fs::write(&config, "").unwrap();
        let mut options = Options {
            gpui_source: gpui.clone(),
            output_directory: temporary.path().join("evidence"),
            timeout_seconds: 120,
        };
        assert!(validate(&options, Example::Drag, Some((&provider, &config))).is_ok());
        assert!(validate(&options, Example::Drag, None).is_err());
        options.output_directory = provider.join("evidence");
        assert!(validate(&options, Example::Drag, Some((&provider, &config))).is_err());
        options.output_directory = gpui.join("evidence");
        assert!(validate(&options, Example::Drag, Some((&provider, &config))).is_err());
    }

    #[test]
    fn resolved_gpui_cannot_include_registry_duplicates_or_another_checkout() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("gpui");
        fs::create_dir(&source).unwrap();
        let source = source.canonicalize().unwrap();
        let mut packages = vec![];
        let mut nodes = vec![];
        for name in ["gpui-pre", "gpui-pre-platform", "gpui-pre-windows"] {
            let member = source.join(name);
            fs::create_dir(&member).unwrap();
            let manifest = member.join("Cargo.toml");
            fs::write(&manifest, "").unwrap();
            packages.push(json!({"name":name,"id":name,"source":null,"manifest_path":manifest}));
            nodes.push(json!({"id":name}));
        }
        let valid = json!({"packages": packages, "resolve":{"nodes":nodes}});
        assert_eq!(qualify_gpui(&valid, &source).unwrap().len(), 3);
        let mut registry = valid.clone();
        registry["packages"][0]["source"] = "registry+https://example.invalid/index".into();
        assert!(qualify_gpui(&registry, &source).is_err());
        let mut duplicate = valid.clone();
        let mut package = duplicate["packages"][0].clone();
        package["id"] = "another-gpui-core".into();
        duplicate["packages"].as_array_mut().unwrap().push(package);
        duplicate["resolve"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"another-gpui-core"}));
        assert!(qualify_gpui(&duplicate, &source).is_err());
        let outside = temporary.path().join("other-Cargo.toml");
        fs::write(&outside, "").unwrap();
        let mut foreign = valid.clone();
        foreign["packages"][0]["manifest_path"] = json!(outside);
        assert!(qualify_gpui(&foreign, &source).is_err());
        let mut incomplete = valid;
        incomplete["resolve"]["nodes"].as_array_mut().unwrap().pop();
        assert!(qualify_gpui(&incomplete, &source).is_err());
    }

    #[test]
    fn drag_receipt_requires_every_retention_and_desktop_cleanup_boundary() {
        let success = json!({"schemaVersion":1,"ok":true,"failures":[],"dataObjects":{"created":3,"destroyed":3},"media":{"created":4,"freed":4},"windows":{"created":1,"destroyed":1},"targets":{"registered":1,"revoked":1},"storage":{"workers":2,"cleaned":2},"profileRemoved":true,"foregroundUnchanged":true,"captureUnchanged":true,"cursorUnchanged":true,"clipboardUnchanged":true,"records":[{"case":"contract"}],"scope":"COM ownership only"});
        assert!(qualify_fixture(Example::Drag, &success, false).is_ok());
        assert!(qualify_fixture(Example::Drag, &success, true).is_err());
        for (section, count) in [
            ("dataObjects", "destroyed"),
            ("media", "freed"),
            ("windows", "destroyed"),
            ("targets", "revoked"),
            ("storage", "cleaned"),
        ] {
            let mut report = success.clone();
            report[section][count] = 0.into();
            assert!(qualify_fixture(Example::Drag, &report, false).is_err());
        }
        for flag in [
            "profileRemoved",
            "foregroundUnchanged",
            "captureUnchanged",
            "cursorUnchanged",
            "clipboardUnchanged",
        ] {
            let mut report = success.clone();
            report[flag] = false.into();
            assert!(qualify_fixture(Example::Drag, &report, false).is_err());
        }
        let mut input = json!({"schemaVersion":1,"ok":true,"completed":true,"created":2,"destroyed":2,"failures":[],"observations":[{"case":"unicode"}],"unavailable":["actual IME"],"scope":"Owned-window delivery"});
        assert!(qualify_fixture(Example::Input, &input, false).is_ok());
        input["observations"] = json!([]);
        assert!(qualify_fixture(Example::Input, &input, false).is_err());
    }

    #[test]
    #[ignore = "child fixture for the native runner deadline test"]
    fn native_deadline_child_fixture() {
        assert_eq!(
            std::env::var("NEWPORT_NATIVE_DEADLINE_FIXTURE").as_deref(),
            Ok("1")
        );
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn timed_out_owned_child_is_reaped_before_a_result_is_accepted() {
        let evidence = tempfile::tempdir().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "windows_window_tests::tests::native_deadline_child_fixture",
            ])
            .env("NEWPORT_NATIVE_DEADLINE_FIXTURE", "1");
        let start = Instant::now();
        let receipt = logged(
            &mut command,
            evidence.path(),
            "deadline",
            Duration::from_millis(50),
        )
        .unwrap();
        assert_eq!(receipt["ok"], false);
        assert_eq!(receipt["exitConfirmed"], true);
        assert!(
            receipt["waitError"]
                .as_str()
                .is_some_and(|error| error.contains("exceeded"))
        );
        assert!(receipt["cleanupError"].is_null());
        assert!(start.elapsed() < REAP_TIMEOUT + Duration::from_secs(2));
        assert!(evidence.path().join("deadline.process.json").is_file());
    }
}
