//! SSH: what a server's link is built from, forwarding, and the command a
//! terminal runs.
pub use crate::terminal::shell::Identity as TerminalIdentity;
mod auth;

/// Single-quote a word for a POSIX shell on the server.
pub(crate) fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// sshd runs an exec request through the account's login shell, which may be
/// zsh or fish. Newport's commands are POSIX sh, so they run under sh: zsh
/// aborts on a glob that matches nothing, and fish can't parse them at all.
/// The login shell's environment still reaches the command.
pub(crate) fn posix(command: &str) -> String {
    format!("exec sh -c {}", shell_quote(command))
}
#[cfg(test)]
mod agent_request_tests;
pub mod callback;
pub(crate) mod forward;
mod health;
mod link_parts;
#[cfg(test)]
mod link_tests;
pub use link_parts::link;
#[cfg(test)]
pub(crate) use link_parts::no_jump_hosts;
mod relay;
mod reverse;
pub(crate) mod terminal_startup;

/// What a Newport terminal runs: the login shell, started with Needle's
/// exports for this computer's session, so sharing works without any shell
/// setup. The person's startup files run after it and may add to them. `sh`
/// runs it whatever the login shell is, and the selector is a UUID from the
/// session owner, never shell input.
pub fn terminal_command(selector: uuid::Uuid, terminal: Option<TerminalIdentity>) -> String {
    terminal_command_with_identity(
        selector,
        &crate::terminal::display::Prepared::None,
        terminal,
    )
    .expect("A plain shell has no display to validate")
}
pub(crate) fn terminal_command_with_identity(
    selector: uuid::Uuid,
    gui: &crate::terminal::display::Prepared,
    terminal: Option<crate::terminal::shell::Identity>,
) -> std::result::Result<String, String> {
    terminal_command_preflight(selector, gui, terminal, None)
}
#[cfg(all(test, unix))]
pub(crate) fn checked_terminal_command(
    selector: uuid::Uuid,
    gui: &crate::terminal::display::Prepared,
) -> std::result::Result<String, String> {
    terminal_command_preflight(
        selector,
        gui,
        None,
        Some(&terminal_startup::Readiness::new()),
    )
}

pub(crate) fn terminal_command_preflight(
    selector: uuid::Uuid,
    gui: &crate::terminal::display::Prepared,
    terminal: Option<crate::terminal::shell::Identity>,
    readiness: Option<&terminal_startup::Readiness>,
) -> std::result::Result<String, String> {
    use crate::terminal::display::Prepared;
    use terminal_startup::Ack;
    let assignments = gui.assignments(selector)?;
    let mut setup = String::new();
    if let Some(identity) = terminal {
        setup.push_str(&format!(
            "export NEWPORT_TERMINAL='{}' NEWPORT_TERMINAL_EPOCH='{}'; ",
            identity.terminal, identity.epoch
        ));
    }
    let agent = "agent=\"$HOME/.local/bin/newport-agent\"; \
        if [ -x \"$agent\" ] && exports=$(\"$agent\" env --shell sh 2>/dev/null); then \
        eval \"$exports\"; fi; unset agent exports; ";
    let mut ready =
        readiness.map_or_else(String::new, |readiness| readiness.command(Ack::Unavailable));
    if let Prepared::Shared(lease) = gui {
        let value = |name: &str| {
            assignments
                .get(name)
                .and_then(Option::as_deref)
                .ok_or_else(|| format!("Missing shared display {name}"))
        };
        let socket = value("WAYLAND_DISPLAY")?;
        // GTK still requires a runtime directory when WAYLAND_DISPLAY is
        // absolute. Headless SSH logins may have none. Reuse the owned
        // socket's directory only in that case; preserve an existing user
        // runtime. Bus inheritance is selected separately below.
        let runtime = socket
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .filter(|directory| !directory.is_empty())
            .ok_or("Shared Wayland socket has no runtime directory")?;
        let environment = lease.environment()?;
        let terminal_bus = lease.terminal_bus()?;
        // Every owned endpoint is checked before any is exported. A missing
        // one leaves an ordinary shell; it never ends or delays the shell.
        let mut available = Vec::new();
        if let Some(authority) = assignments.get("XAUTHORITY").and_then(Option::as_deref) {
            available.push(format!("[ -r {} ]", shell_quote(authority)));
        }
        available.push(format!("[ -S {} ]", shell_quote(socket)));
        let mut exports = format!(
            "if [ -z \"${{XDG_RUNTIME_DIR:-}}\" ]; then export XDG_RUNTIME_DIR={}; fi; ",
            shell_quote(runtime)
        );
        if let Some(address) = terminal_bus.address() {
            // Explicit new-terminal policy, before the login profile runs.
            // Profiles and later command-level overrides remain final.
            available.push(format!(
                "[ -S {} ]",
                shell_quote(environment.fallback_bus_socket().unwrap())
            ));
            exports.push_str(&format!(
                "export DBUS_SESSION_BUS_ADDRESS={}; unset DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE DBUS_SESSION_BUS_PID DBUS_SESSION_BUS_WINDOWID; ",
                shell_quote(address)
            ));
        } else if let Some(address) = &environment.fallback_bus {
            available.push(format!(
                "{{ [ -n \"${{DBUS_SESSION_BUS_ADDRESS:-}}\" ] || [ -n \"${{DBUS_STARTER_ADDRESS:-}}\" ] || [ -S {} ]; }}",
                shell_quote(environment.fallback_bus_socket().unwrap())
            ));
            exports.push_str(&format!(
                "if [ -z \"${{DBUS_SESSION_BUS_ADDRESS:-}}\" ] && [ -z \"${{DBUS_STARTER_ADDRESS:-}}\" ]; then export DBUS_SESSION_BUS_ADDRESS={}; unset DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE; fi; ",
                shell_quote(address)
            ));
        }
        // An owned shared lease. Only fixed names are accepted; no PATH,
        // preload variables, or executable shell fragments cross this boundary.
        for (name, value) in &assignments {
            match value {
                Some(value) => exports.push_str(&format!("export {name}={}; ", shell_quote(value))),
                None => exports.push_str(&format!("unset {name}; ")),
            }
        }
        match readiness {
            None => setup.push_str(&format!(
                "if {}; then {exports}fi; ",
                available.join(" && ")
            )),
            Some(readiness) => {
                // A shell whose display fails the recheck below gets back exactly
                // the variables it started with.
                let mut names = assignments
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                names.extend([
                    "XDG_RUNTIME_DIR",
                    "DBUS_SESSION_BUS_ADDRESS",
                    "DBUS_STARTER_ADDRESS",
                    "DBUS_STARTER_BUS_TYPE",
                    "DBUS_SESSION_BUS_PID",
                    "DBUS_SESSION_BUS_WINDOWID",
                ]);
                let names = names.into_iter().collect::<Vec<_>>().join(" ");
                let save = format!(
                    r#"for newport_name in {names}; do eval "newport_set_$newport_name=\${{$newport_name+1}} newport_was_$newport_name=\${{$newport_name-}}"; done; "#
                );
                let restore = format!(
                    r#"for newport_name in {names}; do if eval "[ -n \"\${{newport_set_$newport_name-}}\" ]"; then eval "export $newport_name=\"\$newport_was_$newport_name\""; else unset "$newport_name"; fi; done; "#
                );
                setup.push_str(&format!(
                    "newport_gui=; if {}; then {save}{exports}newport_gui=1; fi; ",
                    available.join(" && ")
                ));
                // Recheck every owned endpoint immediately before acknowledgment,
                // after the agent environment command has completed. A display
                // without X11 has no X11 listener or authority file to check.
                let mut checks = vec![format!("[ -S {} ]", shell_quote(socket))];
                if let Some(display) = assignments.get("DISPLAY").and_then(Option::as_deref) {
                    let display = display
                        .strip_prefix(':')
                        .ok_or("Invalid shared X11 display")?;
                    checks.push(format!("[ -r {} ]", shell_quote(value("XAUTHORITY")?)));
                    checks.push(format!(
                        "[ -S {} ]",
                        shell_quote(&format!("/tmp/.X11-unix/X{display}"))
                    ));
                }
                if let Some(address) = &environment.fallback_bus {
                    checks.push(format!(
                        "{{ [ \"${{DBUS_SESSION_BUS_ADDRESS-}}\" != {} ] || [ -S {} ]; }}",
                        shell_quote(address),
                        shell_quote(environment.fallback_bus_socket().unwrap())
                    ));
                }
                ready = format!(
                    "if [ -n \"$newport_gui\" ] && {}; then {}else if [ -n \"$newport_gui\" ]; then {restore}{agent}fi; {}fi; unset newport_gui newport_name; ",
                    checks.join(" && "),
                    readiness.command(Ack::Ready),
                    readiness.command(Ack::Unavailable),
                );
            }
        }
    }
    Ok(terminal_script(selector, &setup, agent, &ready))
}

fn terminal_script(selector: uuid::Uuid, setup: &str, agent: &str, ready: &str) -> String {
    let script = format!(
        "NEWPORT_SESSION={selector}; export NEWPORT_SESSION; {setup}{agent}{ready}exec \"$SHELL\" -l"
    );
    format!("exec sh -c {}", shell_quote(&script))
}

#[cfg(test)]
mod integration_tests {
    use crate::model::{AuthMethod, Server};
    use uuid::Uuid;
    /// The disposable SSH fixture's account.
    pub(super) fn server() -> Server {
        Server {
            id: Uuid::new_v4(),
            name: "Fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("NEWPORT_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: true,
            setup_complete: true,
            proxy: None,
        }
    }
}

// Servers are Linux; on Windows `bash` may be the WSL launcher, not a shell.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// As sshd runs it for an account whose login shell is `shell`.
    fn through_login_shell(shell: &str, command: &str) -> Option<std::process::Output> {
        std::process::Command::new(shell)
            .arg("-c")
            .arg(posix(command))
            .output()
            .ok()
    }

    #[test]
    fn commands_keep_posix_meaning_under_any_login_shell() {
        let empty = tempfile::tempdir().unwrap();
        let script = format!(
            "set -eu; n=0; for f in {}/*; do test -e \"$f\" && n=$((n+1)); done; printf '%s' \"$n\"",
            shell_quote(&empty.path().display().to_string())
        );
        for shell in ["sh", "bash", "zsh", "fish"] {
            let Some(output) = through_login_shell(shell, &script) else {
                continue;
            };
            assert!(output.status.success(), "{shell}: {output:?}");
            assert_eq!(output.stdout, b"0", "{shell}");
        }
    }
}

#[cfg(all(test, unix))]
mod terminal_gui_tests {
    use super::*;
    #[test]
    fn terminal_identity_reaches_the_login_shell_without_gui_setup() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let shell = home.path().join("login-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\nprintf '%s\\n' \"$NEWPORT_SESSION\" \"$NEWPORT_TERMINAL\" \"$NEWPORT_TERMINAL_EPOCH\" \"$1\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        let routing = uuid::Uuid::new_v4();
        let terminal = uuid::Uuid::new_v4();
        let identity = crate::terminal::shell::Identity::new(terminal);
        let result = std::process::Command::new("sh")
            .args([
                "-c",
                &terminal_command_with_identity(
                    routing,
                    &crate::terminal::display::Prepared::None,
                    Some(identity),
                )
                .unwrap(),
            ])
            .env("HOME", home.path())
            .env("SHELL", shell)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            format!("{routing}\n{terminal}\n{}\n-l\n", identity.epoch)
        );
    }
}
