//! The account's stable display as a terminal's shell names it, held or
//! plain: its aliases in the account's runtime directory and what programs on
//! it get (`shared_terminal_v1::terminal`), so raw commands in the shell run
//! on it as launched apps do, wherever it is shown.
use newport_protocol::display::environment::shared_terminal_v1;
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// A change to a shell's environment: a variable set, or removed.
pub(crate) type Change = (&'static str, Option<OsString>);

/// The file in the display's folder that records its X11 display number.
const X11_DISPLAY: &str = "x11-display";
/// The X11 display numbers the stable display takes one from when the
/// recorded one is in use.
const X11_DISPLAYS: std::ops::Range<u32> = 0..33;

/// What a shell gets for the display, with its X11 number `x11`.
fn names(runtime: &Path, x11: Option<u32>) -> Option<Vec<Change>> {
    let runtime = runtime.to_str()?;
    Some(
        shared_terminal_v1::terminal(runtime, x11, |name| std::env::var(name).ok())
            .into_iter()
            .map(|(name, value)| (name, value.map(Into::into)))
            .collect(),
    )
}

fn runtime() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|runtime| !runtime.is_empty())
        .map(PathBuf::from)
        .filter(|runtime| runtime.is_absolute())
}

/// The names of the running display, with the X11 number it serves now, if
/// any. Only a display in a folder private to this account counts.
pub(crate) fn running() -> Option<Vec<Change>> {
    let runtime = runtime()?;
    let folder = runtime.join(shared_terminal_v1::DIRECTORY);
    let endpoints = crate::gui_attachment::running_stable_display(&folder).ok()?;
    let x11 = match endpoints.display {
        Some(display) => Some(display.strip_prefix(':')?.parse().ok()?),
        None => None,
    };
    names(&runtime, x11)
}

/// The names for a shell whose computer will show the display, before it
/// runs. The X11 number is the one a run recorded, or one reserved here for
/// the display to take.
pub(crate) fn expected() -> Option<Vec<Change>> {
    let runtime = runtime()?;
    let folder = runtime.join(shared_terminal_v1::DIRECTORY);
    private_folder(&folder).ok()?;
    let number = x11_display(&folder, Path::new("/tmp")).ok()?;
    names(&runtime, Some(number))
}

/// Creates the stable display's folder as the display does, and refuses one
/// another account could reach.
fn private_folder(folder: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)?;
    let metadata = fs::symlink_metadata(folder)?;
    if !metadata.is_dir()
        || metadata.uid() != crate::paths::effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "The stable display's folder isn't private.",
        ));
    }
    Ok(())
}

/// The X11 display number recorded in `folder`, or the lowest one free in
/// `root` (no socket in `.X11-unix`, no lock), recorded there. The record is
/// never replaced, so shells started together agree on it.
pub(super) fn x11_display(folder: &Path, root: &Path) -> io::Result<u32> {
    let path = folder.join(X11_DISPLAY);
    let recorded = || {
        fs::read_to_string(&path)
            .ok()
            .and_then(|number| number.trim().parse::<u32>().ok())
    };
    if let Some(number) = recorded() {
        return Ok(number);
    }
    let missing = |path: std::path::PathBuf| {
        fs::symlink_metadata(path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
    };
    let number = X11_DISPLAYS
        .into_iter()
        .find(|number| {
            missing(root.join(format!(".X11-unix/X{number}")))
                && missing(root.join(format!(".X{number}-lock")))
        })
        .ok_or_else(|| io::Error::other("No X11 display number is free."))?;
    let mut staged = tempfile::NamedTempFile::new_in(folder)?;
    staged.write_all(number.to_string().as_bytes())?;
    match staged.persist_noclobber(&path) {
        Ok(_) => Ok(number),
        // Another shell, or the display, recorded one first.
        Err(error) => recorded().ok_or(error.error),
    }
}

/// `changes` as POSIX shell commands, for a plain terminal to evaluate.
pub(super) fn exports(changes: &[Change]) -> String {
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    changes
        .iter()
        .map(|(name, value)| match value {
            Some(value) => format!("export {name}={}\n", quote(&value.to_string_lossy())),
            None => format!("unset {name}\n"),
        })
        .collect()
}

/// `terminal display`: what a plain terminal's shell exports to name the
/// stable display, running or expected, exactly as a held shell gets it.
/// Newport runs it only where its computer may use the server's Remote apps.
pub(super) fn print() -> io::Result<()> {
    let changes = running()
        .or_else(expected)
        .ok_or_else(|| io::Error::other("The account has no runtime directory."))?;
    io::stdout().lock().write_all(exports(&changes).as_bytes())
}
