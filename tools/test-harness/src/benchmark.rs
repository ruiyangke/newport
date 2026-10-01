use crate::{command, contract::write_report, process, root};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::PathBuf};
pub fn run(args: &[String]) -> Result<()> {
    let mut options = HashMap::new();
    let mut sizes = vec![];
    let mut i = 0;
    while i < args.len() {
        ensure!(
            [
                "--agent",
                "--before",
                "--after",
                "--size",
                "--rounds",
                "--latency-ms",
                "--output"
            ]
            .contains(&args[i].as_str())
                && i + 1 < args.len(),
            "Invalid benchmark option"
        );
        if args[i] == "--size" {
            sizes.push(args[i + 1].clone());
        } else {
            options.insert(args[i].as_str(), args[i + 1].as_str());
        }
        i += 2;
    }
    let output = options
        .get("--output")
        .ok_or_else(|| anyhow::anyhow!("--output required"))?;
    let rounds: usize = options.get("--rounds").unwrap_or(&"3").parse()?;
    ensure!(rounds > 0, "Rounds must be positive");
    ensure!(
        options.contains_key("--before") == options.contains_key("--after"),
        "Supply both --before and --after"
    );
    ensure!(
        !(options.contains_key("--agent") && options.contains_key("--before")),
        "Choose --agent or --before/--after"
    );
    if sizes.is_empty() {
        sizes = vec!["small".into(), "medium".into(), "large".into()];
    }
    process::run(command("cargo").args([
        "build",
        "--manifest-path",
        "tools/agent/Cargo.toml",
        "--locked",
        "--release",
        "--example",
        "git_benchmark",
    ]))?;
    let variants: Vec<(&str, PathBuf)> = if let Some(before) = options.get("--before") {
        vec![
            ("before", fs::canonicalize(before)?),
            ("after", fs::canonicalize(options["--after"])?),
        ]
    } else {
        let binary = if let Some(agent) = options.get("--agent") {
            fs::canonicalize(agent)?
        } else {
            process::run(command("cargo").args([
                "build",
                "--manifest-path",
                "tools/agent/Cargo.toml",
                "--locked",
                "--release",
                "--bin",
                "newport-agent",
            ]))?;
            root().join("tools/agent/target/release/newport-agent")
        };
        vec![("current", binary)]
    };
    let temp = tempfile::tempdir()?;
    let mut reports = vec![];
    for size in sizes {
        for round in 0..rounds {
            let mut order = variants.clone();
            if round % 2 == 1 {
                order.reverse();
            }
            for (variant, agent) in order {
                let path = temp.path().join("report.json");
                process::run(
                    command(root().join("tools/agent/target/release/examples/git_benchmark"))
                        .arg("--agent")
                        .arg(agent)
                        .args([
                            "--size",
                            &size,
                            "--rounds",
                            "1",
                            "--latency-ms",
                            options.get("--latency-ms").unwrap_or(&"0"),
                            "--output",
                        ])
                        .arg(&path),
                )?;
                let mut report: Value = serde_json::from_slice(&fs::read(path)?)?;
                report["variant"] = json!(variant);
                report["round"] = json!(round);
                reports.push(report);
            }
        }
    }
    write_report(
        output,
        &json!({"protocol":"MessagePack v4","reports":reports}),
    )
}
