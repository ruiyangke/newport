//! Read existing editor settings on the remote host. Native project configs
//! stay with the language server, which owns their discovery and precedence.
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

const VSCODE: &str = ".vscode/settings.json";
const ZED: &str = ".zed/settings.json";
const MAX_BYTES: u64 = 256 * 1024;

fn read(root: &Path, relative: &str) -> io::Result<Value> {
    let path = root.join(relative);
    let canonical = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(io::Error::other(format!("Cannot read {relative}: {error}"))),
    };
    if !canonical.starts_with(root.canonicalize()?) {
        return Err(io::Error::other(format!(
            "{relative} points outside the worktree."
        )));
    }
    // Non-blocking, so a FIFO or device there can't hold up the open.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(canonical)
        .map_err(|e| io::Error::other(format!("Cannot read {relative}: {e}")))?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(format!(
            "{relative} isn't a regular file."
        )));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io::Error::other(format!("Cannot read {relative}: {e}")))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other(format!("{relative} exceeds 256 KiB.")));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| io::Error::other(format!("Invalid {relative}: {e}")))?
        .trim_start_matches('\u{feff}');
    // Editors create the file empty before anything is set.
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    // JSONC, not JSON5: comments and trailing commas are the only extensions.
    let options = jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    };
    let value: Value = jsonc_parser::parse_to_serde_value(text, &options)
        .map_err(|e| io::Error::other(format!("Invalid {relative}: {e}")))?;
    if !value.is_object() {
        return Err(io::Error::other(format!(
            "{relative} must contain a settings object."
        )));
    }
    Ok(value)
}

fn merge(target: &mut Value, source: Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source {
            merge(
                target.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
            );
        }
    } else {
        *target = source;
    }
}

/// Expand setting IDs, but never dictionary keys inside values (environment
/// variables, diagnostic names, gopls' dotted option names, etc.).
fn insert_setting(target: &mut Value, key: &str, value: Value) -> io::Result<()> {
    let components: Vec<_> = key.split('.').collect();
    if components.len() > 64 || components.iter().any(|part| part.is_empty()) {
        return Err(io::Error::other(format!("Invalid setting name: {key}")));
    }
    let mut current = target;
    for component in components {
        if !current.is_object() {
            *current = json!({});
        }
        current = current
            .as_object_mut()
            .unwrap()
            .entry(component)
            .or_insert(Value::Null);
    }
    merge(current, value);
    Ok(())
}

fn section(settings: &Value, name: &str) -> io::Result<Value> {
    let mut result = settings.get(name).cloned().unwrap_or_else(|| json!({}));
    if !result.is_object() {
        return Err(io::Error::other(format!(
            "{VSCODE}: {name} must be an object."
        )));
    }
    let prefix = format!("{name}.");
    for (key, value) in settings.as_object().unwrap() {
        if let Some(key) = key.strip_prefix(&prefix) {
            insert_setting(&mut result, key, value.clone())
                .map_err(|e| io::Error::other(format!("{VSCODE}: {e}")))?;
        }
    }
    Ok(result)
}

fn vscode(settings: &Value, server: &str) -> io::Result<Value> {
    let mut result = json!({});
    match server {
        "rust-analyzer" | "gopls" => {
            result[server] = section(settings, server)?;
        }
        "pyright" | "basedpyright" => {
            result["python"] = section(settings, "python")?;
            result[server] = section(settings, server)?;
            if server == "basedpyright" {
                // Newport prefers basedpyright when installed. Preserve common
                // Pyright/Pylance analysis settings on that fallback path.
                let mut fallback = section(settings, "pyright")?;
                if let Some(analysis) = result["python"].get("analysis") {
                    fallback["analysis"] = analysis.clone();
                }
                merge(&mut fallback, result[server].take());
                result[server] = fallback;
            }
        }
        "typescript-language-server" => {
            // These have the same meaning in VS Code and typescript-language-server.
            // VS Code's completion preferences require extension-specific adapters;
            // raw initialization_options in Zed can set the server's preferences.
            for language in ["typescript", "javascript"] {
                let format = section(settings, &format!("{language}.format"))?;
                result[language] = json!({"format":format});
            }
        }
        _ => {}
    }
    Ok(result)
}

/// The configuration to start `server` with. Editor settings that can't be
/// used leave it at its defaults, with the problem as `settingsError`.
pub(super) fn resolve(root: &Path, server: &str) -> Value {
    load(root, server).unwrap_or_else(|error| {
        let mut config = configure(&json!({}), &json!({}), server).expect("default settings");
        config["settingsError"] = json!(format!(
            "{error} The language server started with its default settings."
        ));
        config
    })
}

/// `server` names the actual executable family (notably basedpyright vs pyright).
fn load(root: &Path, server: &str) -> io::Result<Value> {
    configure(&read(root, VSCODE)?, &read(root, ZED)?, server)
}

fn configure(vscode_settings: &Value, zed: &Value, server: &str) -> io::Result<Value> {
    let mut settings = vscode(vscode_settings, server)?;
    if zed.get("lsp").is_some_and(|lsp| !lsp.is_object()) {
        return Err(io::Error::other(format!("{ZED}: lsp must be an object.")));
    }
    // Match the desktop's default even when editor settings are present.
    let mut initialization = if server == "rust-analyzer" {
        json!({"files":{"watcher":"server"}})
    } else {
        json!({})
    };
    if server == "rust-analyzer" || server == "gopls" {
        merge(&mut initialization, settings[server].clone());
    }
    let mut zed_settings = json!({});
    if let Some(config) = zed.get("lsp").and_then(|lsp| lsp.get(server)) {
        if !config.is_object() {
            return Err(io::Error::other(format!(
                "{ZED}: lsp.{server} must be an object."
            )));
        }
        for property in ["settings", "initialization_options"] {
            if let Some(value) = config.get(property) {
                if !value.is_object() {
                    return Err(io::Error::other(format!(
                        "{ZED}: lsp.{server}.{property} must be an object."
                    )));
                }
                if property == "settings" {
                    // Zed accepts dotted top-level section names as well.
                    for (key, value) in value.as_object().unwrap() {
                        insert_setting(&mut zed_settings, key, value.clone())
                            .map_err(|e| io::Error::other(format!("{ZED}: {e}")))?;
                    }
                } else {
                    merge(&mut initialization, value.clone());
                }
            }
        }
    }
    // These servers read the same options both at initialization and through
    // workspace/configuration. Keep both paths consistent so a later query
    // cannot undo Zed initialization overrides with lower-priority VS Code data.
    if server == "rust-analyzer" || server == "gopls" {
        if let Some(overrides) = zed_settings.get(server) {
            merge(&mut initialization, overrides.clone());
        }
        settings[server] = initialization.clone();
    }
    merge(&mut settings, zed_settings);
    Ok(json!({"settings":settings,"initializationOptions":initialization}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, file: &str, text: &str) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn vscode_jsonc_maps_dotted_settings_without_rewriting_dictionary_keys() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            VSCODE,
            r#"{
            // Shared editor settings are irrelevant to the language server.
            "editor.fontSize": 14,
            "rust-analyzer.check.command": "clippy",
            "rust-analyzer.cargo.features": ["feature.a",],
            "rust-analyzer.cargo.extraEnv": {"ENV.WITH.DOTS":"https://example.test/*literal*/"},
            /* The gopls extension uses an object with literal dotted option IDs. */
            "gopls": {"formatting.gofumpt":true,"analyses":{"unusedparams":true}},
        }"#,
        );
        let source = std::fs::read_to_string(root.path().join(VSCODE)).unwrap();
        write(root.path(), VSCODE, &format!("\u{feff}{source}"));
        let rust = load(root.path(), "rust-analyzer").unwrap();
        assert_eq!(
            rust["settings"]["rust-analyzer"]["check"]["command"],
            "clippy"
        );
        assert_eq!(rust["initializationOptions"]["files"]["watcher"], "server");
        assert_eq!(
            rust["initializationOptions"]["cargo"]["features"],
            json!(["feature.a"])
        );
        assert_eq!(
            rust["initializationOptions"]["cargo"]["extraEnv"]["ENV.WITH.DOTS"],
            "https://example.test/*literal*/"
        );
        assert!(rust["settings"].get("editor").is_none());
        assert!(rust["settings"].get("gopls").is_none());
        let go = load(root.path(), "gopls").unwrap();
        assert_eq!(go["settings"]["gopls"]["formatting.gofumpt"], true);
        assert_eq!(go["settings"]["gopls"], go["initializationOptions"]);
    }

    #[test]
    fn python_configuration_matches_the_selected_server_and_explicit_values_win() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            VSCODE,
            r#"{
            "python.pythonPath":".venv/bin/python",
            "python.analysis.typeCheckingMode":"strict",
            "python.analysis.diagnosticSeverityOverrides":{"reportUnusedImport":"warning"},
            "pyright.disableOrganizeImports":true,
            "basedpyright.analysis.typeCheckingMode":"recommended"
        }"#,
        );
        let pyright = load(root.path(), "pyright").unwrap();
        assert_eq!(
            pyright["settings"]["python"]["analysis"]["typeCheckingMode"],
            "strict"
        );
        assert_eq!(
            pyright["settings"]["pyright"]["disableOrganizeImports"],
            true
        );
        assert!(pyright["settings"].get("basedpyright").is_none());
        let based = load(root.path(), "basedpyright").unwrap();
        assert_eq!(
            based["settings"]["basedpyright"]["analysis"]["typeCheckingMode"],
            "recommended"
        );
        assert_eq!(
            based["settings"]["basedpyright"]["analysis"]["diagnosticSeverityOverrides"]["reportUnusedImport"],
            "warning"
        );
        assert_eq!(
            based["settings"]["python"]["pythonPath"],
            ".venv/bin/python"
        );
    }

    #[test]
    fn typescript_and_javascript_format_settings_stay_separate() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            VSCODE,
            r#"{"typescript.format.semicolons":"remove","javascript.format.semicolons":"insert","typescript.tsdk":"/not/used"}"#,
        );
        let config = load(root.path(), "typescript-language-server").unwrap();
        assert_eq!(
            config["settings"]["typescript"]["format"]["semicolons"],
            "remove"
        );
        assert_eq!(
            config["settings"]["javascript"]["format"]["semicolons"],
            "insert"
        );
        assert!(config["settings"]["typescript"].get("tsdk").is_none());
    }

    #[test]
    fn zed_overrides_merge_objects_replace_arrays_and_survive_configuration_queries() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            VSCODE,
            r#"{"rust-analyzer.check.command":"check","rust-analyzer.cargo.features":["old"],"rust-analyzer.cargo.allTargets":false}"#,
        );
        write(
            root.path(),
            ZED,
            r#"{"lsp":{"rust-analyzer":{
            "initialization_options":{"check":{"command":"clippy"},"cargo":{"features":["new"]}},
            "settings":{"rust-analyzer.cargo":{"allTargets":true}},
            "binary":{"path":"/not/used"}
        }}}"#,
        );
        let config = load(root.path(), "rust-analyzer").unwrap();
        assert_eq!(
            config["initializationOptions"]["check"]["command"],
            "clippy"
        );
        assert_eq!(
            config["initializationOptions"]["cargo"]["features"],
            json!(["new"])
        );
        assert_eq!(config["initializationOptions"]["cargo"]["allTargets"], true);
        assert_eq!(
            config["initializationOptions"],
            config["settings"]["rust-analyzer"]
        );
        assert!(config.get("binary").is_none());
    }

    #[test]
    fn zed_uses_the_actual_python_server_and_preserves_raw_typescript_options() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            ZED,
            r#"{"lsp":{
            "pyright":{"settings":{"python":{"analysis":{"typeCheckingMode":"strict"}}}},
            "basedpyright":{"settings":{"basedpyright.analysis":{"typeCheckingMode":"basic"}}},
            "typescript-language-server":{"initialization_options":{"preferences":{"quotePreference":"single"}},"settings":{"completions":{"completeFunctionCalls":false}}}
        }}"#,
        );
        let based = load(root.path(), "basedpyright").unwrap();
        assert_eq!(
            based["settings"]["basedpyright"]["analysis"]["typeCheckingMode"],
            "basic"
        );
        assert!(based["settings"]["python"].get("analysis").is_none());
        let ts = load(root.path(), "typescript-language-server").unwrap();
        assert_eq!(
            ts["initializationOptions"]["preferences"]["quotePreference"],
            "single"
        );
        assert_eq!(
            ts["settings"]["completions"]["completeFunctionCalls"],
            false
        );
    }

    #[test]
    fn missing_settings_leave_native_project_configurations_to_the_server() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "pyrightconfig.json",
            "{\"typeCheckingMode\":\"strict\"}",
        );
        write(
            root.path(),
            ".newport/lsp.json",
            "obsolete and deliberately invalid",
        );
        let config = load(root.path(), "pyright").unwrap();
        assert_eq!(config["settings"], json!({"python":{},"pyright":{}}));
    }

    #[test]
    fn invalid_or_oversized_editor_settings_name_the_source() {
        let root = tempfile::tempdir().unwrap();
        for file in [VSCODE, ZED] {
            for invalid in ["{", "[]", "{\"x\":1 \"y\":2}", "{unquoted:1}"] {
                write(root.path(), file, invalid);
                assert!(
                    load(root.path(), "gopls")
                        .unwrap_err()
                        .to_string()
                        .contains(file)
                );
            }
            write(root.path(), file, &" ".repeat(MAX_BYTES as usize + 1));
            assert!(
                load(root.path(), "gopls")
                    .unwrap_err()
                    .to_string()
                    .contains("256 KiB")
            );
            write(root.path(), file, "{}");
        }
        write(root.path(), ZED, r#"{"lsp":{"gopls":{"settings":[]}}}"#);
        assert!(
            load(root.path(), "gopls")
                .unwrap_err()
                .to_string()
                .contains("lsp.gopls.settings")
        );
    }

    #[test]
    fn empty_settings_are_empty_and_unusable_settings_start_with_defaults() {
        let root = tempfile::tempdir().unwrap();
        let defaults = resolve(root.path(), "rust-analyzer");
        assert!(defaults.get("settingsError").is_none());
        for file in [VSCODE, ZED] {
            for empty in ["", " \n", "\u{feff}"] {
                write(root.path(), file, empty);
                assert_eq!(resolve(root.path(), "rust-analyzer"), defaults);
            }
        }
        write(root.path(), ZED, "{\"lsp\":");
        let config = resolve(root.path(), "rust-analyzer");
        assert!(config["settingsError"].as_str().unwrap().contains(ZED));
        assert_eq!(
            config["initializationOptions"],
            defaults["initializationOptions"]
        );
        assert_eq!(config["settings"], defaults["settings"]);
    }

    #[test]
    fn settings_that_are_not_regular_files_are_not_read() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".vscode")).unwrap();
        let fifo = std::ffi::CString::new(root.path().join(VSCODE).as_os_str().as_encoded_bytes())
            .unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(
            resolve(root.path(), "gopls")["settingsError"]
                .as_str()
                .unwrap()
                .contains("isn't a regular file")
        );
        std::fs::remove_file(root.path().join(VSCODE)).unwrap();
        std::fs::create_dir(root.path().join(VSCODE)).unwrap();
        assert!(
            load(root.path(), "gopls")
                .unwrap_err()
                .to_string()
                .contains(VSCODE)
        );
    }

    #[cfg(unix)]
    #[test]
    fn configuration_symlinks_cannot_escape_the_worktree() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "settings.json", "{}");
        std::fs::create_dir(root.path().join(".vscode")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("settings.json"),
            root.path().join(VSCODE),
        )
        .unwrap();
        assert!(
            load(root.path(), "gopls")
                .unwrap_err()
                .to_string()
                .contains("outside the worktree")
        );
    }
}
