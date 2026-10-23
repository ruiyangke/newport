//! The environment a language server starts in: the user's interactive login
//! shell entered in the worktree, so its `cd` hooks run as in a terminal, then
//! the worktree's direnv environment. direnv's allow list decides whether an
//! `.envrc` runs; Newport never evaluates one itself.
use super::process::{self, Collector};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) type Vars = BTreeMap<String, String>;

const SHELL_TIMEOUT: Duration = Duration::from_secs(10);
/// A first `use flake` can build a dev shell for minutes.
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ENVIRONMENT: usize = 4 * 1024 * 1024;
const MESSAGE_TAIL: usize = 4 * 1024;

pub(super) struct Loaded {
    pub(super) vars: Vars,
    /// direnv, when an `.envrc` was loaded and can be checked for changes.
    pub(super) direnv: Option<PathBuf>,
}

/// The environment for a server in `root`, reporting each state as it is
/// reached. `None` means the server must not start; the report said why.
pub(super) fn load(
    root: &Path,
    mut vars: Vars,
    use_envrc: bool,
    report: &mut dyn FnMut(Value) -> io::Result<()>,
) -> io::Result<Option<Loaded>> {
    vars.insert("PWD".into(), root.to_string_lossy().into_owned());
    let Some(direnv) = find_program("direnv", vars.get("PATH").map(String::as_str)) else {
        return Ok(match envrc_above(root) {
            Some(path) if use_envrc => {
                report(state("unavailable", Some(&path), None))?;
                None
            }
            Some(path) => {
                report(state("skipped", Some(&path), None))?;
                Some(Loaded { vars, direnv: None })
            }
            None => {
                report(state("none", None, None))?;
                Some(Loaded { vars, direnv: None })
            }
        });
    };
    let status = run(&direnv, &["status", "--json"], &vars, root, CHECK_TIMEOUT)?;
    let Some((path, allowed)) = parse_status(&String::from_utf8_lossy(&status.stdout)) else {
        // A direnv that failed couldn't say whether an `.envrc` that is there may
        // run. Starting without it would quietly give the server the wrong
        // environment.
        let failed = status.timed_out || !status.success;
        return Ok(match envrc_above(root).filter(|_| failed) {
            None => {
                report(state("none", None, None))?;
                Some(Loaded { vars, direnv: None })
            }
            Some(path) if !use_envrc => {
                report(state("skipped", Some(&path), None))?;
                Some(Loaded { vars, direnv: None })
            }
            Some(path) => {
                let message = if status.timed_out {
                    "direnv took longer than 10 seconds to check the .envrc.".to_owned()
                } else {
                    let stderr = status.stderr_text();
                    match stderr.trim() {
                        "" => "direnv couldn't check the .envrc.".to_owned(),
                        message => message.to_owned(),
                    }
                };
                report(state("failed", Some(&path), Some(&message)))?;
                None
            }
        });
    };
    if !use_envrc {
        report(state("skipped", Some(&path), None))?;
        return Ok(Some(Loaded { vars, direnv: None }));
    }
    match allowed {
        Allowed::Yes => {}
        Allowed::No => {
            report(state("blocked", Some(&path), None))?;
            return Ok(None);
        }
        Allowed::Denied => {
            report(state("denied", Some(&path), None))?;
            return Ok(None);
        }
    }
    report(state("loading", Some(&path), None))?;
    let export = run(&direnv, &["export", "json"], &vars, root, LOAD_TIMEOUT)?;
    let failure = if export.timed_out {
        Some(format!(
            "Loading the .envrc took longer than 5 minutes.\n{}",
            export.stderr_text()
        ))
    } else if !export.success {
        Some(export.stderr_text())
    } else {
        apply(&mut vars, &export.stdout).err()
    };
    if let Some(message) = failure {
        report(state("failed", Some(&path), Some(message.trim())))?;
        return Ok(None);
    }
    report(state("loaded", Some(&path), None))?;
    Ok(Some(Loaded {
        vars,
        direnv: Some(direnv),
    }))
}

/// Whether the loaded `.envrc`, or a file it watches, changed since loading.
/// direnv prints nothing when it has nothing to change, and only reads file
/// times to decide.
pub(super) fn changed(direnv: &Path, vars: &Vars, root: &Path) -> bool {
    run(direnv, &["export", "json"], vars, root, CHECK_TIMEOUT).is_ok_and(|ran| {
        !ran.timed_out && (!ran.success || !String::from_utf8_lossy(&ran.stdout).trim().is_empty())
    })
}

fn state(kind: &str, path: Option<&Path>, message: Option<&str>) -> Value {
    let mut state = json!({ "state": kind });
    if let Some(path) = path {
        state["path"] = json!(path.to_string_lossy());
    }
    if let Some(message) = message.filter(|m| !m.is_empty()) {
        state["message"] = json!(message);
    }
    state
}

/// Apply `direnv export json`: each variable is set, or removed when null.
fn apply(vars: &mut Vars, output: &[u8]) -> Result<(), String> {
    let output = String::from_utf8_lossy(output);
    if output.trim().is_empty() {
        // Already loaded, for example by the shell's own direnv hook.
        return Ok(());
    }
    let changes: BTreeMap<String, Option<String>> = serde_json::from_str(&output)
        .map_err(|_| "direnv returned an environment Newport couldn't read.".to_owned())?;
    for (name, value) in changes {
        match value {
            Some(value) => vars.insert(name, value),
            None => vars.remove(&name),
        };
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
enum Allowed {
    Yes,
    No,
    Denied,
}

/// The `.envrc` direnv would load here and whether it may run. Newer direnv
/// prints JSON; older versions print `Found RC …` lines.
fn parse_status(output: &str) -> Option<(PathBuf, Allowed)> {
    let allowed = |value: &str| match value {
        "0" | "true" => Allowed::Yes,
        "2" => Allowed::Denied,
        // Unknown means not approved: direnv would refuse it too.
        _ => Allowed::No,
    };
    if let Ok(status) = serde_json::from_str::<Value>(output.trim()) {
        let found = &status["state"]["foundRC"];
        let path = found["path"].as_str()?;
        return Some((path.into(), allowed(&found["allowed"].to_string())));
    }
    let mut path = None;
    let mut permission = Allowed::No;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("Found RC path ") {
            path = Some(PathBuf::from(value.trim()));
        } else if let Some(value) = line.strip_prefix("Found RC allowed ") {
            permission = allowed(value.trim());
        }
    }
    Some((path?, permission))
}

/// The nearest `.envrc` in `root` or a folder above it, as direnv searches.
fn envrc_above(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .map(|folder| folder.join(".envrc"))
        .find(|path| path.is_file())
}

pub(super) fn find_program(name: &str, path: Option<&str>) -> Option<PathBuf> {
    path?
        .split(':')
        .filter(|folder| folder.starts_with('/'))
        .map(|folder| Path::new(folder).join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

struct Ran {
    success: bool,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
impl Ran {
    fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

fn run(
    program: &Path,
    args: &[&str],
    vars: &Vars,
    folder: &Path,
    timeout: Duration,
) -> io::Result<Ran> {
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .envs(vars)
        .env("TERM", "dumb")
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let stdout = Collector::start(child.stdout.take().expect("piped"), MAX_ENVIRONMENT, false);
    let stderr = Collector::start(child.stderr.take().expect("piped"), MESSAGE_TAIL, true);
    let status = process::wait_until(&mut child, Instant::now() + timeout)?;
    process::signal_group(child.id(), libc::SIGKILL);
    let status = match status {
        Some(status) => Some(status),
        None => {
            let _ = child.wait();
            None
        }
    };
    Ok(Ran {
        success: status.is_some_and(|s| s.success()),
        timed_out: status.is_none(),
        stdout: stdout.finish(Duration::from_secs(1)),
        stderr: stderr.finish(Duration::from_secs(1)),
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Shell {
    Posix,
    Fish,
    Csh,
}
impl Shell {
    fn of(path: &str) -> Option<Self> {
        match Path::new(path).file_name()?.to_str()? {
            "bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" | "ash" | "yash" => Some(Self::Posix),
            "fish" => Some(Self::Fish),
            "csh" | "tcsh" => Some(Self::Csh),
            _ => None,
        }
    }
    fn quote(self, value: &str) -> String {
        match self {
            Self::Fish => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'")),
            Self::Posix | Self::Csh => format!("'{}'", value.replace('\'', "'\\''")),
        }
    }
}

/// The login environment as the user's terminal would have it in `root`.
/// Falls back to this process's own environment.
pub(super) fn login_environment(root: &Path) -> Vars {
    let fallback = || std::env::vars().collect::<Vars>();
    let Ok(needle) = std::env::current_exe() else {
        return fallback();
    };
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| shell.starts_with('/') && Shell::of(shell).is_some())
        .unwrap_or_else(|| "/bin/sh".into());
    capture(&shell, &needle, root).unwrap_or_else(|_| fallback())
}

/// Run an interactive login shell that changes into `root` and prints its
/// environment as JSON on a separate pipe, so rc-file output can't mix in.
/// Interactive zsh and older bash can't redirect to descriptors above 2, so
/// the pipe is the shell's stdin; csh writes to stdout instead.
fn capture(shell: &str, needle: &Path, root: &Path) -> io::Result<Vars> {
    let kind = Shell::of(shell).unwrap_or(Shell::Posix);
    let (reader, writer) = io::pipe()?;
    let mut command = Command::new(shell);
    let mut script = String::new();
    match kind {
        Shell::Csh => {
            // csh is a login shell when its name starts with `-`.
            command.arg0("-");
            command.stdin(Stdio::null()).stdout(writer);
        }
        Shell::Fish => {
            // fish runs direnv and version-manager hooks on the prompt event.
            script.push_str("emit fish_prompt; ");
            command.arg("-l").stdin(writer).stdout(Stdio::null());
        }
        Shell::Posix => {
            command.arg("-l").stdin(writer).stdout(Stdio::null());
        }
    }
    script.push_str(&format!(
        "cd {}; {} print-env{}",
        kind.quote(&root.to_string_lossy()),
        kind.quote(&needle.to_string_lossy()),
        if kind == Shell::Csh { "" } else { " >&0" },
    ));
    command
        .args(["-i", "-c", &script])
        .current_dir(root)
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn()?;
    // The shell and what it starts hold the only writers now.
    drop(command);
    let output = Collector::start(reader, MAX_ENVIRONMENT, false);
    let exited = process::wait_until(&mut child, Instant::now() + SHELL_TIMEOUT)?;
    // Daemons started by rc files may keep the pipe open; don't wait for them.
    let output = output.finish(Duration::from_secs(1));
    process::signal_group(child.id(), libc::SIGKILL);
    if exited.is_none() {
        let _ = child.wait();
    }
    // rc files can fail after the environment was printed; read it first.
    parse_environment(&String::from_utf8_lossy(&output))
        .ok_or_else(|| io::Error::other("The login shell didn't print its environment."))
}

/// The first JSON object of strings in `output`.
fn parse_environment(output: &str) -> Option<Vars> {
    output.match_indices('{').find_map(|(start, _)| {
        let mut parser = serde_json::Deserializer::from_str(&output[start..]);
        serde::Deserialize::deserialize(&mut parser).ok()
    })
}

/// `newport-agent print-env`: this process's environment as one JSON object.
pub(super) fn print() -> io::Result<()> {
    let vars: Vars = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let mut output = serde_json::to_vec(&vars).map_err(io::Error::other)?;
    output.push(b'\n');
    io::Write::write_all(&mut io::stdout().lock(), &output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direnv_status_is_read_from_json_or_older_text() {
        let json =
            r#"{"config":{},"state":{"foundRC":{"path":"/w/.envrc","allowed":0},"loadedRC":null}}"#;
        assert_eq!(parse_status(json), Some(("/w/.envrc".into(), Allowed::Yes)));
        let blocked = json.replace("\"allowed\":0", "\"allowed\":1");
        assert_eq!(parse_status(&blocked).unwrap().1, Allowed::No);
        let denied = json.replace("\"allowed\":0", "\"allowed\":2");
        assert_eq!(parse_status(&denied).unwrap().1, Allowed::Denied);
        let none = r#"{"config":{},"state":{"foundRC":null,"loadedRC":null}}"#;
        assert_eq!(parse_status(none), None);
        let text = "direnv exec path /usr/bin/direnv\nFound RC path /w/.envrc\nFound watch: \"/w/.envrc\"\nFound RC allowed true\n";
        assert_eq!(parse_status(text), Some(("/w/.envrc".into(), Allowed::Yes)));
        assert_eq!(
            parse_status("Found RC path /w/.envrc\nFound RC allowed false\n")
                .unwrap()
                .1,
            Allowed::No
        );
        assert_eq!(parse_status("No .envrc or .env found\n"), None);
    }

    #[test]
    fn direnv_exports_set_and_unset_variables() {
        let mut vars = Vars::from([("A".into(), "1".into()), ("B".into(), "2".into())]);
        apply(&mut vars, br#"{"A":"3","B":null,"C":"4"}"#).unwrap();
        assert_eq!(
            vars,
            Vars::from([("A".into(), "3".into()), ("C".into(), "4".into())])
        );
        apply(&mut vars, b"").unwrap();
        assert!(apply(&mut vars, b"not json").is_err());
    }

    #[test]
    fn the_environment_is_found_in_noisy_output() {
        let vars =
            parse_environment("Welcome {not json}\n{\"PATH\":\"/bin\",\"HOME\":\"/h\"}\n").unwrap();
        assert_eq!(vars["PATH"], "/bin");
        assert!(parse_environment("no environment here").is_none());
    }

    #[test]
    fn shell_quoting_survives_quotes() {
        assert_eq!(Shell::Posix.quote("/a b/it's"), r"'/a b/it'\''s'");
        assert_eq!(Shell::Fish.quote(r"/a\b/it's"), r"'/a\\b/it\'s'");
    }

    #[test]
    fn programs_are_found_on_absolute_path_entries_only() {
        let folder = tempfile::tempdir().unwrap();
        let program = folder.path().join("tool");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        let path = format!("relative:{}", folder.path().display());
        assert_eq!(find_program("tool", Some(&path)), None);
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_program("tool", Some(&path)), Some(program));
        assert_eq!(find_program("tool", Some("relative")), None);
    }

    /// A worktree with an `.envrc` and a fake direnv driven by `FAKE_*` variables.
    fn fake_direnv() -> (tempfile::TempDir, Vars) {
        let folder = tempfile::tempdir().unwrap();
        let bin = folder.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let direnv = bin.join("direnv");
        // Do not write an executable while parallel tests are forking: a child
        // can inherit its write descriptor and make exec fail with ETXTBSY.
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/direnv.sh"),
            &direnv,
        )
        .unwrap();
        std::fs::write(folder.path().join(".envrc"), "use flake\n").unwrap();
        let vars = Vars::from([
            ("PATH".into(), format!("{}:/bin:/usr/bin", bin.display())),
            ("FAKE_ALLOWED".into(), "0".into()),
            (
                "FAKE_EXPORT".into(),
                r#"{"FROM_ENVRC":"yes","FAKE_ALLOWED":null}"#.into(),
            ),
        ]);
        (folder, vars)
    }

    fn load_states(root: &Path, vars: Vars, use_envrc: bool) -> (Vec<String>, Option<Loaded>) {
        let mut states = Vec::new();
        let loaded = load(root, vars, use_envrc, &mut |state| {
            let mut line = state["state"].as_str().unwrap().to_owned();
            if let Some(message) = state["message"].as_str() {
                line = format!("{line}: {message}");
            }
            states.push(line);
            Ok(())
        })
        .unwrap();
        (states, loaded)
    }

    #[test]
    fn an_allowed_envrc_is_loaded_through_direnv() {
        let (folder, vars) = fake_direnv();
        let (states, loaded) = load_states(folder.path(), vars, true);
        assert_eq!(states, ["loading", "loaded"]);
        let loaded = loaded.unwrap();
        assert_eq!(loaded.vars["FROM_ENVRC"], "yes");
        assert!(!loaded.vars.contains_key("FAKE_ALLOWED"));
        assert_eq!(loaded.vars["PWD"], folder.path().to_string_lossy());
        let direnv = loaded.direnv.unwrap();
        // The fake prints its export again, as direnv does after a change.
        assert!(changed(&direnv, &loaded.vars, folder.path()));
        let mut unchanged = loaded.vars.clone();
        unchanged.insert("FAKE_EXPORT".into(), String::new());
        assert!(!changed(&direnv, &unchanged, folder.path()));
    }

    #[test]
    fn a_blocked_denied_or_failing_envrc_stops_the_start() {
        for (allowed, fail, expected) in [
            ("1", "", "blocked"),
            ("2", "", "denied"),
            ("0", "1", "failed: direnv: error boom"),
        ] {
            let (folder, mut vars) = fake_direnv();
            vars.insert("FAKE_ALLOWED".into(), allowed.into());
            vars.insert("FAKE_FAIL".into(), fail.into());
            let (states, loaded) = load_states(folder.path(), vars, true);
            assert_eq!(states.last().unwrap(), expected);
            assert!(loaded.is_none());
        }
    }

    #[test]
    fn a_direnv_that_cant_check_the_envrc_stops_the_start() {
        let (folder, mut vars) = fake_direnv();
        vars.insert("FAKE_STATUS_FAIL".into(), "1".into());
        let (states, loaded) = load_states(folder.path(), vars.clone(), true);
        assert_eq!(states, ["failed: direnv: bad direnvrc"]);
        assert!(loaded.is_none());
        // Starting without the .envrc doesn't need direnv's answer.
        let (states, loaded) = load_states(folder.path(), vars.clone(), false);
        assert_eq!(states, ["skipped"]);
        assert!(loaded.is_some());
        std::fs::remove_file(folder.path().join(".envrc")).unwrap();
        let (states, loaded) = load_states(folder.path(), vars, true);
        assert_eq!(states, ["none"]);
        assert!(loaded.is_some());
    }

    #[test]
    fn starting_without_the_envrc_skips_direnv() {
        let (folder, mut vars) = fake_direnv();
        vars.insert("FAKE_ALLOWED".into(), "1".into());
        let (states, loaded) = load_states(folder.path(), vars, false);
        assert_eq!(states, ["skipped"]);
        let loaded = loaded.unwrap();
        assert!(loaded.direnv.is_none() && !loaded.vars.contains_key("FROM_ENVRC"));
    }

    #[test]
    fn an_envrc_without_direnv_is_reported() {
        let (folder, _) = fake_direnv();
        let vars = Vars::from([("PATH".into(), "/bin:/usr/bin".into())]);
        if find_program("direnv", Some("/bin:/usr/bin")).is_some() {
            return;
        }
        let (states, loaded) = load_states(folder.path(), vars.clone(), true);
        assert_eq!(states, ["unavailable"]);
        assert!(loaded.is_none());
        let (states, loaded) = load_states(folder.path(), vars.clone(), false);
        assert_eq!(states, ["skipped"]);
        assert!(loaded.is_some());
        std::fs::remove_file(folder.path().join(".envrc")).unwrap();
        let (states, _) = load_states(folder.path(), vars, true);
        assert_eq!(states, ["none"]);
    }

    #[test]
    fn the_login_shell_prints_its_environment_on_a_separate_pipe() {
        let folder = tempfile::tempdir().unwrap();
        let printer = folder.path().join("printer");
        // Stands in for `newport-agent print-env`; stdout noise must not mix in.
        std::fs::write(
            &printer,
            "#!/bin/sh\necho noise >/dev/stderr\nprintf '{\"PWD_SEEN\":\"%s\"}' \"$(pwd)\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&printer, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vars = capture("/bin/sh", &printer, folder.path()).unwrap();
        let seen = std::fs::canonicalize(&vars["PWD_SEEN"]).unwrap();
        assert_eq!(seen, std::fs::canonicalize(folder.path()).unwrap());
    }

    #[test]
    fn an_envrc_is_found_in_the_root_or_above_it() {
        let folder = tempfile::tempdir().unwrap();
        let nested = folder.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(envrc_above(&nested), None);
        std::fs::write(folder.path().join("a/.envrc"), "").unwrap();
        assert_eq!(envrc_above(&nested), Some(folder.path().join("a/.envrc")));
    }
}
