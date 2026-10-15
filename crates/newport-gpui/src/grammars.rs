//! Register lightweight query metadata at startup. Native grammar libraries are
//! opened by parser factories only when that language is actually used.
use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};
use libloading::Library;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, c_char},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
struct Metadata {
    name: String,
    aliases: Vec<String>,
    injection_languages: Vec<String>,
    highlights: String,
    injections: String,
    locals: String,
}
#[derive(serde::Deserialize)]
struct Manifest {
    schema: u32,
    grammars: Vec<Metadata>,
}
type GrammarFn = unsafe extern "C" fn() -> *const ();
type MetadataFn = unsafe extern "C" fn(u32) -> *const c_char;
type FactoryFn = unsafe extern "C" fn(u32) -> Option<GrammarFn>;
struct Loaded {
    language: tree_sitter::Language,
    // Dropped after the language; successful entries are retained for process life.
    _library: Library,
}
struct Entry {
    metadata: Metadata,
    path: PathBuf,
    loaded: OnceLock<Result<Loaded, String>>,
}
impl Entry {
    fn language(&self) -> Result<tree_sitter::Language, String> {
        self.loaded
            .get_or_init(|| self.load())
            .as_ref()
            .map(|loaded| loaded.language.clone())
            .map_err(Clone::clone)
    }
    fn load(&self) -> Result<Loaded, String> {
        // SAFETY: Only fixed filenames under our installed resource directory are
        // accepted. A language name never becomes an arbitrary search path.
        let library = unsafe { Library::new(&self.path) }
            .map_err(|e| format!("{}: {e}", self.metadata.name))?;
        // SAFETY: Versioned C ABI uses integer indices, immutable UTF-8 metadata,
        // and Tree-sitter generated function pointers from the same build.
        let metadata = unsafe { library.get::<MetadataFn>(b"newport_grammar_metadata_v1\0") }
            .map_err(|e| e.to_string())?;
        let factory = unsafe { library.get::<FactoryFn>(b"newport_grammar_function_v1\0") }
            .map_err(|e| e.to_string())?;
        let ptr = unsafe { metadata(0) };
        if ptr.is_null() || !unsafe { metadata(1) }.is_null() {
            return Err("Expected exactly one grammar per library".into());
        }
        let actual: Metadata = serde_json::from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes())
            .map_err(|e| e.to_string())?;
        if actual != self.metadata {
            return Err(format!(
                "{} grammar does not match its catalog",
                self.metadata.name
            ));
        }
        let function = unsafe { factory(0) }.ok_or("Missing grammar function")?;
        let language = tree_sitter::Language::new(unsafe {
            tree_sitter_language::LanguageFn::from_raw(function)
        });
        tree_sitter::Parser::new()
            .set_language(&language)
            .map_err(|e| e.to_string())?;
        Ok(Loaded {
            language,
            _library: library,
        })
    }
}
struct Catalog {
    entries: BTreeMap<String, Arc<Entry>>,
}
impl Catalog {
    fn read(directory: &Path) -> Result<Self, String> {
        let path = directory.join("catalog.json");
        if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
            return Err("Grammar catalog is too large".into());
        }
        let manifest: Manifest =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let expected: Vec<String> =
            serde_json::from_str(include_str!("../../../resources/grammar-languages.json"))
                .map_err(|e| e.to_string())?;
        let names: BTreeSet<_> = manifest.grammars.iter().map(|m| m.name.clone()).collect();
        if manifest.schema != 1
            || names.len() != manifest.grammars.len()
            || names != expected.into_iter().collect()
        {
            return Err("Grammar catalog language inventory mismatch".into());
        }
        let mut aliases = BTreeSet::new();
        let mut entries = BTreeMap::new();
        for metadata in manifest.grammars {
            if !metadata.aliases.contains(&metadata.name)
                || metadata
                    .aliases
                    .iter()
                    .any(|alias| !aliases.insert(alias.clone()))
            {
                return Err("Invalid or duplicate grammar alias".into());
            }
            let filename = if cfg!(windows) {
                format!("newport_grammar_{}.dll", metadata.name)
            } else {
                format!(
                    "libnewport_grammar_{}.{}",
                    metadata.name,
                    if cfg!(target_os = "macos") {
                        "dylib"
                    } else {
                        "so"
                    }
                )
            };
            entries.insert(
                metadata.name.clone(),
                Arc::new(Entry {
                    metadata,
                    path: directory.join(filename),
                    loaded: OnceLock::new(),
                }),
            );
        }
        Ok(Self { entries })
    }
    fn register(&self) {
        let registry = LanguageRegistry::singleton();
        for entry in self.entries.values() {
            let meta = &entry.metadata;
            let mut config = GrammarConfig::plain(meta.name.clone());
            config.injection_languages = meta
                .injection_languages
                .iter()
                .cloned()
                .map(Into::into)
                .collect();
            config.highlights = meta.highlights.clone().into();
            config.injections = meta.injections.clone().into();
            config.locals = meta.locals.clone().into();
            for alias in &meta.aliases {
                registry.register(alias, &config);
            }
            let entry = entry.clone();
            registry.register_parser_factory(
                &meta.name,
                Arc::new(move || {
                    let language = entry.language().map_err(std::io::Error::other)?;
                    Ok((tree_sitter::Parser::new(), language))
                }),
            );
        }
    }
}
fn directory() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("Executable has no parent")?;
    if cfg!(target_os = "macos") && parent.file_name().is_some_and(|n| n == "MacOS") {
        return Ok(parent.join("../Resources/grammars"));
    }
    let parent = if parent.file_name().is_some_and(|n| n == "deps") {
        parent.parent().ok_or("Missing Cargo profile directory")?
    } else {
        parent
    };
    Ok(parent.join("grammars"))
}
static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();
pub(crate) fn init() -> Result<(), String> {
    CATALOG
        .get_or_init(|| {
            let catalog = Catalog::read(&directory()?)?;
            catalog.register();
            Ok(catalog)
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
/// Explicit packaging/diagnostic check. Normal startup never calls this.
pub(crate) fn validate_all() -> Result<(), String> {
    init()?;
    for entry in CATALOG
        .get()
        .unwrap()
        .as_ref()
        .map_err(Clone::clone)?
        .entries
        .values()
    {
        let language = entry.language()?;
        for query in [
            &entry.metadata.highlights,
            &entry.metadata.injections,
            &entry.metadata.locals,
        ] {
            tree_sitter::Query::new(&language, query)
                .map_err(|e| format!("{} query: {e}", entry.metadata.name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_lazy_and_loading_one_language_does_not_load_others() {
        let catalog = Catalog::read(&directory().unwrap()).unwrap();
        assert!(catalog.entries.values().all(|e| e.loaded.get().is_none()));
        let rust = &catalog.entries["rust"];
        let language = rust.language().unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).unwrap();
        assert!(
            !parser
                .parse("fn main() {}", None)
                .unwrap()
                .root_node()
                .has_error()
        );
        assert_eq!(
            catalog
                .entries
                .values()
                .filter(|e| e.loaded.get().is_some())
                .count(),
            1
        );
        rust.language().unwrap();
        assert_eq!(
            catalog
                .entries
                .values()
                .filter(|e| e.loaded.get().is_some())
                .count(),
            1
        );
    }
    #[test]
    fn failed_language_is_cached_and_does_not_disable_other_languages() {
        let mut catalog = Catalog::read(&directory().unwrap()).unwrap();
        let rust = Arc::get_mut(catalog.entries.get_mut("rust").unwrap()).unwrap();
        rust.path = PathBuf::from("/nonexistent/newport-grammar-rust");
        assert!(rust.language().is_err());
        assert!(rust.loaded.get().unwrap().is_err());
        assert!(catalog.entries["json"].language().is_ok());
    }
    #[test]
    fn registry_lazily_loads_aliases_and_embedded_languages() {
        // A fresh process proves startup laziness independently of other tests
        // that legitimately load grammars through the global registry.
        const ISOLATED: &str = "NEWPORT_TEST_LAZY_GRAMMARS";
        if std::env::var_os(ISOLATED).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "grammars::tests::registry_lazily_loads_aliases_and_embedded_languages",
                    "--nocapture",
                ])
                .env(ISOLATED, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        use gpui_kit::{
            base::input::Rope,
            component::highlighter::{HighlightTheme, SyntaxHighlighter},
        };
        init().unwrap();
        let catalog = CATALOG.get().unwrap().as_ref().unwrap();
        let loaded = || {
            catalog
                .entries
                .iter()
                .filter(|(_, e)| e.loaded.get().is_some())
                .map(|(name, _)| name.as_str())
                .collect::<BTreeSet<_>>()
        };
        assert!(loaded().is_empty());
        let registry = LanguageRegistry::singleton();
        let _js = SyntaxHighlighter::new("js");
        assert_eq!(loaded(), BTreeSet::from(["javascript"]));
        let _javascript = SyntaxHighlighter::new("javascript");
        assert_eq!(loaded(), BTreeSet::from(["javascript"]));
        for (name, source, embedded_token) in [
            (
                "html",
                "<style>body { color: red; }</style><script>const n = 42;</script>",
                "const",
            ),
            ("markdown", "Title\n\n```rust\nfn main() {}\n```\n", "fn"),
        ] {
            let mut highlighter = SyntaxHighlighter::new(name);
            let rope = Rope::from_str(source);
            assert!(highlighter.update(None, &rope, None));
            let styles =
                highlighter.styles(&(0..source.len()), HighlightTheme::default_light().as_ref());
            let offset = source.find(embedded_token).unwrap();
            assert!(
                styles.iter().any(|(range, _)| range.contains(&offset)),
                "{name} missing embedded highlighting"
            );
        }
        for name in ["html", "css", "javascript", "markdown", "rust"] {
            assert!(loaded().contains(name), "{name} was not loaded");
        }
        assert!(!loaded().contains("python"));
        // Missing libraries degrade just that language to plain text.
        let mut config = GrammarConfig::plain("test-missing-grammar");
        config.highlights = "(identifier) @variable".into();
        registry.register("test-missing-grammar", &config);
        registry.register_parser_factory(
            "test-missing-grammar",
            Arc::new(|| Err(std::io::Error::other("missing fixture").into())),
        );
        let mut fallback = SyntaxHighlighter::new("test-missing-grammar");
        fallback.update(None, &Rope::from_str("plain"), None);
        let mut plain = SyntaxHighlighter::new("text");
        plain.update(None, &Rope::from_str("plain"), None);
        assert_eq!(
            fallback.styles(&(0..5), HighlightTheme::default_light().as_ref()),
            plain.styles(&(0..5), HighlightTheme::default_light().as_ref())
        );
    }
    #[test]
    fn concurrent_requests_share_one_loaded_language() {
        let catalog = Catalog::read(&directory().unwrap()).unwrap();
        let entry = &catalog.entries["rust"];
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    entry.language().unwrap();
                });
            }
        });
        assert!(entry.loaded.get().unwrap().is_ok());
        assert_eq!(
            catalog
                .entries
                .values()
                .filter(|e| e.loaded.get().is_some())
                .count(),
            1
        );
    }
    #[test]
    fn mismatched_language_library_is_rejected() {
        let mut catalog = Catalog::read(&directory().unwrap()).unwrap();
        let json_path = catalog.entries["json"].path.clone();
        let rust = Arc::get_mut(catalog.entries.get_mut("rust").unwrap()).unwrap();
        rust.path = json_path;
        assert!(
            rust.language()
                .unwrap_err()
                .contains("does not match its catalog")
        );
    }
    #[test]
    fn all_bundled_grammars_have_valid_queries() {
        validate_all().unwrap();
    }
}
