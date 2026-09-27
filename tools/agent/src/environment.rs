//! Shell exports for the currently enabled integrations.
use crate::paths::root;
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
                "Usage: porthop-agent env [--shell bash|zsh|sh|fish]",
            ))
        }
    };
    let root = root()?;
    let home = env::var("HOME").map_err(io::Error::other)?;
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
    // Generate everything before writing: callers must never evaluate partial output.
    let output = render(&home, &root, &features, display.as_deref(), fish);
    io::stdout().lock().write_all(output.as_bytes())
}

fn render(home: &str, root: &Path, features: &str, display: Option<&str>, fish: bool) -> String {
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
            &format!("{home}/.local/bin/porthop-browser"),
        ));
    }
    if features.split_whitespace().any(|f| f == "clipboard") {
        let socket = root.join("wayland.sock").to_string_lossy().into_owned();
        let authority = root.join("Xauthority").to_string_lossy().into_owned();
        // Do not redirect applications in an existing graphical desktop session.
        // Existing Porthop shells may refresh their display after reconnection.
        output.push_str(&if fish {
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
            "set -e WAYLAND_SOCKET\nend\n"
        } else {
            "unset WAYLAND_SOCKET\nfi\n"
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_output_is_quoted_and_path_is_idempotent() {
        let home = "/tmp/test ' $(exit 99)";
        let script = render(
            home,
            Path::new("/tmp/clip"),
            "browser clipboard",
            Some(":99"),
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
            format!("{home}/.local/bin:/usr/bin\n{home}/.local/bin/porthop-browser\n:99\n")
        );
    }
}
