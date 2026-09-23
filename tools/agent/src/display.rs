//! Standalone X11 or Wayland display launcher.
use crate::{wl_server, x_server};
use std::{
    env, io,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    X11,
    Wayland,
}

/// Run the display subcommand, returning the wrapped process's exit status.
pub fn run(args: impl Iterator<Item = std::ffi::OsString>) -> io::Result<i32> {
    let mut backend = Backend::X11;
    let mut args = args;
    let mut display = None;
    let mut service = false;
    let mut socket = None;
    let mut command = Vec::new();
    let mut snapshot = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("HOME is required"))?
        .join(".cache/porthop/clipboard/snapshot.tar");
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--service") => service = true,
            Some("--socket") => {
                socket = Some(
                    args.next()
                        .map(PathBuf::from)
                        .ok_or_else(|| io::Error::other("--socket needs a path"))?,
                );
            }
            Some("--snapshot") => {
                snapshot = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| io::Error::other("--snapshot needs a path"))?
            }
            Some("--backend") => {
                backend = match args.next().as_deref().and_then(|arg| arg.to_str()) {
                    Some("x11") => Backend::X11,
                    Some("wayland") => Backend::Wayland,
                    _ => return Err(io::Error::other("--backend must be x11 or wayland")),
                };
            }
            Some("--display") => {
                display = Some(
                    args.next()
                        .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
                        .filter(|n| *n > 0)
                        .ok_or_else(|| {
                            io::Error::other("--display needs a positive display number")
                        })?,
                )
            }
            Some("--") => {
                command.extend(args);
                break;
            }
            Some("--help") => {
                println!("Usage: porthop-agent display [--backend x11|wayland] [--snapshot PATH] [--display NUMBER] [--service [--socket PATH]] [-- COMMAND ...]\n\nStarts a clipboard-only display. The default backend is x11.\n--service starts Wayland at the fixed socket; omit it when Porthop already manages the agent.");
                return Ok(0);
            }
            _ => return Err(io::Error::other("unknown argument; use --help")),
        }
    }
    if backend == Backend::Wayland && display.is_some() {
        return Err(io::Error::other(
            "--display is only supported by the x11 backend",
        ));
    }
    if service && (backend != Backend::Wayland || !command.is_empty()) {
        return Err(io::Error::other(
            "--service requires Wayland and cannot launch a command",
        ));
    }
    if socket.is_some() && !service {
        return Err(io::Error::other("--socket requires --service"));
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::Relaxed);
    })
    .map_err(io::Error::other)?;
    let bridge = if backend == Backend::Wayland {
        Bridge::Wayland(if service {
            let socket = socket.unwrap_or(
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or_else(|| io::Error::other("HOME is required"))?
                    .join(".cache/porthop/clipboard/wayland.sock"),
            );
            wl_server::Server::start_fixed(&snapshot, &socket)?
        } else {
            wl_server::Server::start(&snapshot)?
        })
    } else {
        Bridge::X11(x_server::Server::start(&snapshot, display)?)
    };
    let (mut variables, removed) = bridge.environment();
    variables.push(("PORTHOP_OPEN_ON_MAC", "1".into()));
    if command.is_empty() {
        for (key, value) in &variables {
            println!("export {key}='{}'", value.replace('\'', "'\\''"));
        }
        println!("unset {}", removed.join(" "));
        while !stopped.load(Ordering::Relaxed) {
            if let Bridge::Wayland(server) = &bridge {
                if !server.is_running() {
                    return Err(io::Error::other("Wayland service stopped unexpectedly"));
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(0)
    } else {
        let mut launcher = Command::new(&command[0]);
        launcher.args(&command[1..]).envs(variables);
        for variable in removed {
            launcher.env_remove(variable);
        }
        let mut child = launcher.spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status.code().unwrap_or(1));
            }
            if stopped.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(130);
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

enum Bridge {
    X11(x_server::Server),
    Wayland(wl_server::Server),
}
type Environment = (Vec<(&'static str, String)>, Vec<&'static str>);
impl Bridge {
    fn environment(&self) -> Environment {
        match self {
            Self::X11(server) => (
                vec![
                    ("DISPLAY", server.display.clone()),
                    ("XAUTHORITY", server.authority().display().to_string()),
                ],
                vec!["WAYLAND_DISPLAY", "WAYLAND_SOCKET"],
            ),
            Self::Wayland(server) => (
                vec![("WAYLAND_DISPLAY", server.socket().display().to_string())],
                vec!["DISPLAY", "XAUTHORITY", "WAYLAND_SOCKET"],
            ),
        }
    }
}
