//! SSH: what a server's link is built from, forwarding, and the command a
//! terminal runs.
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

/// What a Newport terminal runs: the login shell, started with Needle's
/// exports, so sharing works without any shell setup. The person's startup
/// files run after it and may add to them. `sh` runs it whatever the login
/// shell is. `routing` is this computer's integration routing id for the
/// server (`ServerSession::integration_session_id`): Needle records it as the
/// `terminal`'s Mac before the shell starts, so the shell's programs'
/// requests come here. With `display`, the shell names the account's
/// display as a held shell does, so raw commands open their windows on it.
pub fn terminal_command(
    routing: uuid::Uuid,
    terminal: Option<uuid::Uuid>,
    display: bool,
) -> String {
    terminal_command_named(
        &crate::terminal::holder_channel::COMPUTER,
        routing,
        terminal,
        display,
    )
}

/// As `terminal_command`, for the computer named `computer`.
fn terminal_command_named(
    computer: &str,
    routing: uuid::Uuid,
    terminal: Option<uuid::Uuid>,
    display: bool,
) -> String {
    let mut setup = String::new();
    if let Some(terminal) = terminal {
        // The route comes before `env`, which follows it. A failed route
        // never stops the shell. The name goes as hexadecimal UTF-8: fish,
        // a login shell this script passes through, reads backslashes inside
        // single quotes, so no quoting carries every name through both.
        let name: String = computer.bytes().map(|b| format!("{b:02x}")).collect();
        setup.push_str(&format!(
            "export NEWPORT_TERMINAL='{terminal}'; \
             \"$HOME/.local/bin/newport-agent\" terminal route '{terminal}' '{routing}' {name} \
             >/dev/null 2>&1; "
        ));
    }
    if display {
        // A failure leaves the shell as it is.
        setup.push_str(
            "if names=$(\"$HOME/.local/bin/newport-agent\" terminal display 2>/dev/null); then \
             eval \"$names\"; fi; unset names; ",
        );
    }
    let agent = "agent=\"$HOME/.local/bin/newport-agent\"; \
        if [ -x \"$agent\" ] && exports=$(\"$agent\" env --shell sh 2>/dev/null); then \
        eval \"$exports\"; fi; unset agent exports; ";
    let shell = match terminal {
        // Without the account's display, the shell runs under a clipboard
        // display of the terminal's own, so the server's clipboard programs
        // reach this computer. Needle runs the shell even when that display
        // can't start; without Needle, the shell runs as it is.
        Some(terminal) if !display => format!(
            "if [ -x \"$HOME/.local/bin/newport-agent\" ]; then \
             exec \"$HOME/.local/bin/newport-agent\" display --terminal '{terminal}' -- \"$SHELL\" -l; \
             fi; exec \"$SHELL\" -l"
        ),
        _ => "exec \"$SHELL\" -l".into(),
    };
    let script = format!("{setup}{agent}{shell}");
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

    /// The login shells a terminal's command must work under: sh, bash and
    /// zsh are required, so a missing one fails the test rather than passing
    /// it untested; fish runs where it is installed, and says when it isn't.
    pub(super) fn login_shells() -> Vec<&'static str> {
        let installed = |shell: &str| {
            std::process::Command::new(shell)
                .args(["-c", "exit 0"])
                .status()
                .is_ok_and(|status| status.success())
        };
        let mut shells = Vec::new();
        for shell in ["sh", "bash", "zsh"] {
            assert!(installed(shell), "{shell} is required for this test");
            shells.push(shell);
        }
        if installed("fish") {
            shells.push("fish");
        } else {
            eprintln!("fish isn't installed: its login-shell cases didn't run");
        }
        shells
    }

    /// As sshd runs it for an account whose login shell is `shell`.
    fn through_login_shell(shell: &str, command: &str) -> std::process::Output {
        std::process::Command::new(shell)
            .arg("-c")
            .arg(posix(command))
            .output()
            .unwrap()
    }

    #[test]
    fn commands_keep_posix_meaning_under_any_login_shell() {
        let empty = tempfile::tempdir().unwrap();
        let script = format!(
            "set -eu; n=0; for f in {}/*; do test -e \"$f\" && n=$((n+1)); done; printf '%s' \"$n\"",
            shell_quote(&empty.path().display().to_string())
        );
        for shell in login_shells() {
            let output = through_login_shell(shell, &script);
            assert!(output.status.success(), "{shell}: {output:?}");
            assert_eq!(output.stdout, b"0", "{shell}");
        }
    }
}

#[cfg(all(test, unix))]
mod terminal_gui_tests {
    use super::*;
    #[test]
    fn a_terminal_records_its_route_and_names_the_display_under_any_login_shell() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let shell = home.path().join("login-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\nprintf '%s\\n' \"$NEWPORT_TERMINAL\" \"${WAYLAND_DISPLAY-unset}\" \"$1\"\n",
        )
        .unwrap();
        // Needle, recording what it is asked to do, naming the display, and
        // running the shell under a clipboard display.
        let bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let agent = bin.join("newport-agent");
        std::fs::write(
            &agent,
            "#!/bin/sh\nprintf '%s|' \"$@\" >> \"$HOME/asked\"\necho >> \"$HOME/asked\"\n\
             if [ \"$2\" = display ]; then echo \"export WAYLAND_DISPLAY='newport-display/wayland-0'\"; fi\n\
             if [ \"$1\" = display ]; then shift 4; WAYLAND_DISPLAY=clipboard exec \"$@\"; fi\n",
        )
        .unwrap();
        for path in [&shell, &agent] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let routing = uuid::Uuid::new_v4();
        let terminal = uuid::Uuid::new_v4();
        // Every character a shell could read, as a computer may be named.
        let computer = "Ann's \\'Mac\\ \"$(touch HACKED)\" `id` $HOME é 工作室\\";
        // sshd runs it through the account's login shell, whichever it is.
        let run = |login: &str, display: bool| {
            let command = terminal_command_named(computer, routing, Some(terminal), display);
            std::process::Command::new(login)
                .args(["-c", &command])
                .current_dir(home.path())
                .env("HOME", home.path())
                .env("SHELL", &shell)
                .env_remove("WAYLAND_DISPLAY")
                .output()
                .unwrap()
        };
        for login in tests::login_shells() {
            for display in [false, true] {
                let result = run(login, display);
                assert!(result.status.success(), "{login}: {result:?}");
                let named = if display {
                    "newport-display/wayland-0"
                } else {
                    "clipboard"
                };
                assert_eq!(
                    String::from_utf8(result.stdout).unwrap(),
                    format!("{terminal}\n{named}\n-l\n"),
                    "{login}"
                );
                let asked = std::fs::read_to_string(home.path().join("asked")).unwrap();
                std::fs::remove_file(home.path().join("asked")).unwrap();
                let asked: Vec<_> = asked.lines().collect();
                let runner = format!("display|--terminal|{terminal}|--|{}|-l|", shell.display());
                let expected = if display {
                    vec!["terminal|display|", "env|--shell|sh|"]
                } else {
                    vec!["env|--shell|sh|", runner.as_str()]
                };
                assert_eq!(asked[1..], expected, "{login}");
                let route: Vec<_> = asked[0].split('|').collect();
                assert_eq!(
                    route[..4],
                    [
                        "terminal",
                        "route",
                        &terminal.to_string(),
                        &routing.to_string()
                    ],
                    "{login}"
                );
                let name: Vec<u8> = (0..route[4].len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&route[4][i..i + 2], 16).unwrap())
                    .collect();
                assert_eq!(String::from_utf8(name).unwrap(), computer, "{login}");
                assert!(!home.path().join("HACKED").exists(), "{login}");
            }
        }
        // A failing route, or failing to name the display, never stops the
        // shell.
        std::fs::write(&agent, "#!/bin/sh\necho failed >&2\nexit 1\n").unwrap();
        let result = run("sh", true);
        assert!(result.status.success());
        assert!(result.stderr.is_empty(), "{result:?}");
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            format!("{terminal}\nunset\n-l\n")
        );
    }

    /// Without the account's display, the shell runs under the terminal's
    /// clipboard display, in Needle's process, so job control, signals and
    /// the exit status stay the shell's. Without Needle it runs as it is.
    #[test]
    fn a_plain_terminals_shell_runs_under_its_clipboard_display_under_any_login_shell() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let shell = home.path().join("login-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\nprintf '%s\\n' \"${DISPLAY-unset}\" \"$1\"\nexit 3\n",
        )
        .unwrap();
        // Needle's runner, as far as the command sees it: the shell replaces
        // it, naming the display.
        let bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let agent = bin.join("newport-agent");
        std::fs::write(
            &agent,
            "#!/bin/sh\nif [ \"$1\" = display ]; then printf '%s|' \"$@\" > \"$HOME/asked\"; \
             shift 4; DISPLAY=:77 exec \"$@\"; fi\n",
        )
        .unwrap();
        for path in [&shell, &agent] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let terminal = uuid::Uuid::new_v4();
        let command = terminal_command_named("Studio", uuid::Uuid::new_v4(), Some(terminal), false);
        let run = |login: &str| {
            std::process::Command::new(login)
                .args(["-c", &command])
                .current_dir(home.path())
                .env("HOME", home.path())
                .env("SHELL", &shell)
                .env_remove("DISPLAY")
                .output()
        };
        for login in tests::login_shells() {
            let result = run(login).unwrap();
            assert_eq!(result.status.code(), Some(3), "{login}: {result:?}");
            assert_eq!(
                String::from_utf8(result.stdout).unwrap(),
                ":77\n-l\n",
                "{login}"
            );
            assert_eq!(
                std::fs::read_to_string(home.path().join("asked")).unwrap(),
                format!("display|--terminal|{terminal}|--|{}|-l|", shell.display()),
                "{login}"
            );
            std::fs::remove_file(home.path().join("asked")).unwrap();
        }
        // Without Needle, the shell starts directly.
        std::fs::remove_file(&agent).unwrap();
        let result = run("sh").unwrap();
        assert_eq!(result.status.code(), Some(3), "{result:?}");
        assert_eq!(String::from_utf8(result.stdout).unwrap(), "unset\n-l\n");
    }
}
