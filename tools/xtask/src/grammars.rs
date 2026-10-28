//! Tree-sitter grammar libraries staged beside the desktop executable.
use crate::util::{self, Os, Platform};
use crate::{command, process, root};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Language names from `resources/grammar-languages.json`.
pub fn languages() -> Result<Vec<String>> {
    util::read_json(&root().join("resources/grammar-languages.json"))
}

/// The shared-library file name for a language on an OS.
pub fn library(language: &str, os: Os) -> String {
    match os {
        Os::Windows => format!("newport_grammar_{language}.dll"),
        Os::Macos => format!("libnewport_grammar_{language}.dylib"),
        Os::Linux => format!("libnewport_grammar_{language}.so"),
    }
}

/// Every staged file name: `catalog.json` and each grammar library.
pub fn files(os: Os) -> Result<Vec<String>> {
    Ok(std::iter::once("catalog.json".to_owned())
        .chain(languages()?.iter().map(|language| library(language, os)))
        .collect())
}

/// The library Cargo builds for `newport-grammars`.
fn built_library(os: Os) -> &'static str {
    match os {
        Os::Windows => "newport_grammars.dll",
        Os::Macos => "libnewport_grammars.dylib",
        Os::Linux => "libnewport_grammars.so",
    }
}

/// Check the catalog executable's output against the language inventory.
fn check_catalog(output: &[u8], languages: &[String]) -> Result<()> {
    #[derive(Deserialize)]
    struct Catalog {
        schema: u32,
        grammars: Vec<Grammar>,
    }
    #[derive(Deserialize)]
    struct Grammar {
        name: String,
    }
    let catalog: Catalog = serde_json::from_slice(output).context("Invalid grammar catalog")?;
    let mut names: Vec<_> = catalog.grammars.into_iter().map(|g| g.name).collect();
    names.sort();
    ensure!(
        catalog.schema == 1 && names == languages,
        "Grammar catalog does not match the language inventory"
    );
    Ok(())
}

/// Give a staged macOS library its own `@rpath` identity and an ad-hoc signature.
fn identify(path: &Path, name: &str) -> Result<()> {
    util::run(
        command("install_name_tool")
            .args(["-id", &format!("@rpath/{name}")])
            .arg(path),
    )
    .context("Cannot set grammar library identity")?;
    crate::signing::codesign(path, "-", &[]).context("Cannot sign staged grammar")
}

/// Replace `<output>/grammars` with `staging`, restoring the previous directory on failure.
fn replace(output: &Path, staging: &Path) -> Result<()> {
    let current = output.join("grammars");
    let previous = output.join("grammars-previous");
    match fs::remove_dir_all(&previous) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    match fs::rename(&current, &previous) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    if let Err(error) = fs::rename(staging, &current) {
        let _ = fs::rename(&previous, &current);
        return Err(error).context("Cannot replace staged grammars");
    }
    let _ = fs::remove_dir_all(&previous);
    Ok(())
}

/// Build the catalog and each language library into `<output>/grammars`.
/// `cargo` runs `cargo build` with extra arguments, `catalog` runs the catalog
/// executable, and `identify` finishes a macOS library.
fn stage(
    output: &Path,
    os: Os,
    languages: &[String],
    cargo: &mut dyn FnMut(&[&str]) -> Result<()>,
    catalog: &dyn Fn(&Path) -> Result<Vec<u8>>,
    identify: &dyn Fn(&Path, &str) -> Result<()>,
) -> Result<PathBuf> {
    // The catalog executable is build tooling only; it is never copied into the application.
    cargo(&[
        "--bin",
        "newport-grammar-catalog",
        "--no-default-features",
        "--features",
        "catalog",
    ])
    .context("Grammar build failed")?;
    let executable = output.join(if os == Os::Windows {
        "newport-grammar-catalog.exe"
    } else {
        "newport-grammar-catalog"
    });
    let listing = catalog(&executable).context("Cannot generate grammar catalog on this host")?;
    check_catalog(&listing, languages)?;
    fs::create_dir_all(output)?;
    let staging = tempfile::Builder::new()
        .prefix("grammars-staging-")
        .tempdir_in(output)?;
    fs::write(staging.path().join("catalog.json"), listing.trim_ascii())?;
    for language in languages {
        let feature = format!("lang-{language}");
        cargo(&["--lib", "--no-default-features", "--features", &feature])
            .context("Grammar build failed")?;
        let name = library(language, os);
        let destination = staging.path().join(&name);
        fs::copy(output.join(built_library(os)), &destination)
            .with_context(|| format!("Cannot stage {name}"))?;
        if os == Os::Macos {
            identify(&destination, &name)?;
        }
    }
    // Replace only after every language builds; this also drops stale libraries.
    replace(output, staging.path())?;
    Ok(output.join("grammars"))
}

/// Build every grammar and stage it in `<target>/<profile>/grammars`; returns that directory.
pub fn build(release: bool, target: Option<&str>) -> Result<PathBuf> {
    let target = util::build_target(target);
    let target = target.as_deref();
    let os = Platform::of(target)?.os;
    let output = util::profile_directory(target, release)?;
    let languages = languages()?;
    let mut cargo = |args: &[&str]| {
        let mut c = command("cargo");
        c.args(["build", "--locked", "-p", "newport-grammars"])
            .args(args);
        if release {
            c.arg("--release");
        }
        if let Some(target) = target {
            c.args(["--target", target]);
        }
        util::run(util::without_release_secrets(&mut c))
    };
    let catalog = |executable: &Path| process::capture(&mut command(executable), None);
    let staged = stage(&output, os, &languages, &mut cargo, &catalog, &identify)?;
    println!(
        "Staged {} language libraries in {}",
        languages.len(),
        staged.display()
    );
    Ok(staged)
}

/// The newest modification time among the files the grammar libraries are built from.
fn newest_input(root: &Path) -> Result<SystemTime> {
    let mut newest = SystemTime::UNIX_EPOCH;
    let files = [
        root.join("Cargo.lock"),
        root.join("resources/grammar-languages.json"),
    ];
    for entry in walkdir::WalkDir::new(root.join("crates/newport-grammars"))
        .into_iter()
        .map(|entry| entry.map(walkdir::DirEntry::into_path))
        .chain(files.into_iter().map(Ok))
    {
        newest = newest.max(fs::metadata(entry?)?.modified()?);
    }
    Ok(newest)
}

/// True when `staged` holds exactly `files` and was staged after every input changed.
fn current(staged: &Path, files: &[String], newest_input: SystemTime) -> bool {
    let Ok(staged_at) = fs::metadata(staged).and_then(|m| m.modified()) else {
        return false;
    };
    let Ok(entries) = fs::read_dir(staged) else {
        return false;
    };
    let mut names: Vec<_> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    let mut expected = files.to_vec();
    expected.sort();
    names == expected && staged_at >= newest_input
}

/// [`build`], unless the staged grammars are already current for their sources.
pub fn ensure(release: bool, target: Option<&str>) -> Result<PathBuf> {
    let target = util::build_target(target);
    let os = Platform::of(target.as_deref())?.os;
    let staged = util::profile_directory(target.as_deref(), release)?.join("grammars");
    if current(&staged, &files(os)?, newest_input(&root())?) {
        return Ok(staged);
    }
    eprintln!("Grammar libraries are missing or out of date; building them");
    build(release, target.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_grammars_are_current_only_when_complete_and_newer_than_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let staged = directory.path().join("grammars");
        let files = vec!["catalog.json".to_owned(), library("zig", Os::Macos)];
        assert!(!current(&staged, &files, SystemTime::UNIX_EPOCH));
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("catalog.json"), "{}").unwrap();
        assert!(!current(&staged, &files, SystemTime::UNIX_EPOCH));
        fs::write(staged.join(&files[1]), "").unwrap();
        assert!(current(&staged, &files, SystemTime::UNIX_EPOCH));
        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        assert!(!current(&staged, &files, later));
        fs::write(staged.join("libnewport_grammar_old.dylib"), "").unwrap();
        assert!(!current(&staged, &files, SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn grammar_inputs_include_the_crate_and_the_inventory() {
        assert!(newest_input(&root()).unwrap() > SystemTime::UNIX_EPOCH);
    }

    fn catalog_json(languages: &[String]) -> String {
        let grammars: Vec<_> = languages
            .iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect();
        serde_json::json!({ "schema": 1, "grammars": grammars }).to_string()
    }

    #[test]
    fn library_names_follow_the_platform() {
        assert_eq!(library("zig", Os::Windows), "newport_grammar_zig.dll");
        assert_eq!(library("zig", Os::Macos), "libnewport_grammar_zig.dylib");
        assert_eq!(library("zig", Os::Linux), "libnewport_grammar_zig.so");
        let files = files(Os::Windows).unwrap();
        assert_eq!(files[0], "catalog.json");
        assert_eq!(files.len(), languages().unwrap().len() + 1);
    }

    #[test]
    fn catalog_must_match_the_inventory() {
        let languages = languages().unwrap();
        check_catalog(catalog_json(&languages).as_bytes(), &languages).unwrap();
        let error =
            check_catalog(catalog_json(&languages[1..]).as_bytes(), &languages).unwrap_err();
        assert!(error.to_string().contains("does not match"));
        let wrong = catalog_json(&languages).replace("\"schema\":1", "\"schema\":2");
        assert!(check_catalog(wrong.as_bytes(), &languages).is_err());
    }

    #[test]
    fn staging_replaces_atomically_and_keeps_the_previous_output_on_failure() {
        let languages = languages().unwrap();
        let catalog = catalog_json(&languages);
        for os in [Os::Windows, Os::Macos] {
            let directory = tempfile::tempdir().unwrap();
            let output = directory.path().join("debug");
            let name = built_library(os);
            fs::create_dir_all(output.join("grammars")).unwrap();
            fs::write(output.join(name), format!("Fixture for {os:?}")).unwrap();
            fs::write(output.join("grammars").join(name), "stale monolith").unwrap();
            let listing = |_: &Path| Ok(format!("{catalog}\n").into_bytes());
            let identified = std::cell::Cell::new(0);
            let identify = |path: &Path, name: &str| {
                assert_eq!(path.file_name().unwrap(), name);
                identified.set(identified.get() + 1);
                Ok(())
            };
            let mut builds = Vec::new();
            let mut cargo = |args: &[&str]| {
                builds.push(args.join(" "));
                Ok(())
            };
            let staged = stage(&output, os, &languages, &mut cargo, &listing, &identify).unwrap();
            assert_eq!(staged, output.join("grammars"));
            assert_eq!(builds.len(), languages.len() + 1);
            assert!(builds[0].contains("--features catalog"));
            assert!(
                builds[1..]
                    .iter()
                    .all(|b| b.starts_with("--lib --no-default-features"))
            );
            let expected_identities = if os == Os::Macos { languages.len() } else { 0 };
            assert_eq!(identified.get(), expected_identities);
            let mut staged_files: Vec<_> = fs::read_dir(&staged)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            staged_files.sort();
            let mut expected = files(os).unwrap();
            expected.sort();
            assert_eq!(staged_files, expected);
            for file in files(os).unwrap() {
                let content = fs::read_to_string(staged.join(&file)).unwrap();
                if file == "catalog.json" {
                    assert_eq!(content, catalog);
                } else {
                    assert_eq!(content, format!("Fixture for {os:?}"));
                }
            }

            let mut failing = |args: &[&str]| {
                anyhow::ensure!(!args.contains(&"lang-zig"), "exit status: 88");
                Ok(())
            };
            let error =
                stage(&output, os, &languages, &mut failing, &listing, &identify).unwrap_err();
            assert!(error.to_string().contains("Grammar build failed"));
            assert_eq!(
                fs::read_to_string(staged.join("catalog.json")).unwrap(),
                catalog
            );
            assert!(!fs::read_dir(&output).unwrap().any(|e| {
                let name = e.unwrap().file_name().into_string().unwrap();
                name.starts_with("grammars-staging-") || name == "grammars-previous"
            }));
        }
    }

    #[test]
    fn a_catalog_that_cannot_run_stops_before_staging() {
        let languages = languages().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("debug");
        let error = stage(
            &output,
            Os::Linux,
            &languages,
            &mut |_| Ok(()),
            &|_| anyhow::bail!("exec format error"),
            &|_, _| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("on this host"));
        assert!(!output.join("grammars").exists());
    }
}
