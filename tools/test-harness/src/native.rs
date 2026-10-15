//! Native persistence and executable checks, using only disposable profiles.
use crate::{command, process};
use anyhow::{ensure, Context, Result};

pub fn run(args: &[String]) -> Result<()> {
    ensure!(
        args.is_empty() || args == ["--headless"],
        "Usage: native [--headless]"
    );
    let headless = args == ["--headless"];

    // Keep the persistence checks from the former desktop smoke test. These
    // exercise the same services composed by the standalone application, with
    // stronger migration, corrupt-file and concurrency coverage.
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
    process::run(command("node").arg("scripts/build-grammars.mjs"))
        .context("Staging native grammar resources")?;
    if cfg!(target_os = "windows") {
        process::run(command("node").arg("scripts/stage-pdfium.mjs"))
            .context("Staging native Windows PDFium")?;
    }

    let mut lifecycle = command("cargo");
    lifecycle.args([
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
