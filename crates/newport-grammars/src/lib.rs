//! Versioned C boundary for Newport's bundled grammar library.
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
        highlights: &str,
        injections: &str,
        locals: &str,
    ) -> Self {
        Self {
            name: name.into(),
            language: Some(language),
            injection_languages,
            highlights: highlights.into(),
            injections: injections.into(),
            locals: locals.into(),
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
fn entries() -> &'static Vec<(GrammarConfig, CString)> {
    static ENTRIES: OnceLock<Vec<(GrammarConfig, CString)>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        languages::Language::all()
            .filter(|l| *l != languages::Language::Plain)
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
#[unsafe(no_mangle)]
pub extern "C" fn newport_grammar_metadata_v1(index: u32) -> *const c_char {
    entries()
        .get(index as usize)
        .map_or(std::ptr::null(), |e| e.1.as_ptr())
}
#[unsafe(no_mangle)]
pub extern "C" fn newport_grammar_function_v1(
    index: u32,
) -> Option<unsafe extern "C" fn() -> *const ()> {
    entries()
        .get(index as usize)
        .and_then(|e| e.0.language.map(|l| l.into_raw()))
}
