//! Versioned C boundary for Newport's per-language grammar libraries.
mod languages;
use std::{
    ffi::{CString, c_char},
    sync::OnceLock,
};
use tree_sitter_language::LanguageFn;

#[derive(serde::Serialize)]
struct GrammarConfig {
    name: String,
    injection_languages: Vec<String>,
    highlights: String,
    injections: String,
    locals: String,
    #[serde(skip)]
    language: Option<LanguageFn>,
}
impl GrammarConfig {
    fn new(
        name: &str,
        language: LanguageFn,
        injection_languages: Vec<String>,
        highlights: String,
        injections: String,
        locals: String,
    ) -> Self {
        Self {
            name: name.into(),
            language: Some(language),
            injection_languages,
            highlights: theme_names(&highlights),
            injections,
            locals,
        }
    }
    fn plain(name: &str) -> Self {
        Self {
            name: name.into(),
            language: None,
            injection_languages: vec![],
            highlights: String::new(),
            injections: String::new(),
            locals: String::new(),
        }
    }
}
/// Capture names in the grammars' own queries, as the highlight theme names
/// them. Names the theme knows, or whose first part it knows, stay as they are.
const THEME_NAMES: &[(&str, &str)] = &[
    ("comment.documentation", "comment.doc"),
    ("variable.builtin", "variable.special"),
    ("variable.member", "property"),
    ("field", "property"),
    ("parameter", "variable"),
    ("string.special.key", "property"),
    ("method", "function"),
    ("method.call", "function"),
    ("conditional", "keyword"),
    ("repeat", "keyword"),
    ("include", "keyword"),
    ("import", "keyword"),
    ("exception", "keyword"),
    ("storageclass", "keyword"),
    ("float", "number"),
    ("character", "string"),
    ("character.special", "string.special"),
    ("escape", "string.escape"),
    ("string.regexp", "string.regex"),
    ("string.special.regex", "string.regex"),
    ("delimiter", "punctuation.delimiter"),
    ("tag.delimiter", "punctuation.bracket"),
    ("text.title", "title"),
    ("text.uri", "link_uri"),
    ("text.reference", "link_text"),
    ("text.emphasis", "emphasis"),
    ("text.strong", "emphasis.strong"),
    ("text.note", "comment"),
    ("text.warning", "comment"),
    ("text.danger", "comment"),
    ("module", "type"),
    ("module.builtin", "type"),
    ("namespace", "type"),
];

/// `query` with each capture renamed by [`THEME_NAMES`]. Strings and comments
/// are left alone: a query matches literal tokens such as CSS's `"@import"`.
fn theme_names(query: &str) -> String {
    let mut output = String::with_capacity(query.len());
    let mut chars = query.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        output.push(c);
        match c {
            '"' => {
                let mut escaped = false;
                for (_, c) in chars.by_ref() {
                    output.push(c);
                    if c == '"' && !escaped {
                        break;
                    }
                    escaped = c == '\\' && !escaped;
                }
            }
            ';' => {
                for (_, c) in chars.by_ref() {
                    output.push(c);
                    if c == '\n' {
                        break;
                    }
                }
            }
            '@' => {
                let rest = &query[at + 1..];
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || "_.-".contains(c)))
                    .unwrap_or(rest.len());
                let name = &rest[..end];
                let theme = THEME_NAMES
                    .iter()
                    .find_map(|(from, to)| (*from == name).then_some(*to));
                output.push_str(theme.unwrap_or(name));
                for _ in 0..name.chars().count() {
                    chars.next();
                }
            }
            _ => {}
        }
    }
    output
}

fn entries() -> &'static Vec<(GrammarConfig, CString)> {
    static ENTRIES: OnceLock<Vec<(GrammarConfig, CString)>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        languages::Language::all()
            .filter(|l| l.config().language.is_some())
            .map(|l| {
                let config = l.config();
                let aliases = match l {
                    languages::Language::Plain => vec!["text", "plain", "plaintext"],
                    languages::Language::Json => vec!["json", "jsonc"],
                    languages::Language::Astro => vec!["astro"],
                    languages::Language::Bash => vec!["bash", "sh"],
                    languages::Language::C => vec!["c"],
                    languages::Language::CMake => vec!["cmake"],
                    languages::Language::Cpp => vec!["cpp", "c++"],
                    languages::Language::CSharp => vec!["csharp", "cs"],
                    languages::Language::Css => vec!["css", "scss"],
                    languages::Language::Diff => vec!["diff"],
                    languages::Language::Ejs => vec!["ejs"],
                    languages::Language::Elixir => vec!["elixir", "ex"],
                    languages::Language::Erb => vec!["erb"],
                    languages::Language::Go => vec!["go"],
                    languages::Language::GraphQL => vec!["graphql"],
                    languages::Language::Html => vec!["html"],
                    languages::Language::Java => vec!["java"],
                    languages::Language::JavaScript => vec!["javascript", "js"],
                    languages::Language::JsDoc => vec!["jsdoc"],
                    languages::Language::Kotlin => vec!["kt", "kts", "ktm", "kotlin"],
                    languages::Language::Lua => vec!["lua"],
                    languages::Language::Make => vec!["make", "makefile"],
                    languages::Language::Markdown => vec!["markdown", "md", "mdx"],
                    languages::Language::MarkdownInline => {
                        vec!["markdown_inline", "markdown-inline"]
                    }
                    languages::Language::Php => vec!["php", "php3", "php4", "php5", "phtml"],
                    languages::Language::Proto => vec!["proto", "protobuf"],
                    languages::Language::Python => vec!["python", "py"],
                    languages::Language::Ruby => vec!["ruby", "rb"],
                    languages::Language::Rust => vec!["rust", "rs"],
                    languages::Language::Scala => vec!["scala"],
                    languages::Language::Sql => vec!["sql"],
                    languages::Language::Svelte => vec!["svelte"],
                    languages::Language::Swift => vec!["swift"],
                    languages::Language::Toml => vec!["toml"],
                    languages::Language::Tsx => vec!["tsx"],
                    languages::Language::TypeScript => vec!["typescript", "ts"],
                    languages::Language::Yaml => vec!["yaml", "yml"],
                    languages::Language::Zig => vec!["zig"],
                };
                let mut metadata = serde_json::to_value(&config).expect("grammar metadata");
                metadata["aliases"] = serde_json::json!(aliases);
                let json = CString::new(metadata.to_string()).expect("JSON has no raw NUL");
                (config, json)
            })
            .collect()
    })
}
/// Metadata and pointers remain valid until the library is unloaded.
// SAFETY: The exported name is unique to this library.
#[unsafe(no_mangle)]
pub extern "C" fn newport_grammar_metadata_v1(index: u32) -> *const c_char {
    entries()
        .get(index as usize)
        .map_or(std::ptr::null(), |e| e.1.as_ptr())
}
// SAFETY: The exported name is unique to this library.
#[unsafe(no_mangle)]
pub extern "C" fn newport_grammar_function_v1(
    index: u32,
) -> Option<unsafe extern "C" fn() -> *const ()> {
    entries()
        .get(index as usize)
        .and_then(|e| e.0.language.map(|l| l.into_raw()))
}

/// Build-time catalog; contains queries and aliases, never native pointers.
pub fn catalog() -> String {
    let grammars: Vec<serde_json::Value> = entries()
        .iter()
        .map(|(_, json)| serde_json::from_str(json.to_str().unwrap()).unwrap())
        .collect();
    serde_json::json!({"schema": 1, "grammars": grammars}).to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn captures_are_renamed_but_strings_and_comments_are_not() {
        assert_eq!(
            super::theme_names(
                "; @import in a comment\n\"@import\" @import (#eq? @field \"a\\\"@field\")\n"
            ),
            "; @import in a comment\n\"@import\" @keyword (#eq? @property \"a\\\"@field\")\n"
        );
    }
}
