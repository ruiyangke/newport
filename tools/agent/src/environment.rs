//! Shell helper routes for the currently enabled integrations. Clipboard
//! protocol endpoints are private to helpers; only a GUI handoff supplies a
//! rendering display. Refresh removes only recorded clipboard endpoints and
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
    // Opt-in only: ordinary shell startup does not wait for GUI IPC. Reuse the
    // checked route, rather than reconciling to another session after checking.
    let selected = if check_gui == 1 {
        Ok(crate::gui_attachment::validate()?)
    } else {
        crate::routing::select(None)
    };
    let selection = match selected {
        Ok(selection) => selection,
        // Needle's commands still go on PATH: they find a session when run.
        Err(error) => {
            eprintln!("newport-agent: {error}");
            let output = render(&home, "", fish) + &clipboard_cleanup(fish) + &gui_exports(fish);
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
    let mut output = render(&home, &features, fish) + &clipboard_cleanup(fish);
    if let Some(id) = selection.session {
        output.push_str(&if fish {
            format!("set -gx NEWPORT_SESSION '{id}'\n")
        } else {
            format!("export NEWPORT_SESSION='{id}'\n")
        });
    }
    output.push_str(&gui_exports(fish));
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

// Preparing PATH does not wait for the integration transport to finish connecting.
fn gui_exports(fish: bool) -> String {
    // A ready graphical shell already has its display. Keep command lookup
    // native, including after refreshing a shell that inherited old shims.
    if crate::native::gui_display() {
        let home = env::var("HOME").unwrap_or_default();
        let directory = format!("{home}/.cache/newport/gui-bin");
        let mut output = without_gui_path(&directory, fish);
        // prepare() returns the canonical private directory. HOME itself may
        // use an alias (for example /var versus /private/var on macOS).
        if let Ok(canonical) = fs::canonicalize(&directory) {
            let canonical = canonical.to_string_lossy();
            if canonical != directory {
                output.push_str(&without_gui_path(&canonical, fish));
            }
        }
        return output;
    }
    let mut output = String::new();
    if env::var("NEWPORT_TERMINAL").is_ok_and(|id| uuid::Uuid::parse_str(&id).is_ok()) {
        if let Ok(directory) = crate::gui_launch::prepare() {
            let directory = directory.to_string_lossy();
            output.push_str(&gui_path(&directory, fish));
        }
    }
    output
}

fn without_gui_path(directory: &str, fish: bool) -> String {
    if fish {
        let quoted = format!("'{}'", directory.replace('\\', "\\\\").replace('\'', "\\'"));
        return format!(
            "set -l _newport_gui_keep\nfor _newport_gui_entry in $PATH\n\
             if test \"$_newport_gui_entry\" != {quoted}\n\
             set -a _newport_gui_keep \"$_newport_gui_entry\"\nend\nend\n\
             set -gx PATH $_newport_gui_keep\nset -e _newport_gui_keep\nset -e _newport_gui_entry\n"
        );
    }
    let quoted = format!("'{}'", directory.replace('\'', "'\\''"));
    // Do not change IFS or positional arguments in the user's shell. Keep
    // unrelated entries, duplicates and empty PATH components exactly as given.
    format!(
        "_newport_gui_rest=${{PATH-}}\n_newport_gui_keep=\n_newport_gui_sep=\n\
         while :; do\ncase $_newport_gui_rest in\n\
         *:*) _newport_gui_entry=${{_newport_gui_rest%%:*}}; _newport_gui_rest=${{_newport_gui_rest#*:}}; _newport_gui_more=1 ;;\n\
         *) _newport_gui_entry=$_newport_gui_rest; _newport_gui_more=0 ;;\nesac\n\
         if [ \"$_newport_gui_entry\" != {quoted} ]; then\n\
         _newport_gui_keep=\"$_newport_gui_keep$_newport_gui_sep$_newport_gui_entry\"; _newport_gui_sep=:\nfi\n\
         [ \"$_newport_gui_more\" = 1 ] || break\ndone\n\
         export PATH=$_newport_gui_keep\n\
         unset _newport_gui_rest _newport_gui_keep _newport_gui_sep _newport_gui_entry _newport_gui_more\n\
         hash -r 2>/dev/null || :\n"
    )
}

fn gui_path(directory: &str, fish: bool) -> String {
    let quoted = if fish {
        format!("'{}'", directory.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        format!("'{}'", directory.replace('\'', "'\\''"))
    };
    if fish {
        format!("if not contains -- {quoted} $PATH\nset -gx PATH {quoted} $PATH\nend\n")
    } else {
        // prepare() may add a launcher while its directory is already on PATH.
        // Forget native paths cached before that launcher existed. Uninstall
        // deletes launchers under open shells; bash alone keeps running a
        // deleted path it cached, unless it checks the cache first.
        format!(
            "case \":${{PATH-}}:\" in *\":\"{quoted}\":\"*) ;; *) export PATH={quoted}${{PATH:+:\"$PATH\"}} ;; esac\nhash -r 2>/dev/null || :\nif [ -n \"${{BASH_VERSION-}}\" ]; then shopt -s checkhash; fi\n"
        )
    }
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
    fn ready_display_removes_only_its_legacy_gui_path_without_changing_shell_arguments() {
        for shell in ["sh", "bash"] {
            check_gui_path_removal(shell);
        }
    }
    #[test]
    #[ignore = "Requires fish; run in the local shared-shell Docker fixture with --include-ignored"]
    fn fish_ready_display_removes_only_its_legacy_gui_path() {
        check_gui_path_removal("fish");
    }
    fn check_gui_path_removal(shell: &str) {
        let directory = "/tmp/gui ' $(exit 99)";
        let fish = shell == "fish";
        let refresh = without_gui_path(directory, fish);
        for (path, expected) in [
            (
                format!("/usr/bin:{directory}:/bin:{directory}"),
                "/usr/bin:/bin".to_owned(),
            ),
            (
                format!("{directory}/other:/bin:{directory}"),
                format!("{directory}/other:/bin"),
            ),
            (format!(":{directory}::/bin:"), "::/bin:".to_owned()),
            (directory.to_owned(), String::new()),
        ] {
            let source = if fish {
                format!("{refresh}\n{refresh}\nprintf '%s\\n' \"$PATH\" \"$argv[1]\"")
            } else {
                format!(
                    "IFS=sentinel\n{refresh}\n{refresh}\nprintf '%s\\n' \"$PATH\" \"$1\" \"$IFS\""
                )
            };
            let executable = if fish {
                "/usr/bin/fish"
            } else if shell == "bash" {
                "/bin/bash"
            } else {
                "/bin/sh"
            };
            let mut command = std::process::Command::new(executable);
            command.args([if fish { "--no-config" } else { "-eu" }, "-c", &source]);
            if !fish {
                command.arg("fixture");
            }
            let result = command
                .arg("original argument")
                .env("PATH", path)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            let tail = if fish { "" } else { "sentinel\n" };
            // Fish normalizes inherited empty PATH components to '.', before
            // any generated refresh runs. Preserve that native representation.
            let expected = if fish && !expected.is_empty() {
                expected
                    .split(':')
                    .map(|entry| if entry.is_empty() { "." } else { entry })
                    .collect::<Vec<_>>()
                    .join(":")
            } else {
                expected
            };
            assert_eq!(
                String::from_utf8(result.stdout).unwrap(),
                format!("{expected}\noriginal argument\n{tail}")
            );
        }
    }
    #[test]
    fn refreshed_launchers_replace_cached_native_commands() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let gui = root.path().join("gui");
        let native = root.path().join("native");
        fs::create_dir(&gui).unwrap();
        fs::create_dir(&native).unwrap();
        for (name, output) in [("cached-app", "native"), ("new-launcher", "launcher")] {
            let path = native.join(name);
            fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' {output}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let refresh = gui_path(gui.to_str().unwrap(), false);
        let script = format!(
            "PATH=\"$1:$2:/usr/bin:/bin\"\ncached-app\nmv \"$2/new-launcher\" \"$1/cached-app\"\n{refresh}\ncached-app"
        );
        let result = std::process::Command::new("bash")
            .args(["--noprofile", "--norc", "-eu", "-c", &script, "fixture"])
            .arg(gui)
            .arg(native)
            .output()
            .unwrap();
        assert!(result.status.success(), "{:?}", result);
        assert_eq!(result.stdout, b"native\nlauncher\n");
    }
    #[test]
    fn removed_launchers_fall_back_to_native_commands() {
        use std::os::unix::fs::PermissionsExt;
        for shell in ["bash", "sh"] {
            let root = tempfile::tempdir().unwrap();
            let gui = root.path().join("gui");
            let native = root.path().join("native");
            for (dir, output) in [(&gui, "launcher"), (&native, "native")] {
                fs::create_dir(dir).unwrap();
                let path = dir.join("app");
                fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' {output}\n")).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let refresh = gui_path(gui.to_str().unwrap(), false);
            let script = format!("PATH=\"$2:/usr/bin:/bin\"\n{refresh}\napp\nrm -r \"$1\"\napp");
            let result = std::process::Command::new(shell)
                .args(["-eu", "-c", &script, "fixture"])
                .arg(&gui)
                .arg(&native)
                .output()
                .unwrap();
            assert!(result.status.success(), "{shell}: {result:?}");
            assert_eq!(result.stdout, b"launcher\nnative\n", "{shell}");
        }
    }
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
