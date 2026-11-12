// Adapted from GPUI Kit bfc64ce6 (Apache-2.0). Keep queries and aliases together.
type SharedString = String;

use crate::GrammarConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, enum_iterator::Sequence)]
pub enum Language {
    Json,
    Plain,
    Astro,
    Bash,
    C,
    CMake,
    CSharp,
    Cpp,
    Css,
    Diff,
    Ejs,
    Elixir,
    Erb,
    Go,
    GraphQL,
    Html,
    Java,
    JavaScript,
    JsDoc,
    Kotlin,
    Lua,
    Make,
    Markdown,
    MarkdownInline,
    Php,
    Proto,
    Python,
    Ruby,
    Rust,
    Scala,
    Sql,
    Svelte,
    Swift,
    Toml,
    Tsx,
    TypeScript,
    Yaml,
    Zig,
}

impl From<Language> for SharedString {
    fn from(language: Language) -> Self {
        language.name().into()
    }
}

impl Language {
    pub fn all() -> impl Iterator<Item = Self> {
        enum_iterator::all::<Language>()
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Plain => "text",
            Self::Astro => "astro",
            Self::Bash => "bash",
            Self::C => "c",
            Self::CMake => "cmake",
            Self::CSharp => "csharp",
            Self::Cpp => "cpp",
            Self::Css => "css",
            Self::Diff => "diff",
            Self::Ejs => "ejs",
            Self::Elixir => "elixir",
            Self::Erb => "erb",
            Self::Go => "go",
            Self::GraphQL => "graphql",
            Self::Html => "html",
            Self::Java => "java",
            Self::JavaScript => "javascript",
            Self::JsDoc => "jsdoc",
            Self::Kotlin => "kotlin",
            Self::Lua => "lua",
            Self::Make => "make",
            Self::Markdown => "markdown",
            Self::MarkdownInline => "markdown_inline",
            Self::Php => "php",
            Self::Proto => "proto",
            Self::Python => "python",
            Self::Ruby => "ruby",
            Self::Rust => "rust",
            Self::Scala => "scala",
            Self::Sql => "sql",
            Self::Svelte => "svelte",
            Self::Swift => "swift",
            Self::Toml => "toml",
            Self::Tsx => "tsx",
            Self::TypeScript => "typescript",
            Self::Yaml => "yaml",
            Self::Zig => "zig",
        }
    }

    #[allow(unused)]
    pub fn from_str(s: &str) -> Self {
        Self::from_name(s).unwrap_or(Self::Plain)
    }

    pub(crate) fn from_name(s: &str) -> Option<Self> {
        match s {
            "text" | "plain" | "plaintext" => Some(Self::Plain),
            "json" | "jsonc" => Some(Self::Json),
            "astro" => Some(Self::Astro),
            "bash" | "sh" => Some(Self::Bash),
            "c" => Some(Self::C),
            "cmake" => Some(Self::CMake),
            "cpp" | "c++" => Some(Self::Cpp),
            "csharp" | "cs" => Some(Self::CSharp),
            "css" | "scss" => Some(Self::Css),
            "diff" => Some(Self::Diff),
            "ejs" => Some(Self::Ejs),
            "elixir" | "ex" => Some(Self::Elixir),
            "erb" => Some(Self::Erb),
            "go" => Some(Self::Go),
            "graphql" => Some(Self::GraphQL),
            "html" => Some(Self::Html),
            "java" => Some(Self::Java),
            "javascript" | "js" => Some(Self::JavaScript),
            "jsdoc" => Some(Self::JsDoc),
            "kt" | "kts" | "ktm" | "kotlin" => Some(Self::Kotlin),
            "lua" => Some(Self::Lua),
            "make" | "makefile" => Some(Self::Make),
            "markdown" | "md" | "mdx" => Some(Self::Markdown),
            "markdown_inline" | "markdown-inline" => Some(Self::MarkdownInline),
            "php" | "php3" | "php4" | "php5" | "phtml" => Some(Self::Php),
            "proto" | "protobuf" => Some(Self::Proto),
            "python" | "py" => Some(Self::Python),
            "ruby" | "rb" => Some(Self::Ruby),
            "rust" | "rs" => Some(Self::Rust),
            "scala" => Some(Self::Scala),
            "sql" => Some(Self::Sql),
            "svelte" => Some(Self::Svelte),
            "swift" => Some(Self::Swift),
            "toml" => Some(Self::Toml),
            "tsx" => Some(Self::Tsx),
            "typescript" | "ts" => Some(Self::TypeScript),
            "yaml" | "yml" => Some(Self::Yaml),
            "zig" => Some(Self::Zig),
            _ => None,
        }
    }

    #[allow(unused)]
    pub(super) fn injection_languages(&self) -> Vec<SharedString> {
        let mut languages: Vec<&'static str> = Vec::new();

        match self {
            Self::Markdown => {
                languages.push("markdown_inline");
                languages.push("html");
                languages.push("toml");
                languages.push("yaml");
            }
            Self::Html => {
                languages.push("javascript");
                languages.push("css");
            }
            Self::Rust => {
                languages.push("rust");
            }
            Self::JavaScript => {
                languages.push("jsdoc");
                languages.push("json");
                languages.push("css");
                languages.push("html");
                languages.push("sql");
                languages.push("typescript");
                languages.push("javascript");
                languages.push("tsx");
                languages.push("yaml");
                languages.push("graphql");
            }
            Self::TypeScript => {
                languages.push("jsdoc");
                languages.push("json");
                languages.push("css");
                languages.push("html");
                languages.push("sql");
                languages.push("typescript");
                languages.push("javascript");
                languages.push("tsx");
                languages.push("yaml");
                languages.push("graphql");
            }
            Self::Astro => {
                languages.push("html");
                languages.push("css");
                languages.push("javascript");
                languages.push("typescript");
            }
            Self::Php => {
                languages.push("php");
                languages.push("html");
                languages.push("css");
                languages.push("javascript");
                languages.push("json");
                languages.push("jsdoc");
                languages.push("graphql");
            }
            Self::Svelte => {
                languages.push("svelte");
                languages.push("html");
                languages.push("css");
                languages.push("typescript");
            }
            _ => {}
        }

        languages.into_iter().map(SharedString::from).collect()
    }

    /// The grammar and its official queries: highlights, injections and
    /// locals, each joined from the grammar's own files and, where upstream
    /// builds on a base language, that language's files first.
    pub(super) fn config(&self) -> GrammarConfig {
        #[allow(unreachable_patterns)]
        let (language, query, injection, locals) = match self {
            Self::Plain => return GrammarConfig::plain(self.name()),
            #[cfg(feature = "lang-json")]
            Self::Json => (
                tree_sitter_json::LANGUAGE,
                q(&[tree_sitter_json::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-markdown")]
            Self::Markdown => (
                tree_sitter_md::LANGUAGE,
                q(&[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK]),
                q(&[tree_sitter_md::INJECTION_QUERY_BLOCK]),
                q(&[]),
            ),
            #[cfg(feature = "lang-markdown_inline")]
            Self::MarkdownInline => (
                tree_sitter_md::INLINE_LANGUAGE,
                q(&[tree_sitter_md::HIGHLIGHT_QUERY_INLINE]),
                q(&[tree_sitter_md::INJECTION_QUERY_INLINE]),
                q(&[]),
            ),
            #[cfg(feature = "lang-toml")]
            Self::Toml => (
                tree_sitter_toml_ng::LANGUAGE,
                q(&[tree_sitter_toml_ng::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-yaml")]
            Self::Yaml => (
                tree_sitter_yaml::LANGUAGE,
                q(&[tree_sitter_yaml::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-rust")]
            Self::Rust => (
                tree_sitter_rust::LANGUAGE,
                q(&[tree_sitter_rust::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_rust::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-go")]
            Self::Go => (
                tree_sitter_go::LANGUAGE,
                q(&[tree_sitter_go::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-c")]
            Self::C => (
                tree_sitter_c::LANGUAGE,
                q(&[tree_sitter_c::HIGHLIGHT_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-cpp")]
            Self::Cpp => (
                tree_sitter_cpp::LANGUAGE,
                q(&[
                    tree_sitter_c::HIGHLIGHT_QUERY,
                    tree_sitter_cpp::HIGHLIGHT_QUERY,
                ]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-javascript")]
            Self::JavaScript => (
                tree_sitter_javascript::LANGUAGE,
                q(&[
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                ]),
                q(&[tree_sitter_javascript::INJECTIONS_QUERY]),
                q(&[tree_sitter_javascript::LOCALS_QUERY]),
            ),
            #[cfg(feature = "lang-jsdoc")]
            Self::JsDoc => (
                tree_sitter_jsdoc::LANGUAGE,
                q(&[tree_sitter_jsdoc::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-zig")]
            Self::Zig => (
                tree_sitter_zig::LANGUAGE,
                q(&[tree_sitter_zig::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_zig::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-java")]
            Self::Java => (
                tree_sitter_java::LANGUAGE,
                q(&[tree_sitter_java::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-python")]
            Self::Python => (
                tree_sitter_python::LANGUAGE,
                q(&[tree_sitter_python::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-ruby")]
            Self::Ruby => (
                tree_sitter_ruby::LANGUAGE,
                q(&[tree_sitter_ruby::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[tree_sitter_ruby::LOCALS_QUERY]),
            ),
            #[cfg(feature = "lang-bash")]
            Self::Bash => (
                tree_sitter_bash::LANGUAGE,
                q(&[tree_sitter_bash::HIGHLIGHT_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-html")]
            Self::Html => (
                tree_sitter_html::LANGUAGE,
                q(&[tree_sitter_html::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_html::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-css")]
            Self::Css => (
                tree_sitter_css::LANGUAGE,
                q(&[tree_sitter_css::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-swift")]
            Self::Swift => (
                tree_sitter_swift::LANGUAGE,
                q(&[tree_sitter_swift::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_swift::INJECTIONS_QUERY]),
                q(&[tree_sitter_swift::LOCALS_QUERY]),
            ),
            #[cfg(feature = "lang-scala")]
            Self::Scala => (
                tree_sitter_scala::LANGUAGE,
                q(&[tree_sitter_scala::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[tree_sitter_scala::LOCALS_QUERY]),
            ),
            #[cfg(feature = "lang-sql")]
            Self::Sql => (
                tree_sitter_sequel::LANGUAGE,
                q(&[tree_sitter_sequel::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-csharp")]
            Self::CSharp => (
                tree_sitter_c_sharp::LANGUAGE,
                q(&[tree_sitter_c_sharp::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-graphql")]
            Self::GraphQL => (tree_sitter_graphql::LANGUAGE, q(&[]), q(&[]), q(&[])),
            #[cfg(feature = "lang-proto")]
            Self::Proto => (
                tree_sitter_proto::LANGUAGE,
                q(&[tree_sitter_proto::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_proto::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-make")]
            Self::Make => (
                tree_sitter_make::LANGUAGE,
                q(&[tree_sitter_make::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-cmake")]
            Self::CMake => (
                tree_sitter_cmake::LANGUAGE,
                q(&[tree_sitter_cmake::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_cmake::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                q(&[
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                ]),
                q(&[tree_sitter_javascript::INJECTIONS_QUERY]),
                q(&[
                    tree_sitter_javascript::LOCALS_QUERY,
                    tree_sitter_typescript::LOCALS_QUERY,
                ]),
            ),
            #[cfg(feature = "lang-tsx")]
            Self::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX,
                q(&[
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                ]),
                q(&[tree_sitter_javascript::INJECTIONS_QUERY]),
                q(&[
                    tree_sitter_javascript::LOCALS_QUERY,
                    tree_sitter_typescript::LOCALS_QUERY,
                ]),
            ),
            #[cfg(feature = "lang-diff")]
            Self::Diff => (
                tree_sitter_diff::LANGUAGE,
                q(&[tree_sitter_diff::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-elixir")]
            Self::Elixir => (
                tree_sitter_elixir::LANGUAGE,
                q(&[tree_sitter_elixir::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_elixir::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-erb")]
            Self::Erb => (
                tree_sitter_embedded_template::LANGUAGE,
                q(&[tree_sitter_embedded_template::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_embedded_template::INJECTIONS_ERB_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-ejs")]
            Self::Ejs => (
                tree_sitter_embedded_template::LANGUAGE,
                q(&[tree_sitter_embedded_template::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_embedded_template::INJECTIONS_EJS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-php")]
            Self::Php => (
                tree_sitter_php::LANGUAGE_PHP,
                q(&[tree_sitter_php::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_php::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-astro")]
            Self::Astro => (
                tree_sitter_astro_next::LANGUAGE,
                q(&[tree_sitter_astro_next::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_astro_next::INJECTIONS_QUERY]),
                q(&[]),
            ),
            #[cfg(feature = "lang-kotlin")]
            Self::Kotlin => (
                tree_sitter_kotlin_sg::LANGUAGE,
                q(&[tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY]),
                q(&[]),
                q(&[]),
            ),
            #[cfg(feature = "lang-lua")]
            Self::Lua => (
                tree_sitter_lua::LANGUAGE,
                q(&[tree_sitter_lua::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_lua::INJECTIONS_QUERY]),
                q(&[tree_sitter_lua::LOCALS_QUERY]),
            ),
            #[cfg(feature = "lang-svelte")]
            Self::Svelte => (
                tree_sitter_svelte_next::LANGUAGE,
                q(&[tree_sitter_svelte_next::HIGHLIGHTS_QUERY]),
                q(&[tree_sitter_svelte_next::INJECTIONS_QUERY]),
                q(&[tree_sitter_svelte_next::LOCALS_QUERY]),
            ),
            _ => return GrammarConfig::plain(self.name()),
        };

        GrammarConfig::new(
            self.name(),
            language,
            self.injection_languages(),
            query,
            injection,
            locals,
        )
    }
}

/// Queries joined in order; a later pattern adds to an earlier one.
fn q(parts: &[&str]) -> String {
    parts.join("\n")
}
