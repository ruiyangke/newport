//! The desktop UI reads replies as the protocol's own types. This check fails
//! when production code in the UI or the client works with JSON, and when
//! anything under `crates/` names `execute_rendered`.
use anyhow::{Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// A source tree where production code must not work with JSON, and the files
/// exempt from that, each with what its JSON input is.
struct Scope {
    directory: &'static str,
    list: &'static str,
    files: &'static [(&'static str, &'static str)],
}

const SCOPES: &[Scope] = &[
    Scope {
        directory: "crates/newport-gpui/src",
        list: "UI_JSON_FILES",
        files: UI_JSON_FILES,
    },
    Scope {
        directory: "crates/newport-client/src",
        list: "CLIENT_JSON_FILES",
        files: CLIENT_JSON_FILES,
    },
];

/// UI files whose input is JSON that is not a protocol reply, and what it is.
/// Keep this short: a file belongs here only while that JSON is its input.
const UI_JSON_FILES: &[(&str, &str)] = &[
    ("about.rs", "the application manifest"),
    (
        "workspace_state.rs",
        "the saved workspace file, whose decoding rules stay exact",
    ),
    ("ui_state.rs", "the saved UI state file"),
    ("grammars.rs", "the grammar metadata files"),
    (
        "git_diff_syntax.rs",
        "the highlight theme, whose styles can only be built by deserializing",
    ),
    ("project_editor_lsp.rs", "language server messages"),
    (
        "project_editor_lsp_actions.rs",
        "language server command replies",
    ),
    (
        "project_editor_lsp_protocol.rs",
        "language server registrations and the settings Needle passes to the server",
    ),
];

/// Client files whose input is JSON that is not a protocol reply, and what it
/// is. Keep this short: a file belongs here only while that JSON is its input.
const CLIENT_JSON_FILES: &[(&str, &str)] = &[
    ("lsp.rs", "the language server's JSON-RPC stream"),
    (
        "window_preferences.rs",
        "the saved preferences file, read key by key",
    ),
];

const RENDERED: &str = "execute_rendered";
/// `LSPAny`, `LSPObject` and `LSPArray` in `lsp_types` are aliases of
/// `serde_json` types, so they carry JSON without naming it.
const JSON_NAMES: &[&str] = &["serde_json", "json!", "LSPAny", "LSPObject", "LSPArray"];

pub fn check(root: &Path) -> Result<()> {
    let mut problems = Vec::new();
    for file in rust_files(&root.join("crates")) {
        let code = mask(&std::fs::read_to_string(&file)?);
        if let Some(line) = find(&code, RENDERED.as_bytes()) {
            problems.push(format!(
                "{}:{line}: names `{RENDERED}`; read the typed reply instead",
                relative(root, &file)
            ));
        }
    }
    for scope in SCOPES {
        let directory = root.join(scope.directory);
        let allowed: BTreeSet<_> = scope
            .files
            .iter()
            .map(|(file, _)| directory.join(file))
            .collect();
        problems.extend(json_problems(root, &directory, &allowed, scope.list)?);
        for file in &allowed {
            if !file.is_file() {
                problems.push(format!(
                    "{}: listed in `{}` but missing; take it off that list",
                    relative(root, file),
                    scope.list
                ));
            }
        }
    }
    if !problems.is_empty() {
        bail!(
            "The desktop UI and client must work with typed values:\n{}",
            problems.join("\n")
        );
    }
    Ok(())
}

/// Production code in `directory`, outside `allowed`, that names `serde_json`,
/// `json!` or an `lsp_types` alias of a JSON type.
fn json_problems(
    root: &Path,
    directory: &Path,
    allowed: &BTreeSet<PathBuf>,
    list: &str,
) -> Result<Vec<String>> {
    let files = rust_files(directory);
    let tests = test_files(&files)?;
    let mut problems = Vec::new();
    for file in files {
        if tests.contains(&file) || allowed.contains(&file) {
            continue;
        }
        let mut code = mask(&std::fs::read_to_string(&file)?);
        without_tests(&mut code);
        for name in JSON_NAMES {
            if let Some(line) = find(&code, name.as_bytes()) {
                problems.push(format!(
                    "{}:{line}: uses `{name}` in production code; read the protocol type, \
                     or list the file in `{list}` in tools/xtask/src/typed_ui.rs if its input is JSON that is not a reply",
                    relative(root, &file)
                ));
            }
        }
    }
    Ok(problems)
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
}

fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = walkdir::WalkDir::new(directory)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "target")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    files.sort();
    files
}

/// `source` with comments and the contents of string and character literals
/// blanked, so byte positions and line breaks still match the source.
fn mask(source: &str) -> Vec<u8> {
    let s = source.as_bytes();
    let mut out = s.to_vec();
    let blank = |out: &mut [u8], from: usize, to: usize| {
        for byte in &mut out[from..to.min(s.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let ident = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut i = 0;
    while i < s.len() {
        let next = s.get(i + 1).copied();
        match s[i] {
            b'/' if next == Some(b'/') => {
                let end = s[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(s.len(), |n| i + n);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if next == Some(b'*') => {
                let (mut j, mut depth) = (i + 2, 1);
                while j < s.len() && depth > 0 {
                    if s[j..].starts_with(b"/*") {
                        depth += 1;
                        j += 2;
                    } else if s[j..].starts_with(b"*/") {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut out, i, j);
                i = j;
            }
            b'r' | b'b' if i == 0 || !ident(s[i - 1]) => {
                let start = if s[i] == b'b' && next == Some(b'r') {
                    i + 2
                } else if s[i] == b'r' {
                    i + 1
                } else {
                    i += 1;
                    continue;
                };
                let hashes = s[start..].iter().take_while(|&&b| b == b'#').count();
                if s.get(start + hashes) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let open = start + hashes + 1;
                let mut close = vec![b'"'];
                close.extend(std::iter::repeat_n(b'#', hashes));
                let end = s[open..]
                    .windows(close.len())
                    .position(|w| w == close.as_slice())
                    .map_or(s.len(), |n| open + n);
                blank(&mut out, open, end);
                i = end + close.len();
            }
            b'"' => {
                let mut j = i + 1;
                while j < s.len() && s[j] != b'"' {
                    j += if s[j] == b'\\' { 2 } else { 1 };
                }
                blank(&mut out, i + 1, j);
                i = j + 1;
            }
            b'\'' => {
                if next == Some(b'\\') {
                    let mut j = i + 3;
                    while j < s.len() && s[j] != b'\'' {
                        j += 1;
                    }
                    blank(&mut out, i + 1, j);
                    i = j + 1;
                } else {
                    let width = source[i + 1..].chars().next().map_or(1, char::len_utf8);
                    if s.get(i + 1 + width) == Some(&b'\'') {
                        blank(&mut out, i + 1, i + 1 + width);
                        i += 2 + width;
                    } else {
                        // A lifetime or label.
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// The 1-based line of the first occurrence of `needle` in `code`.
fn find(code: &[u8], needle: &[u8]) -> Option<usize> {
    let at = code.windows(needle.len()).position(|w| w == needle)?;
    Some(code[..at].iter().filter(|&&b| b == b'\n').count() + 1)
}

/// The end of the attributes starting at `i`, past whitespace.
fn after_attributes(code: &[u8], mut i: usize) -> usize {
    loop {
        while code.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if !code[i..].starts_with(b"#[") {
            return i;
        }
        let mut depth = 0;
        while i < code.len() {
            match code[i] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// The end of the item, field or statement starting at `i`: past its block,
/// or its `;` or `,`, or before the brace that closes its parent.
fn item_end(code: &[u8], mut i: usize) -> usize {
    let mut depth = 0usize;
    while i < code.len() {
        match code[i] {
            b'(' | b'[' => depth += 1,
            b')' | b']' if depth == 0 => return i,
            b')' | b']' => depth -= 1,
            b'}' if depth == 0 => return i,
            b';' | b',' if depth == 0 => return i + 1,
            b'{' if depth == 0 => {
                let mut braces = 0usize;
                while i < code.len() {
                    match code[i] {
                        b'{' => braces += 1,
                        b'}' => {
                            braces -= 1;
                            if braces == 0 {
                                return i + 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                return i;
            }
            _ => {}
        }
        i += 1;
    }
    i
}

const CFG_TEST: &[u8] = b"#[cfg(test)]";

/// Blanks every item marked `#[cfg(test)]`.
fn without_tests(code: &mut [u8]) {
    let mut i = 0;
    while let Some(found) = code[i..]
        .windows(CFG_TEST.len())
        .position(|w| w == CFG_TEST)
    {
        let start = i + found;
        let end = item_end(code, after_attributes(code, start + CFG_TEST.len()));
        for byte in &mut code[start..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        i = end.max(start + 1);
    }
}

/// A `mod name;` declaration: the file it names, and whether it is compiled
/// only for tests.
struct Declaration {
    files: Vec<PathBuf>,
    test: bool,
}

/// Module files declared in `source`, the file at `file`.
fn declarations(file: &Path, source: &str) -> Vec<Declaration> {
    let code = mask(source);
    let directory = file.parent().unwrap_or(Path::new(""));
    let owner = matches!(
        file.file_name().and_then(|n| n.to_str()),
        Some("lib.rs" | "main.rs" | "mod.rs")
    );
    let nested = if owner {
        directory.to_path_buf()
    } else {
        directory.join(file.file_stem().unwrap_or_default())
    };
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(at) = code[i..].windows(4).position(|w| w == b"mod ") {
        let start = i + at;
        i = start + 4;
        if start > 0 && (code[start - 1].is_ascii_alphanumeric() || code[start - 1] == b'_') {
            continue;
        }
        let name: String = code[i..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
            .map(|&b| b as char)
            .collect();
        let rest = &code[i + name.len()..];
        let terminated = rest
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|&b| b == b';');
        if name.is_empty() || !terminated {
            continue;
        }
        // The attributes and visibility before `mod`, back to the previous item.
        let head_start = code[..start]
            .iter()
            .rposition(|&b| b == b';' || b == b'}' || b == b'{')
            .map_or(0, |n| n + 1);
        let head = &code[head_start..start];
        let test = find(head, CFG_TEST).is_some();
        let path = head
            .windows(7)
            .position(|w| w == b"#[path ")
            .and_then(|at| {
                let open = head_start + at + head[at..].iter().position(|&b| b == b'"')? + 1;
                let close = open + source.as_bytes()[open..].iter().position(|&b| b == b'"')?;
                Some(source[open..close].to_owned())
            });
        let files = match path {
            Some(path) => vec![directory.join(path)],
            None => vec![
                nested.join(format!("{name}.rs")),
                nested.join(&name).join("mod.rs"),
            ],
        };
        found.push(Declaration { files, test });
    }
    found
}

/// The files compiled only for tests: those declared under `#[cfg(test)]`,
/// and every module a test-only file declares.
fn test_files(files: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let mut declared = BTreeMap::new();
    for file in files {
        declared.insert(
            file.clone(),
            declarations(file, &std::fs::read_to_string(file)?),
        );
    }
    let mut tests = BTreeSet::new();
    loop {
        let before = tests.len();
        for (file, declarations) in &declared {
            let test_owner = tests.contains(file);
            for declaration in declarations {
                if declaration.test || test_owner {
                    tests.extend(declaration.files.iter().cloned());
                }
            }
        }
        if tests.len() == before {
            return Ok(tests);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flagged(files: &[(&str, &str)], allowed: &[&str]) -> Vec<String> {
        let root = tempfile::tempdir().unwrap();
        let ui = root.path().join("ui");
        std::fs::create_dir(&ui).unwrap();
        let mut lib = String::new();
        for (name, text) in files {
            std::fs::write(ui.join(name), text).unwrap();
            lib.push_str(&format!("mod {};\n", name.trim_end_matches(".rs")));
        }
        std::fs::write(ui.join("lib.rs"), lib).unwrap();
        let allowed = allowed.iter().map(|name| ui.join(name)).collect();
        json_problems(root.path(), &ui, &allowed, "UI_JSON_FILES").unwrap()
    }

    #[test]
    fn production_ui_code_naming_json_fails_with_file_and_line() {
        let problems = flagged(
            &[(
                "view.rs",
                "fn f() {\n    let v = serde_json::json!(1);\n}\n",
            )],
            &[],
        );
        assert_eq!(problems.len(), 2);
        assert!(problems[0].starts_with("ui/view.rs:2: uses `serde_json`"));
        assert!(problems[0].contains("production code"));
        assert!(problems[1].starts_with("ui/view.rs:2: uses `json!`"));
        assert!(problems[0].contains("`UI_JSON_FILES`"));
        assert_eq!(
            flagged(&[("view.rs", "use serde_json::Value;")], &[]).len(),
            1
        );
    }

    #[test]
    fn lsp_types_aliases_of_json_types_fail() {
        for name in ["LSPAny", "LSPObject", "LSPArray"] {
            let code = format!("fn f(v: lsp_types::{name}) {{}}\n");
            let problems = flagged(&[("view.rs", &code)], &[]);
            assert_eq!(problems.len(), 1, "{name}");
            assert!(problems[0].starts_with(&format!("ui/view.rs:1: uses `{name}`")));
        }
        let test_only = "#[cfg(test)]\nfn f(v: lsp_types::LSPAny) {}\n";
        assert!(flagged(&[("view.rs", test_only)], &[]).is_empty());
        assert!(flagged(&[("raw.rs", "type A = lsp_types::LSPAny;\n")], &["raw.rs"]).is_empty());
    }

    #[test]
    fn a_client_production_file_using_json_fails_and_a_listed_one_passes() {
        let root = tempfile::tempdir().unwrap();
        let client = root.path().join("client");
        std::fs::create_dir(&client).unwrap();
        std::fs::write(client.join("lib.rs"), "mod reply;\nmod rpc;\nmod fake;\n").unwrap();
        std::fs::write(
            client.join("reply.rs"),
            "fn f() -> serde_json::Value { todo!() }\n",
        )
        .unwrap();
        std::fs::write(client.join("rpc.rs"), "use serde_json::json;\n").unwrap();
        std::fs::write(
            client.join("fake.rs"),
            "#[cfg(test)]\nmod tests { use serde_json::json; }\n",
        )
        .unwrap();
        let allowed = BTreeSet::from([client.join("rpc.rs")]);
        let problems = json_problems(root.path(), &client, &allowed, "CLIENT_JSON_FILES").unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("client/reply.rs:1: uses `serde_json`"));
        assert!(problems[0].contains("`CLIENT_JSON_FILES`"));
    }

    #[test]
    fn every_listed_file_exists() {
        for scope in SCOPES {
            for (file, reason) in scope.files {
                let path = crate::root().join(scope.directory).join(file);
                assert!(path.is_file(), "{} is listed but missing", path.display());
                assert!(!reason.is_empty());
            }
        }
    }

    #[test]
    fn json_in_tests_comments_literals_and_listed_files_passes() {
        let inline = "#[cfg(test)]\nmod tests {\n    use serde_json::json;\n    fn f() { json!(1); }\n}\n#[cfg(test)]\nuse serde_json::Value;\n";
        assert!(flagged(&[("view.rs", inline)], &[]).is_empty());
        let quiet = "// serde_json\nfn f() { let _ = \"json!\"; }\n";
        assert!(flagged(&[("view.rs", quiet)], &[]).is_empty());
        let json = "use serde_json::Value;\n";
        assert!(flagged(&[("raw.rs", json)], &["raw.rs"]).is_empty());
        assert_eq!(flagged(&[("raw.rs", json)], &[]).len(), 1);
    }

    #[test]
    fn json_in_a_module_declared_for_tests_passes() {
        let root = tempfile::tempdir().unwrap();
        let ui = root.path().join("ui");
        std::fs::create_dir(&ui).unwrap();
        std::fs::write(ui.join("fixture.rs"), "use serde_json::json;\n").unwrap();
        std::fs::write(ui.join("lib.rs"), "#[cfg(test)]\nmod fixture;\n").unwrap();
        assert!(
            json_problems(root.path(), &ui, &BTreeSet::new(), "UI_JSON_FILES")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn modules_declared_for_tests_and_their_children_are_test_only() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path();
        let write = |name: &str, text: &str| std::fs::write(src.join(name), text).unwrap();
        write(
            "lib.rs",
            "mod shown;\n#[cfg(test)]\nmod fixture;\n#[cfg(test)]\n#[path = \"other_tests.rs\"]\nmod other;\n",
        );
        write("shown.rs", "");
        write("fixture.rs", "mod helper;\n");
        std::fs::create_dir(src.join("fixture")).unwrap();
        write("fixture/helper.rs", "");
        write("other_tests.rs", "");
        let files = rust_files(src);
        let tests = test_files(&files).unwrap();
        assert!(tests.contains(&src.join("fixture.rs")));
        assert!(tests.contains(&src.join("fixture/helper.rs")));
        assert!(tests.contains(&src.join("other_tests.rs")));
        assert!(!tests.contains(&src.join("shown.rs")));
        assert!(!tests.contains(&src.join("lib.rs")));
    }

    #[test]
    fn the_desktop_ui_reads_typed_replies() {
        check(&crate::root()).unwrap();
    }
}
