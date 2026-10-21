//! `Newport.app`: Info.plist, stripped binaries, ad-hoc signatures and a `ditto` zip.
use super::{copy_file, probe, Staged};
use crate::util::{self, Os};
use crate::{build_info, command, grammars, signing};
use anyhow::{Context, Result};
use std::{fs, path::PathBuf};

/// Finish the staged bundle, run its self-checks and return the zip.
pub(super) fn finish(staged: &Staged) -> Result<PathBuf> {
    let layout = staged.layout;
    let executable = &layout.executable;
    copy_file(
        &staged.root.join("resources/icons/icon.icns"),
        &layout.resources.join("icon.icns"),
    )?;
    let info = build_info::read(executable, staged.expect)?;
    fs::write(
        layout.app.join("Contents/Info.plist"),
        build_info::mac_info_plist(&info),
    )
    .context("Cannot write Info.plist")?;
    util::run(command("xcrun").arg("strip").arg(executable))?;
    for name in grammars::files(Os::Macos)?
        .iter()
        .filter(|name| name.ends_with(".dylib"))
    {
        let library = layout.resources.join("grammars").join(name);
        util::run(command("xcrun").args(["strip", "-x"]).arg(&library))?;
        signing::codesign(&library, "-", &[])?;
    }
    signing::codesign(&layout.app, "-", &[])?;
    for check in ["--check-needle", "--check-grammars", "--check-pdf"] {
        probe(executable, check)?;
    }
    let archive = staged.stage.join("Newport-macos.zip");
    util::run(
        command("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&layout.app)
            .arg(&archive),
    )?;
    Ok(archive)
}
