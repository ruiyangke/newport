//! Language servers for the project editor. A worker starts one server in a
//! worktree, inside the user's login and direnv environment, and owns the
//! server's process group. LSP messages pass through unchanged; the worker's
//! own `$/newport/*` notifications sit between whole server messages.
mod configuration;
mod environment;
mod framing;
mod process;
mod workspace;

use crate::transport::{Cancellation, DeadlineWriter};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, BufReader, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) const VERSION: u32 = 1;

/// A server the desktop may ask for, and the commands tried in order.
struct Server {
    id: &'static str,
    commands: &'static [(&'static str, &'static [&'static str])],
}
const SERVERS: &[Server] = &[
    Server {
        id: "rust-analyzer",
        commands: &[("rust-analyzer", &[])],
    },
    Server {
        id: "typescript-language-server",
        commands: &[("typescript-language-server", &["--stdio"])],
    },
    Server {
        id: "pyright",
        commands: &[
            ("basedpyright-langserver", &["--stdio"]),
            ("pyright-langserver", &["--stdio"]),
        ],
    },
    Server {
        id: "gopls",
        commands: &[("gopls", &[])],
    },
];

/// After the desktop goes away, how long the server may exit on its own.
const INPUT_CLOSED_GRACE: Duration = Duration::from_secs(5);
/// How long a stopped group has between SIGTERM and SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// How often a loaded `.envrc` is checked for changes.
const ENVIRONMENT_CHECK: Duration = Duration::from_secs(10);
const STDERR_TAIL: usize = 8 * 1024;

type Output = Arc<Mutex<DeadlineWriter<File>>>;

/// `newport-agent print-env`, used while capturing the login environment.
pub fn print_env() -> io::Result<()> {
    environment::print()
}

/// Worker arguments: the worktree root, a server id, and `envrc` or
/// `skip-envrc` (the user chose to start without a blocked `.envrc`).
pub(crate) fn serve_bound(args: &[String], cancellation: Cancellation) -> io::Result<()> {
    let [root, id, mode] = args else {
        return Err(io::Error::other("Invalid language server request."));
    };
    let root = PathBuf::from(root);
    if !root.is_absolute() || !root.is_dir() {
        return Err(io::Error::other(
            "The worktree folder doesn't exist on this server.",
        ));
    }
    let server = SERVERS
        .iter()
        .find(|server| server.id == id)
        .ok_or_else(|| io::Error::other("Unknown language server."))?;
    let use_envrc = match mode.as_str() {
        "envrc" => true,
        "skip-envrc" => false,
        _ => return Err(io::Error::other("Invalid language server request.")),
    };
    let output = stdout(&cancellation)?;
    let login = environment::login_environment(&root);
    let loaded = environment::load(&root, login, use_envrc, &mut |state| {
        notify(&output, "$/newport/environment", state)
    })?;
    let Some(loaded) = loaded else {
        return Ok(());
    };
    let path = loaded.vars.get("PATH").map(String::as_str);
    let Some((name, program, arguments)) = server.commands.iter().find_map(|(name, arguments)| {
        environment::find_program(name, path).map(|program| (*name, program, *arguments))
    }) else {
        return notify(
            &output,
            "$/newport/server",
            json!({"state": "missing", "server": server.id}),
        );
    };
    let config_server = if name == "basedpyright-langserver" {
        "basedpyright"
    } else {
        id
    };
    run(
        &root,
        server.id,
        (&program, arguments),
        loaded,
        configuration::resolve(&root, config_server),
        output,
        cancellation,
    )
}

fn run(
    root: &Path,
    id: &str,
    (program, arguments): (&Path, &[&str]),
    loaded: environment::Loaded,
    config: Value,
    output: Output,
    cancellation: Cancellation,
) -> io::Result<()> {
    let mut child = match Command::new(program)
        .args(arguments)
        .env_clear()
        .envs(&loaded.vars)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return notify(
                &output,
                "$/newport/server",
                json!({"state": "exited", "server": id, "error": format!("{} couldn't start: {error}", program.display())}),
            );
        }
    };
    let stopping = Arc::new(AtomicBool::new(false));
    let result = (|| -> io::Result<()> {
        let watcher = workspace::watch(root, output.clone(), stopping.clone());
        let mut config = config;
        config["fileWatch"] = json!(watcher.is_ok());
        if let Err(error) = &watcher {
            config["watchError"] = json!(error.to_string());
        }
        notify(&output, "$/newport/configuration", config)?;
        notify(
            &output,
            "$/newport/server",
            json!({"state": "started", "server": id, "command": program.to_string_lossy()}),
        )?;

        // Desktop to server: bytes as written.
        let input_closed = Arc::new(AtomicBool::new(false));
        let mut server_input = child.stdin.take().expect("piped");
        let mut input = cancellation.stdin()?;
        let closed = input_closed.clone();
        std::thread::spawn(move || {
            let _ = io::copy(&mut input, &mut server_input);
            drop(server_input);
            closed.store(true, Ordering::Release);
        });

        // Server to desktop: whole messages, so notifications never split one.
        let server_output = child.stdout.take().expect("piped");
        let forward = output.clone();
        let reader = std::thread::spawn(move || -> io::Result<()> {
            let mut server_output = BufReader::new(server_output);
            while let Some(message) = framing::read_message(&mut server_output)? {
                forward.lock().unwrap().write_all(&message)?;
            }
            Ok(())
        });
        let stderr =
            process::Collector::start(child.stderr.take().expect("piped"), STDERR_TAIL, true);

        if let Some(direnv) = loaded.direnv.clone() {
            let (vars, root, output, stopping) = (
                loaded.vars.clone(),
                root.to_owned(),
                output.clone(),
                stopping.clone(),
            );
            std::thread::spawn(move || {
                let mut next = Instant::now() + ENVIRONMENT_CHECK;
                while !stopping.load(Ordering::Acquire) {
                    if Instant::now() < next {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    if environment::changed(&direnv, &vars, &root) {
                        let _ = notify(
                            &output,
                            "$/newport/environment",
                            json!({"state": "changed"}),
                        );
                        return;
                    }
                    next = Instant::now() + ENVIRONMENT_CHECK;
                }
            });
        }

        let mut closed_at = None;
        let mut output_ended_at = None;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if cancellation.check().is_err() {
                break None;
            }
            if input_closed.load(Ordering::Acquire) {
                let at = *closed_at.get_or_insert_with(Instant::now);
                if at.elapsed() > INPUT_CLOSED_GRACE {
                    break None;
                }
            }
            // A server that closed its output, or broke the protocol, is done.
            if reader.is_finished() {
                let at = *output_ended_at.get_or_insert_with(Instant::now);
                if at.elapsed() > STOP_GRACE {
                    break None;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        stopping.store(true, Ordering::Release);
        // Stop what the server left running too, such as `cargo check`.
        let stopped = process::stop_group(
            &mut child,
            if status.is_some() {
                Duration::ZERO
            } else {
                STOP_GRACE
            },
        );
        let status = status.or(stopped);
        // A process that left the group may still hold the server's output open.
        let deadline = Instant::now() + STOP_GRACE;
        while !reader.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let protocol = match reader.is_finished().then(|| reader.join()) {
            Some(Ok(Err(error))) if error.kind() == io::ErrorKind::InvalidData => {
                Some(error.to_string())
            }
            _ => None,
        };
        let stderr = stderr.finish(Duration::from_secs(1));
        let _ = notify(
            &output,
            "$/newport/server",
            json!({
                "state": "exited",
                "server": id,
                "status": status.and_then(|s| s.code()),
                "signal": status.and_then(|s| s.signal()),
                "stderr": String::from_utf8_lossy(&stderr),
                "error": protocol,
            }),
        );
        Ok(())
    })();
    if result.is_err() {
        // The desktop is gone, or the worker can't go on: don't leave the
        // server or the watcher running without it.
        stopping.store(true, Ordering::Release);
        process::stop_group(&mut child, Duration::ZERO);
    }
    result
}

fn stdout(cancellation: &Cancellation) -> io::Result<Output> {
    use std::os::fd::{AsFd, AsRawFd};
    let stdout = File::from(io::stdout().as_fd().try_clone_to_owned()?);
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Arc::new(Mutex::new(DeadlineWriter {
        inner: stdout,
        // Large responses (a full completion list) cross a slow link.
        timeout: Duration::from_secs(30),
        cancellation: cancellation.clone(),
    })))
}

fn notify<W: Write>(output: &Mutex<W>, method: &str, params: Value) -> io::Result<()> {
    let body = serde_json::to_vec(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
        .map_err(io::Error::other)?;
    output.lock().unwrap().write_all(&framing::encode(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_whole_lsp_messages() {
        let output = Mutex::new(Vec::new());
        notify(&output, "$/newport/server", json!({"state": "missing"})).unwrap();
        let bytes = output.into_inner().unwrap();
        let message = framing::read_message(&mut &bytes[..]).unwrap().unwrap();
        assert_eq!(message, bytes);
        let body = &bytes[bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4..];
        let value: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(value["method"], "$/newport/server");
        assert_eq!(value["params"]["state"], "missing");
    }

    #[test]
    fn requests_name_an_existing_folder_a_known_server_and_a_mode() {
        let refuse = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
            serve_bound(&args, Cancellation::default())
                .unwrap_err()
                .to_string()
        };
        assert!(refuse(&["relative", "gopls", "envrc"]).contains("doesn't exist"));
        assert!(refuse(&["/nonexistent-newport", "gopls", "envrc"]).contains("doesn't exist"));
        assert!(refuse(&["/", "sh", "envrc"]).contains("Unknown"));
        assert!(refuse(&["/", "gopls", "maybe"]).contains("Invalid"));
        assert!(refuse(&["/", "gopls"]).contains("Invalid"));
    }

    #[test]
    fn every_server_has_a_command() {
        for server in SERVERS {
            assert!(!server.commands.is_empty(), "{}", server.id);
            assert!(
                server
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'-')
            );
        }
    }
}
