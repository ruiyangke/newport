//! `Newport.app`: Info.plist, stripped binaries, ad-hoc signatures and a `ditto` zip.
use super::{Staged, copy_file, probe};
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
    // A distribution built from an explicit signed full-agent export must embed
    // exactly that export before Developer ID signing or notarization.
    if let (Some(export), Some(_)) = (
        std::env::var_os("NEWPORT_AGENT_RESOURCE_DIR"),
        std::env::var_os("NEWPORT_REQUIRE_NEEDLE_RELEASE"),
    ) {
        newport_release::desktop_agent::verify_desktop_agent(
            &layout.app,
            std::path::Path::new(&export),
            &util::app()?.version,
            &util::updater_public_key()?,
            &newport_release::desktop_agent::run_probe,
        )
        .context("The packaged desktop does not embed the requested signed agent export")?;
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
