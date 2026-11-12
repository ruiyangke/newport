//! Policy for speculative reads only. Explicit file opens are never filtered.
use std::collections::HashSet;

pub(super) fn relative<'a>(root: &str, path: &'a str) -> Option<&'a str> {
    let relative = path
        .strip_prefix(root.trim_end_matches('/'))?
        .strip_prefix('/')?;
    (!relative.is_empty()
        && !relative.contains('\0')
        && relative.split('/').all(|s| !matches!(s, "" | "." | "..")))
    .then_some(relative)
}
pub(super) fn candidate(root: &str, path: &str) -> bool {
    let Some(relative) = relative(root, path) else {
        return false;
    };
    let mut parts = relative.rsplit('/');
    let name = parts.next().unwrap().to_ascii_lowercase();
    if parts.any(|p| {
        matches!(
            p.to_ascii_lowercase().as_str(),
            ".git"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".kube"
                | ".gnupg"
                | ".gcloud"
                | ".config"
                | "node_modules"
                | "bower_components"
                | "vendor"
                | "target"
                | "dist"
                | "build"
                | "out"
                | "coverage"
                | ".venv"
                | "venv"
                | "env"
                | "__pycache__"
                | ".tox"
                | ".nox"
                | ".mypy_cache"
                | ".pytest_cache"
                | ".ruff_cache"
                | ".gradle"
                | ".m2"
                | ".ivy2"
                | ".sbt"
                | ".bloop"
                | ".build"
                | "pods"
                | "carthage"
                | "deriveddata"
                | ".dart_tool"
                | ".pub-cache"
                | ".terraform"
                | ".serverless"
                | ".next"
                | ".nuxt"
                | ".output"
                | ".svelte-kit"
                | ".cache"
                | ".parcel-cache"
                | ".turbo"
                | ".yarn"
                | ".pnpm-store"
                | ".cargo"
                | ".rustup"
                | ".bundle"
                | "_build"
                | "deps"
                | "_opam"
                | "_site"
                | ".stack-work"
                | "dist-newstyle"
                | "obj"
                | "bin"
                | "packages"
        )
    }) {
        return false;
    }
    if name.starts_with('.')
        || [
            "secret",
            "credential",
            "token",
            "private",
            "id_rsa",
            "id_ed25519",
            ".min.",
            ".bundle",
            ".generated.",
            ".g.",
        ]
        .iter()
        .any(|s| name.contains(s))
    {
        return false;
    }
    matches!(
        name.rsplit('.').next(),
        Some(
            "rs" | "go"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "py"
                | "rb"
                | "c"
                | "h"
                | "cpp"
                | "cs"
                | "java"
                | "kt"
                | "swift"
                | "dart"
                | "ex"
                | "exs"
                | "hs"
                | "ml"
                | "md"
                | "txt"
                | "toml"
                | "yaml"
                | "yml"
                | "css"
                | "html"
        )
    ) || matches!(name.as_str(), "makefile" | "dockerfile" | "readme")
}
// Git handles nested rules, negations, info/exclude, global excludes and tracked
// files. Paths use NUL-delimited stdin, never command interpolation. A failure
// disables prefetch; it must never silently fall back to ignoring Git's policy.
pub(super) fn command(root: &str) -> String {
    let quoted = format!("'{}'", root.replace('\'', "'\\''"));
    format!(
        "cd -- {quoted} || exit 2; git check-ignore -z --stdin; result=$?; case \"$result\" in 0|1) printf '\\0NEWPORT_IGNORE_OK\\0';; *) exit \"$result\";; esac"
    )
}
pub(super) fn allowed(root: &str, paths: Vec<String>, output: &str) -> Option<Vec<String>> {
    let ignored = output.strip_suffix("\0NEWPORT_IGNORE_OK\0")?;
    if !ignored.is_empty() && !ignored.ends_with('\0') {
        return None;
    }
    let ignored: HashSet<_> = ignored.split_terminator('\0').collect();
    Some(
        paths
            .into_iter()
            .filter(|p| relative(root, p).is_some_and(|r| !ignored.contains(r)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exclusions_apply_to_every_directory_but_not_project_name() {
        for dir in [
            "node_modules",
            "target",
            ".venv",
            "__pycache__",
            "vendor",
            "Pods",
            ".gradle",
            ".dart_tool",
            "deps",
            "dist",
            "obj",
            ".git",
        ] {
            assert!(
                !candidate("/repo", &format!("/repo/nested/{dir}/a.rs")),
                "{dir}"
            );
        }
        assert!(candidate("/work/target", "/work/target/src/main.rs"));
        for path in [
            "/repo2/a.rs",
            "/repo/../a.rs",
            "/repo/.env",
            "/repo/private.rs",
            "/repo/a.min.js",
            "/repo/x.png",
        ] {
            assert!(!candidate("/repo", path));
        }
        assert!(allowed("/repo", vec![], "fatal: not a repository").is_none());
    }
    // Runs the server's script through a local `sh` and Git, standing in for
    // the Linux host; Windows paths aren't server paths.
    #[cfg(unix)]
    #[test]
    fn git_rules_include_nested_global_and_info_excludes_with_safe_filenames() {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let root = std::env::temp_dir().join(format!("newport-ignore-'{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "*.js\n!keep.js\ntracked.rs\n").unwrap();
        std::fs::write(root.join("src/.gitignore"), "*.rs\n!keep.rs\n").unwrap();
        std::fs::write(root.join(".git/info/exclude"), "info.txt\n").unwrap();
        let global = root.join("global-ignore");
        std::fs::write(&global, "global.txt\n").unwrap();
        git(&["config", "core.excludesFile", global.to_str().unwrap()]);
        std::fs::write(root.join("tracked.rs"), "tracked").unwrap();
        git(&["add", "-f", "tracked.rs"]);
        let names = [
            "x.js",
            "keep.js",
            "src/a.rs",
            "src/keep.rs",
            "info.txt",
            "global.txt",
            "tracked.rs",
            "a b\nc.rs",
        ];
        let mut process = Command::new("sh")
            .arg("-c")
            .arg(command(root.to_str().unwrap()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        process
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{}\0", names.join("\0")).as_bytes())
            .unwrap();
        let output = process.wait_with_output().unwrap();
        assert!(output.status.success());
        let paths = names
            .iter()
            .map(|n| root.join(n).to_str().unwrap().to_owned())
            .collect();
        let result = allowed(
            root.to_str().unwrap(),
            paths,
            std::str::from_utf8(&output.stdout).unwrap(),
        )
        .unwrap();
        let expected: Vec<_> = ["keep.js", "src/keep.rs", "tracked.rs", "a b\nc.rs"]
            .iter()
            .map(|n| root.join(n).to_str().unwrap().to_owned())
            .collect();
        assert_eq!(result, expected);
        std::fs::remove_dir_all(root).unwrap();
    }
}
