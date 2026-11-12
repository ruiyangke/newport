//! Shell helper routes for the currently enabled integrations. Clipboard
//! protocol endpoints are private to helpers; a terminal names the account's
//! display itself. Refresh removes only recorded clipboard endpoints and
//! preserves genuine host displays and the existing shared-GUI markers.
use std::{
    env, fs,
    io::{self, Write},
};

pub fn print(args: &[String]) -> io::Result<()> {
    let check_gui = args.iter().filter(|arg| *arg == "--check-gui").count();
    let fish = match args
        .iter()
        .filter(|arg| *arg != "--check-gui")
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["--shell", "bash" | "zsh" | "sh"] => false,
        ["--shell", "fish"] => true,
        _ => {
            return Err(io::Error::other(
                "Usage: newport-agent env [--check-gui] [--shell bash|zsh|sh|fish]",
            ));
        }
    };
    if check_gui > 1 {
        return Err(io::Error::other("Specify --check-gui only once"));
    }
    let home = env::var("HOME").map_err(io::Error::other)?;
    // A terminal's Mac can change, and the stable display is shown on one at
    // a time, so neither is fixed here: each request picks one when it is
    // made. `BROWSER` is always set; whether a Mac takes
    // links is checked when one opens.
    let follows = match crate::routing::Follow::current() {
        Some(crate::routing::Follow::Viewer(_)) => true,
        Some(crate::routing::Follow::Terminal(_)) => !crate::native::gui_display(),
        None => false,
    };
    // Opt-in only: ordinary shell startup does not wait for GUI checks.
    if check_gui == 1 {
        crate::gui_attachment::validate()?;
    }
    if follows {
        let output = render(&home, "browser", fish) + &clipboard_cleanup(fish);
        return io::stdout().lock().write_all(output.as_bytes());
    }
    let selection = match crate::routing::select(None) {
        Ok(selection) => selection,
        // Needle's commands still go on PATH: they find a Mac when run.
        Err(error) => {
            eprintln!("newport-agent: {error}");
            let output = render(&home, "", fish) + &clipboard_cleanup(fish);
            return io::stdout().lock().write_all(output.as_bytes());
        }
    };
    let root = selection.root;
    let features = match fs::read_to_string(root.join("features")) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    // Generate everything before writing: callers must never evaluate partial output.
    let output = render(&home, &features, fish) + &clipboard_cleanup(fish);
    io::stdout().lock().write_all(output.as_bytes())
}

fn clipboard_cleanup(fish: bool) -> String {
    let endpoints = crate::native::clipboard_environment();
    let mut output = String::new();
    let mut clear = |values: &[(&str, &str)]| {
        let quoted = |value: &str| {
            if fish {
                format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
            } else {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
        };
        let guards = values
            .iter()
            .map(|(name, value)| {
                if fish {
                    format!("test \"${name}\" = {}", quoted(value))
                } else {
                    format!("[ \"${{{name}-}}\" = {} ]", quoted(value))
                }
            })
            .collect::<Vec<_>>()
            .join(if fish { "; and " } else { " && " });
        let names = values.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        if fish {
            output.push_str(&format!("if {guards}\n"));
            for name in names {
                output.push_str(&format!("set -e {name}\n"));
            }
            output.push_str("end\n");
        } else {
            output.push_str(&format!(
                "if {guards}; then unset {}; fi\n",
                names.join(" ")
            ));
        }
    };
    if let Some((display, authority)) = endpoints.x11 {
        clear(&[("DISPLAY", &display), ("XAUTHORITY", &authority)]);
    }
    if let Some(wayland) = endpoints.wayland {
        clear(&[("WAYLAND_DISPLAY", &wayland)]);
    }
    output
}

fn render(home: &str, features: &str, fish: bool) -> String {
    let quote = |s: &str| {
        if fish {
            format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
        } else {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
    };
    let bin = quote(&format!("{home}/.local/bin"));
    let mut output = if fish {
        format!("if not contains -- {bin} $PATH\n    set -gx PATH {bin} $PATH\nend\n")
    } else {
        format!(
            "case \":${{PATH-}}:\" in\n    *\":\"{bin}\":\"*) ;;\n    *) export PATH={bin}${{PATH:+:\"$PATH\"}} ;;\nesac\n"
        )
    };
    let export = |name: &str, value: &str| {
        format!(
            "{} {name}{}{}\n",
            if fish { "set -gx" } else { "export" },
            if fish { " " } else { "=" },
            quote(value)
        )
    };
    if features.split_whitespace().any(|f| f == "browser") {
        output.push_str(&export(
            "BROWSER",
            &format!("{home}/.local/bin/newport-browser"),
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_output_is_quoted_and_path_is_idempotent() {
        let home = "/tmp/test ' $(exit 99)";
        let script = render(home, "browser clipboard", false);
        let result = std::process::Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-eu",
                "-c",
                &format!("PATH=/usr/bin\n{script}\n{script}\nprintf '%s\\n' \"$PATH\" \"$BROWSER\" \"${{DISPLAY-unset}}\""),
            ])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XAUTHORITY")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout).unwrap();
        assert_eq!(
            output,
            format!("{home}/.local/bin:/usr/bin\n{home}/.local/bin/newport-browser\nunset\n")
        );
    }
    #[test]
    fn graphical_terminal_keeps_display_when_shell_setup_repeats_or_session_is_stale() {
        for features in ["browser clipboard", "browser"] {
            let script = render("/tmp/test", features, false);
            let output = std::process::Command::new("sh").args(["-eu", "-c", &format!("{script}\n{script}\nprintf '%s\\n' \"$DISPLAY\" \"$XAUTHORITY\" \"${{WAYLAND_DISPLAY-unset}}\" \"$BROWSER\"")])
                .env("DISPLAY", ":75").env("XAUTHORITY", "/tmp/newport-gui/Xauthority")
                .env("NEWPORT_GUI_DISPLAY", ":75").env("NEWPORT_GUI_AUTHORITY", "/tmp/newport-gui/Xauthority")
                .env_remove("WAYLAND_DISPLAY").output().unwrap();
            assert!(output.status.success(), "{:?}", output);
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                ":75\n/tmp/newport-gui/Xauthority\nunset\n/tmp/test/.local/bin/newport-browser\n"
            );
        }
    }
    #[test]
    fn wayland_graphical_terminal_keeps_its_display_on_refresh() {
        for features in ["clipboard", ""] {
            let script = render("/tmp/test", features, false);
            let output = std::process::Command::new("sh")
                .args(["-eu", "-c", &format!("{script}\n{script}\nprintf '%s\\n' \"${{DISPLAY-unset}}\" \"${{XAUTHORITY-unset}}\" \"$WAYLAND_DISPLAY\" \"$XDG_RUNTIME_DIR\"")])
                .env_remove("DISPLAY").env_remove("XAUTHORITY").env_remove("WAYLAND_SOCKET")
                .env("WAYLAND_DISPLAY", "wayland-0-newport-owned")
                .env("XDG_RUNTIME_DIR", "/tmp/newport-gui")
                .env("NEWPORT_GUI_WAYLAND_DISPLAY", "wayland-0-newport-owned")
                .env("NEWPORT_GUI_RUNTIME_DIR", "/tmp/newport-gui")
                .output().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                output.stdout,
                b"unset\nunset\nwayland-0-newport-owned\n/tmp/newport-gui\n"
            );
        }
    }
    #[test]
    fn absolute_wayland_graphical_terminal_keeps_user_runtime_and_bus_on_refresh() {
        for shell in ["sh", "bash"] {
            check_wayland_refresh(shell);
        }
    }
    #[test]
    #[ignore = "Requires fish; run in the local shared-shell Docker fixture with --include-ignored"]
    fn fish_absolute_wayland_graphical_terminal_keeps_user_runtime_and_bus_on_refresh() {
        check_wayland_refresh("fish");
    }
    fn check_wayland_refresh(shell: &str) {
        let fish = shell == "fish";
        for features in ["clipboard", ""] {
            let absolute = "/tmp/owned/wayland-0";
            let relative = "wayland-0";
            for (display, marker, runtime, runtime_marker, fd, _owned) in [
                (absolute, absolute, None, None, None, true),
                (absolute, absolute, Some("/run/user/1000"), None, None, true),
                (
                    absolute,
                    absolute,
                    Some("/run/user/1000"),
                    Some("/tmp/other"),
                    None,
                    true,
                ),
                (
                    relative,
                    relative,
                    Some("/run/user/1000"),
                    Some("/run/user/1000"),
                    None,
                    true,
                ),
                (
                    relative,
                    relative,
                    Some("/run/user/1000"),
                    Some("/tmp/other"),
                    None,
                    false,
                ),
                (
                    absolute,
                    "/tmp/other/wayland-0",
                    Some("/run/user/1000"),
                    None,
                    None,
                    false,
                ),
                (
                    absolute,
                    absolute,
                    Some("/run/user/1000"),
                    None,
                    Some("99"),
                    false,
                ),
            ] {
                let script = render("/tmp/test", features, fish);
                let print = if fish {
                    "set -q DISPLAY; or set DISPLAY unset\nset -q XAUTHORITY; or set XAUTHORITY unset\nset -q XDG_RUNTIME_DIR; or set XDG_RUNTIME_DIR unset\nprintf '%s\\n' \"$DISPLAY\" \"$XAUTHORITY\" \"$WAYLAND_DISPLAY\" \"$XDG_RUNTIME_DIR\" \"$DBUS_SESSION_BUS_ADDRESS\""
                } else {
                    "printf '%s\\n' \"${DISPLAY-unset}\" \"${XAUTHORITY-unset}\" \"$WAYLAND_DISPLAY\" \"${XDG_RUNTIME_DIR-unset}\" \"$DBUS_SESSION_BUS_ADDRESS\""
                };
                let mut command = std::process::Command::new(shell);
                command
                    .args([
                        if fish { "--no-config" } else { "-eu" },
                        "-c",
                        &format!("{script}\n{script}\n{print}"),
                    ])
                    .env_remove("DISPLAY")
                    .env_remove("XAUTHORITY")
                    .env_remove("WAYLAND_SOCKET")
                    .env_remove("NEWPORT_GUI_RUNTIME_DIR")
                    .env("WAYLAND_DISPLAY", display)
                    .env("NEWPORT_GUI_WAYLAND_DISPLAY", marker)
                    .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus");
                if let Some(runtime) = runtime {
                    command.env("XDG_RUNTIME_DIR", runtime);
                } else {
                    command.env_remove("XDG_RUNTIME_DIR");
                }
                if let Some(marker) = runtime_marker {
                    command.env("NEWPORT_GUI_RUNTIME_DIR", marker);
                }
                if let Some(fd) = fd {
                    command.env("WAYLAND_SOCKET", fd);
                }
                let output = command.output().unwrap();
                assert!(output.status.success(), "{output:?}");
                let display = format!("unset\nunset\n{display}");
                assert_eq!(
                    String::from_utf8(output.stdout).unwrap(),
                    format!(
                        "{display}\n{}\nunix:path=/run/user/1000/bus\n",
                        runtime.unwrap_or("unset"),
                    ),
                    "shell={shell} features={features} marker={marker} runtime={runtime:?} runtime_marker={runtime_marker:?} fd={fd:?}"
                );
            }
        }
    }
}
