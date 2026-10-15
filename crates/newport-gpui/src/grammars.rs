//! Load only our bundled, versioned grammar ABI; keep the library alive as long
//! as any Tree-sitter language, parser, query, or syntax tree may reference it.
use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};
use libloading::Library;
use std::{
    ffi::{CStr, c_char},
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(serde::Deserialize)]
struct Metadata {
    name: String,
    aliases: Vec<String>,
    injection_languages: Vec<String>,
    highlights: String,
    injections: String,
    locals: String,
}
type GrammarFn = unsafe extern "C" fn() -> *const ();
type MetadataFn = unsafe extern "C" fn(u32) -> *const c_char;
type FactoryFn = unsafe extern "C" fn(u32) -> Option<GrammarFn>;

fn library_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libnewport_grammars.dylib"
    } else if cfg!(windows) {
        "newport_grammars.dll"
    } else {
        "libnewport_grammars.so"
    }
}
fn library_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("Executable has no parent directory")?;
    if cfg!(target_os = "macos") && parent.file_name().is_some_and(|n| n == "MacOS") {
        return Ok(parent.join("../Resources/grammars").join(library_name()));
    }
    // Cargo test executables live one directory below the staged resources.
    let parent = if parent.file_name().is_some_and(|n| n == "deps") {
        parent.parent().ok_or("Missing Cargo profile directory")?
    } else {
        parent
    };
    Ok(parent.join("grammars").join(library_name()))
}

pub(crate) fn init() -> Result<(), String> {
    static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
    LIBRARY.get_or_init(|| {
        let path = library_path()?;
        load(&path).map_err(|e| format!("Could not load bundled grammars at {}: {e}. Run node scripts/build-grammars.mjs for development builds.", path.display()))
    }).as_ref().map(|_| ()).map_err(Clone::clone)
}

fn load(path: &Path) -> Result<Library, String> {
    // SAFETY: The absolute installation path selects our shipped native library,
    // never a bare name searched through PATH or the current working directory.
    let library = unsafe { Library::new(path) }.map_err(|e| e.to_string())?;
    // SAFETY: v1 exports use only C function pointers, integers and NUL-terminated
    // UTF-8 JSON. The matching library is built from the same Cargo.lock.
    let metadata = unsafe { library.get::<MetadataFn>(b"newport_grammar_metadata_v1\0") }
        .map_err(|e| e.to_string())?;
    let factory = unsafe { library.get::<FactoryFn>(b"newport_grammar_function_v1\0") }
        .map_err(|e| e.to_string())?;
    let mut configs = Vec::new();
    for index in 0..256 {
        // SAFETY: Shipped v1 metadata is immutable and lives with the library.
        let ptr = unsafe { metadata(index) };
        if ptr.is_null() {
            break;
        }
        let meta: Metadata = serde_json::from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes())
            .map_err(|e| e.to_string())?;
        let function =
            unsafe { factory(index) }.ok_or_else(|| format!("Missing grammar {}", meta.name))?;
        // SAFETY: The export returns Tree-sitter-generated grammar functions.
        let language = tree_sitter::Language::new(unsafe {
            tree_sitter_language::LanguageFn::from_raw(function)
        });
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&language)
            .map_err(|e| format!("{}: {e}", meta.name))?;
        for query in [&meta.highlights, &meta.injections, &meta.locals] {
            tree_sitter::Query::new(&language, query)
                .map_err(|e| format!("{} query: {e}", meta.name))?;
        }
        let config = GrammarConfig::new(
            meta.name,
            language,
            meta.injection_languages
                .into_iter()
                .map(Into::into)
                .collect(),
            &meta.highlights,
            &meta.injections,
            &meta.locals,
        );
        configs.push((meta.aliases, config));
    }
    if configs.is_empty() {
        return Err("Grammar library contains no languages".into());
    }
    // Publish only after every grammar has passed ABI/query validation. The
    // caller's static OnceLock retains the library for the process lifetime.
    for (aliases, config) in configs {
        let registry = LanguageRegistry::singleton();
        registry.register(config.name.as_ref(), &config);
        for alias in aliases {
            registry.register(&alias, &config);
        }
    }
    Ok(library)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_grammars_parse_and_preserve_aliases() {
        init().unwrap();
        let registry = LanguageRegistry::singleton();
        for name in registry.languages() {
            let config = registry.language(&name).unwrap();
            if let Some(language) = config.language {
                let mut parser = tree_sitter::Parser::new();
                parser.set_language(&language).unwrap();
                assert!(parser.parse("hello", None).is_some(), "{name}");
            }
        }
        assert_eq!(registry.language("rs").unwrap().name, "rust");
        assert_eq!(registry.language("ts").unwrap().name, "typescript");
        let language = registry.language("rust").unwrap().language.unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).unwrap();
        assert!(
            !parser
                .parse("fn main() {}", None)
                .unwrap()
                .root_node()
                .has_error()
        );
    }
    #[test]
    fn missing_library_is_reported() {
        assert!(load(Path::new("/nonexistent/newport-grammars-library")).is_err());
    }
}
