//! Native persistence and executable checks, using only disposable profiles.
use crate::{command, ghostty, grammars, pdfium, process};
use anyhow::{Context, Result};

pub fn run(headless: bool) -> Result<()> {
    // Persistence checks for the services composed by the desktop application,
    // with corrupt-file and concurrency coverage.
    for filter in ["profile::", "preferences::", "metrics_db::"] {
        process::run(command("cargo").args([
            "test",
            "--locked",
            "-p",
            "newport-backend",
            "--lib",
            filter,
            "--",
            "--test-threads=1",
        ]))
        .with_context(|| format!("Native persistence checks failed: {filter}"))?;
    }

    // Grammar libraries and Windows PDFium are loaded beside the executable.
    // These staging helpers honor Cargo's target and target-directory settings.
    grammars::build(false, None).context("Staging native grammar resources")?;
    if cfg!(target_os = "windows") {
        pdfium::stage(false, None).context("Staging native Windows PDFium")?;
    }

    let env = ghostty::prepare()?;
    let mut lifecycle = command("cargo");
    env.apply(&mut lifecycle).args([
        "test",
        "--locked",
        "-p",
        "newport-app",
        "--test",
        "standalone_lifecycle",
    ]);
    lifecycle.args(["--", "--ignored", "--nocapture", "--test-threads=1"]);
    if headless {
        lifecycle.args(["--skip", "standalone_gui"]);
    }
    process::run(&mut lifecycle).context("Standalone native lifecycle checks failed")?;
    println!("PASS: native persistence and standalone lifecycle checks");
    if headless || !cfg!(target_os = "macos") {
        println!("GUI activation/shutdown acceptance is not covered by this headless run.");
    }
    Ok(())
}
