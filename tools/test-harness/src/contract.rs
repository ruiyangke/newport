use crate::{command, process, remote};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::Path};
pub fn write_report(path: &str, value: &Value) -> Result<()> {
    if let Some(dir) = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}
pub fn run(local: bool, args: &[String]) -> Result<()> {
    let mut frontend = false;
    let mut output = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frontend" => frontend = true,
            "--output" | "--metrics-output" => {
                i += 1;
                output = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("Missing output path"))?
                        .clone(),
                );
            }
            _ => anyhow::bail!("Unknown contract option: {}", args[i]),
        }
        i += 1;
    }
    let temp = tempfile::tempdir()?;
    let trace = temp.path().join("trace.ndjson");
    if local {
        process::run(
            command("cargo")
                .args([
                    "test",
                    "--manifest-path",
                    "tools/agent/Cargo.toml",
                    "--locked",
                    "--lib",
                    "git::cli::",
                    "--",
                    "--test-threads=1",
                ])
                .env("NEWPORT_CLI_TRACE_PATH", &trace),
        )?;
    } else {
        std::env::set_var("NEWPORT_GIT_TRACE_PATH", &trace);
        remote::run(&["--filter".into(), "remote_tests::git::".into()])?;
    }
    let rows = fs::read_to_string(&trace)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()?;
    ensure!(!rows.is_empty(), "Contract test produced no responses");
    let methods: BTreeSet<_> = rows
        .iter()
        .filter_map(|r| r["request"]["method"].as_str())
        .collect();
    let actions: BTreeSet<_> = rows
        .iter()
        .filter(|r| {
            matches!(
                r["response"]["state"].as_str(),
                Some("succeeded" | "needs_resolution")
            )
        })
        .filter_map(|r| r["request"]["params"]["action"]["kind"].as_str())
        .collect();
    if frontend {
        process::run(
            command("node")
                .args([
                    "node_modules/vitest/vitest.mjs",
                    "run",
                    "src/api/gitLiveContract.test.ts",
                ])
                .env("NEWPORT_GIT_TRACE_PATH", &trace),
        )?;
    }
    let samples:Vec<Value>=rows.iter().map(|r|json!({"method":r["request"]["method"],"action":r["request"]["params"]["action"]["kind"],"state":r["response"]["state"],"errorCode":r["error"]["code"],"measurements":r["measurements"]})).collect();
    if let Some(output) = output {
        write_report(
            &output,
            &json!({"backend":"cli","transport":if local{"service-level"}else{"OpenSSH MessagePack v4"},"frontendVerified":frontend,"responses":rows.len(),"methods":methods,"actions":actions,"samples":samples}),
        )?;
    }
    println!(
        "Contract verified: {} methods, {} actions, {} responses",
        methods.len(),
        actions.len(),
        rows.len()
    );
    Ok(())
}
