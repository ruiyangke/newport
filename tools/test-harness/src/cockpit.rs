use crate::{command, process, root};
use anyhow::Result;
use std::{fs, os::unix::fs::PermissionsExt};
pub fn run() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let dir = temp.path();
    fs::create_dir_all(dir.join("proc/net"))?;
    fs::create_dir(dir.join("bin"))?;
    for (path, text) in [
        ("proc/stat", "cpu 10 0 5 80 5 0 0 0 0 0\n"),
        (
            "proc/meminfo",
            "MemTotal: 1000 kB\nMemAvailable: 400 kB\nSwapTotal: 100 kB\nSwapFree: 60 kB\n",
        ),
        ("proc/uptime", "1234.5 987\n"),
        ("proc/loadavg", "1.00 2.00 3.00 1/12 55\n"),
        (
            "proc/net/dev",
            "eth0: 1000 1 0 0 0 0 0 0 2000 1 0 0 0 0 0 0\n",
        ),
        ("os-release", "PRETTY_NAME=\"Fixture Linux\"\n"),
    ] {
        fs::write(dir.join(path), text)?;
    }
    for (name,text)in [("ps"," 12 tester 4.2 512 S command\"with\\quotes\n"),("df","Filesystem 1-blocks Used Available Capacity Mounted on\n/dev/sda1 100000 40000 50000 44% /mount with spaces\n"),("getconf","8\n"),("uname","fixture-host\n"),("sleep","")]{let path=dir.join("bin").join(name);fs::write(&path,format!("#!/bin/sh\ncat <<'FIXTURE'\n{text}FIXTURE\n"))?;fs::set_permissions(path,fs::Permissions::from_mode(0o700))?;}
    let script = fs::read_to_string(root().join("src-tauri/src/cockpit.sh"))?
        .replace("/proc/", &format!("{}/proc/", dir.display()))
        .replace("/etc/os-release", &dir.join("os-release").to_string_lossy());
    let mut paths = vec![dir.join("bin")];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let data: serde_json::Value = serde_json::from_slice(&process::capture(
        command("sh")
            .arg("-s")
            .env("PATH", std::env::join_paths(paths)?),
        Some(script.as_bytes()),
    )?)?;
    assert_eq!(data["memoryUsed"], 600 * 1024);
    assert_eq!(data["swapUsed"], 40 * 1024);
    assert_eq!(data["processes"][0]["name"], "command\"with\\quotes");
    assert_eq!(data["processes"][0]["memory"], 512 * 1024);
    assert_eq!(data["disks"][0]["mount"], "/mount with spaces");
    assert_eq!(data["network"][0]["received"], 1000);
    assert_eq!(data["load"], serde_json::json!([1.0, 2.0, 3.0]));
    assert_eq!(data["cpu"].as_f64(), Some(0.0));
    assert_eq!(data["processCpuMode"], "lifetime");
    println!("PASS: cockpit metrics and JSON escaping");
    Ok(())
}
