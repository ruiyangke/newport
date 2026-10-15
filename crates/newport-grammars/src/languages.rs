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

    /// Return the language info for the language.
    ///
    /// (language, query, injection, locals)
    pub(super) fn config(&self) -> GrammarConfig {
        #[allow(unreachable_patterns)]
        let (language, query, injection, locals) = match self {
            Self::Plain => return GrammarConfig::plain(self.name()),
            #[cfg(feature = "lang-json")]
            Self::Json => (
                tree_sitter_json::LANGUAGE,
                include_str!("languages/json/highlights.scm"),
                "",
                "",
            ),
            #[cfg(feature = "lang-markdown")]
            Self::Markdown => (
                tree_sitter_md::LANGUAGE,
                include_str!("languages/markdown/highlights.scm"),
                include_str!("languages/markdown/injections.scm"),
                "",
            ),
            #[cfg(feature = "lang-markdown_inline")]
            Self::MarkdownInline => (
                tree_sitter_md::INLINE_LANGUAGE,
                include_str!("languages/markdown_inline/highlights.scm"),
                "",
                "",
            ),
            #[cfg(feature = "lang-toml")]
            Self::Toml => (
                tree_sitter_toml_ng::LANGUAGE,
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-yaml")]
            Self::Yaml => (
                tree_sitter_yaml::LANGUAGE,
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-rust")]
            Self::Rust => (
                tree_sitter_rust::LANGUAGE,
                include_str!("languages/rust/highlights.scm"),
                include_str!("languages/rust/injections.scm"),
                "",
            ),
            #[cfg(feature = "lang-go")]
            Self::Go => (
                tree_sitter_go::LANGUAGE,
                include_str!("languages/go/highlights.scm"),
                "",
                "",
            ),
            #[cfg(feature = "lang-c")]
            Self::C => (
                tree_sitter_c::LANGUAGE,
                tree_sitter_c::HIGHLIGHT_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-cpp")]
            Self::Cpp => (
                tree_sitter_cpp::LANGUAGE,
                tree_sitter_cpp::HIGHLIGHT_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-javascript")]
            Self::JavaScript => (
                tree_sitter_javascript::LANGUAGE,
                include_str!("languages/javascript/highlights.scm"),
                include_str!("languages/javascript/injections.scm"),
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            #[cfg(feature = "lang-jsdoc")]
            Self::JsDoc => (
                tree_sitter_jsdoc::LANGUAGE,
                tree_sitter_jsdoc::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-zig")]
            Self::Zig => (
                tree_sitter_zig::LANGUAGE,
                include_str!("languages/zig/highlights.scm"),
                include_str!("languages/zig/injections.scm"),
                "",
            ),
            #[cfg(feature = "lang-java")]
            Self::Java => (
                tree_sitter_java::LANGUAGE,
                tree_sitter_java::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-python")]
            Self::Python => (
                tree_sitter_python::LANGUAGE,
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-ruby")]
            Self::Ruby => (
                tree_sitter_ruby::LANGUAGE,
                tree_sitter_ruby::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_ruby::LOCALS_QUERY,
            ),
            #[cfg(feature = "lang-bash")]
            Self::Bash => (
                tree_sitter_bash::LANGUAGE,
                tree_sitter_bash::HIGHLIGHT_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-html")]
            Self::Html => (
                tree_sitter_html::LANGUAGE,
                include_str!("languages/html/highlights.scm"),
                include_str!("languages/html/injections.scm"),
                "",
            ),
            #[cfg(feature = "lang-css")]
            Self::Css => (
                tree_sitter_css::LANGUAGE,
                tree_sitter_css::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-swift")]
            Self::Swift => (tree_sitter_swift::LANGUAGE, "", "", ""),
            #[cfg(feature = "lang-scala")]
            Self::Scala => (
                tree_sitter_scala::LANGUAGE,
                tree_sitter_scala::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_scala::LOCALS_QUERY,
            ),
            #[cfg(feature = "lang-sql")]
            Self::Sql => (
                tree_sitter_sequel::LANGUAGE,
                tree_sitter_sequel::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-csharp")]
            Self::CSharp => (tree_sitter_c_sharp::LANGUAGE, "", "", ""),
            #[cfg(feature = "lang-graphql")]
            Self::GraphQL => (tree_sitter_graphql::LANGUAGE, "", "", ""),
            #[cfg(feature = "lang-proto")]
            Self::Proto => (tree_sitter_proto::LANGUAGE, "", "", ""),
            #[cfg(feature = "lang-make")]
            Self::Make => (
                tree_sitter_make::LANGUAGE,
                tree_sitter_make::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-cmake")]
            Self::CMake => (tree_sitter_cmake::LANGUAGE, "", "", ""),
            #[cfg(feature = "lang-typescript")]
            Self::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                include_str!("languages/typescript/highlights.scm"),
                include_str!("languages/javascript/injections.scm"),
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            #[cfg(feature = "lang-tsx")]
            Self::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            #[cfg(feature = "lang-diff")]
            Self::Diff => (
                tree_sitter_diff::LANGUAGE,
                tree_sitter_diff::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            #[cfg(feature = "lang-elixir")]
            Self::Elixir => (
                tree_sitter_elixir::LANGUAGE,
                tree_sitter_elixir::HIGHLIGHTS_QUERY,
                tree_sitter_elixir::INJECTIONS_QUERY,
                "",
            ),
            #[cfg(feature = "lang-erb")]
            Self::Erb => (
                tree_sitter_embedded_template::LANGUAGE,
                tree_sitter_embedded_template::HIGHLIGHTS_QUERY,
                tree_sitter_embedded_template::INJECTIONS_EJS_QUERY,
                "",
            ),
            #[cfg(feature = "lang-ejs")]
            Self::Ejs => (
                tree_sitter_embedded_template::LANGUAGE,
                tree_sitter_embedded_template::HIGHLIGHTS_QUERY,
                tree_sitter_embedded_template::INJECTIONS_EJS_QUERY,
                "",
            ),
            #[cfg(feature = "lang-php")]
            Self::Php => (
                tree_sitter_php::LANGUAGE_PHP,
                tree_sitter_php::HIGHLIGHTS_QUERY,
                include_str!("languages/php/injections.scm"),
                "",
            ),
            #[cfg(feature = "lang-astro")]
            Self::Astro => (
                tree_sitter_astro_next::LANGUAGE,
                tree_sitter_astro_next::HIGHLIGHTS_QUERY,
                tree_sitter_astro_next::INJECTIONS_QUERY,
                "",
            ),
            #[cfg(feature = "lang-kotlin")]
            Self::Kotlin => (
                tree_sitter_kotlin_sg::LANGUAGE,
                include_str!("languages/kotlin/highlights.scm"),
                "",
                "",
            ),
            #[cfg(feature = "lang-lua")]
            Self::Lua => (
                tree_sitter_lua::LANGUAGE,
                include_str!("languages/lua/highlights.scm"),
                tree_sitter_lua::INJECTIONS_QUERY,
                tree_sitter_lua::LOCALS_QUERY,
            ),
            #[cfg(feature = "lang-svelte")]
            Self::Svelte => (
                tree_sitter_svelte_next::LANGUAGE,
                tree_sitter_svelte_next::HIGHLIGHTS_QUERY,
                tree_sitter_svelte_next::INJECTIONS_QUERY,
                tree_sitter_svelte_next::LOCALS_QUERY,
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
