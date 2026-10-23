//! Shell exports for the currently enabled integrations. A shell may keep
//! them for months (tmux), so every value stays valid across sessions:
//! commands that resolve the live session when they run, the session ID the
//! desktop derives from its profile and the server, and display paths
//! through the desktop's alias.
use std::{
    env, fs,
    io::{self, Write},
    path::Path,
};

pub fn print(args: &[String]) -> io::Result<()> {
    let fish = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["--shell", "bash" | "zsh" | "sh"] => false,
        ["--shell", "fish"] => true,
        _ => {
            return Err(io::Error::other(
                "Usage: newport-agent env [--shell bash|zsh|sh|fish]",
            ))
        }
    };
    let home = env::var("HOME").map_err(io::Error::other)?;
    let selection = match crate::routing::select(None) {
        Ok(selection) => selection,
        // Needle's commands still go on PATH: they find a session when run.
        Err(error) => {
            eprintln!("newport-agent: {error}");
            let output = render(&home, Path::new(""), "", None, false, fish) + &gui_exports(fish);
            return io::stdout().lock().write_all(output.as_bytes());
        }
    };
    let root = selection.root;
    let features = match fs::read_to_string(root.join("features")) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let display = match fs::read_to_string(root.join("display")) {
        Ok(value) => Some(value),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    // Display paths through the desktop's alias outlive this session.
    let shown = fs::read_to_string(root.join("agent-client"))
        .ok()
        .and_then(|client| crate::paths::client_alias(&root, client.trim()))
        .filter(|alias| fs::canonicalize(alias).ok() == fs::canonicalize(&root).ok())
        .unwrap_or_else(|| root.clone());
    // A shell still holding another Newport session's display takes this one.
    let stale = ["XAUTHORITY", "WAYLAND_DISPLAY"].iter().any(|name| {
        env::var(name).is_ok_and(|value| {
            value.starts_with("/tmp/newport-runtime-")
                || crate::paths::integration_display_path(&value)
        })
    });
    // Generate everything before writing: callers must never evaluate partial output.
    let mut output = render(&home, &shown, &features, display.as_deref(), stale, fish);
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

// Preparing PATH does not wait for the integration transport to finish connecting.
fn gui_exports(fish: bool) -> String {
    let mut output = String::new();
    if env::var("NEWPORT_TERMINAL").is_ok_and(|id| uuid::Uuid::parse_str(&id).is_ok()) {
        if let Ok(directory) = crate::gui_launch::prepare() {
            let directory = directory.to_string_lossy();
            output.push_str(&gui_path(&directory, fish));
        }
    }
    output
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
        // Forget native paths cached before that launcher existed.
        format!("case \":${{PATH-}}:\" in *\":\"{quoted}\":\"*) ;; *) export PATH={quoted}${{PATH:+:\"$PATH\"}} ;; esac\nhash -r 2>/dev/null || :\n")
    }
}

fn render(
    home: &str,
    root: &Path,
    features: &str,
    display: Option<&str>,
    stale: bool,
    fish: bool,
) -> String {
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
        format!("case \":${{PATH-}}:\" in\n    *\":\"{bin}\":\"*) ;;\n    *) export PATH={bin}${{PATH:+:\"$PATH\"}} ;;\nesac\n")
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
    if features
        .split_whitespace()
        .any(|f| f == "clipboard" || f == "gui")
    {
        let socket = root.join("wayland.sock").to_string_lossy().into_owned();
        let authority = root.join("Xauthority").to_string_lossy().into_owned();
        // Do not redirect applications in an existing graphical desktop session.
        // Existing Newport shells may refresh their display after reconnection.
        let gui_guard = if fish {
            "if set -q DISPLAY[1]; and set -q XAUTHORITY[1]; and test \"$NEWPORT_GUI_DISPLAY\" = \"$DISPLAY\"; and test \"$NEWPORT_GUI_AUTHORITY\" = \"$XAUTHORITY\"\n"
        } else {
            "if [ -n \"${DISPLAY-}\" ] && [ -n \"${XAUTHORITY-}\" ] && [ \"${NEWPORT_GUI_DISPLAY-}\" = \"$DISPLAY\" ] && [ \"${NEWPORT_GUI_AUTHORITY-}\" = \"$XAUTHORITY\" ]; then\n"
        };
        output.push_str(gui_guard);
        output.push_str(if fish { "true\nelse\n" } else { ":\nelse\n" });
        output.push_str(&if stale {
            if fish { "if true\n" } else { "if true; then\n" }.to_owned()
        } else if fish {
            format!("if begin; not set -q DISPLAY[1]; and not set -q WAYLAND_DISPLAY[1]; end; or test \"$XAUTHORITY\" = {}; or test \"$WAYLAND_DISPLAY\" = {}\n", quote(&authority), quote(&socket))
        } else {
            format!("if {{ [ -z \"${{DISPLAY-}}\" ] && [ -z \"${{WAYLAND_DISPLAY-}}\" ]; }} || [ \"${{XAUTHORITY-}}\" = {} ] || [ \"${{WAYLAND_DISPLAY-}}\" = {} ]; then\n", quote(&authority), quote(&socket))
        });
        output.push_str(&export("WAYLAND_DISPLAY", &socket));
        if let Some(display) = display {
            output.push_str(&export("DISPLAY", display.trim()));
            output.push_str(&export("XAUTHORITY", &authority));
        }
        output.push_str(if fish {
            "set -e WAYLAND_SOCKET\nend\nend\n"
        } else {
            "unset WAYLAND_SOCKET\nfi\nfi\n"
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn shell_output_is_quoted_and_path_is_idempotent() {
        let home = "/tmp/test ' $(exit 99)";
        let script = render(
            home,
            Path::new("/tmp/clip"),
            "browser clipboard",
            Some(":99"),
            false,
            false,
        );
        let result = std::process::Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-eu",
                "-c",
                &format!("PATH=/usr/bin\n{script}\n{script}\nprintf '%s\\n' \"$PATH\" \"$BROWSER\" \"$DISPLAY\""),
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
            format!("{home}/.local/bin:/usr/bin\n{home}/.local/bin/newport-browser\n:99\n")
        );
    }
    #[test]
    fn graphical_terminal_keeps_display_when_shell_setup_repeats_or_session_is_stale() {
        for stale in [false, true] {
            let script = render(
                "/tmp/test",
                Path::new("/tmp/newport-clipboard"),
                "browser clipboard",
                Some(":99"),
                stale,
                false,
            );
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
}
