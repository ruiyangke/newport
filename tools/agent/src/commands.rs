use crate::{snapshot, wire};
use std::{
    env, fs,
    io::{self, Read, Write},
    os::unix::{
        fs::{symlink, DirBuilderExt, MetadataExt, PermissionsExt},
        net::UnixStream,
    },
    path::PathBuf,
    process::Command,
    time::Duration,
};

pub fn root() -> io::Result<PathBuf> {
    let root =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?)
            .join(".cache/porthop/clipboard");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)?;
    let meta = fs::symlink_metadata(&root)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "clipboard directory must be owned by this account with mode 700",
        ));
    }
    Ok(root)
}
pub fn clipboard(args: &[String]) -> io::Result<()> {
    let mut target = "text/plain";
    let mut read = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "-out" => read = true,
            "-selection" | "-sel" | "-s" => {
                if !matches!(args.next().map(String::as_str), Some("clipboard" | "c")) {
                    return Err(io::Error::other(
                        "only the clipboard selection is supported",
                    ));
                }
            }
            "-target" | "-t" => {
                target = args
                    .next()
                    .ok_or_else(|| io::Error::other("missing target"))?
            }
            "-quiet" | "-silent" | "-verbose" => (),
            _ => {
                return Err(io::Error::other(
                    "usage: xclip -selection clipboard -o [-t FORMAT]; clipboard is read-only",
                ))
            }
        }
    }
    if !read {
        return Err(io::Error::other("clipboard is read-only; use -o"));
    }
    let formats = snapshot::read(&root()?.join("snapshot.tar"))?;
    if target == "TARGETS" {
        for key in formats.keys() {
            println!("{key}");
        }
        if formats.contains_key("text/plain") {
            println!("UTF8_STRING");
        }
        return Ok(());
    }
    if matches!(
        target,
        "UTF8_STRING" | "STRING" | "TEXT" | "text/plain;charset=utf-8"
    ) {
        target = "text/plain";
    }
    io::stdout().write_all(
        formats
            .get(target)
            .ok_or_else(|| io::Error::other("clipboard format unavailable"))?,
    )
}
pub fn wl_paste(args: &[String]) -> io::Result<()> {
    let mut xargs = vec!["-o".into()];
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" | "--no-newline" => (),
            "-l" | "--list-types" => xargs.extend(["-t".into(), "TARGETS".into()]),
            "-t" | "--type" => xargs.extend([
                "-t".into(),
                args.next()
                    .ok_or_else(|| io::Error::other("missing type"))?
                    .clone(),
            ]),
            _ => return Err(io::Error::other("unsupported wl-paste option")),
        }
    }
    clipboard(&xargs)
}
pub fn open(args: &[String], desktop: bool) -> io::Result<()> {
    if args.len() != 1 {
        return Err(io::Error::other("usage: porthop-agent open URL"));
    }
    let root = root()?;
    let wayland = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let fake_wayland = wayland == root.join("wayland.sock").to_string_lossy()
        || wayland.starts_with("/tmp/porthop-wl-");
    let fake_x = fs::read_to_string(root.join("display"))
        .ok()
        .is_some_and(|v| env::var("DISPLAY").ok().as_deref() == Some(v.trim()));
    let graphical = (env::var_os("DISPLAY").is_some() && !fake_x)
        || (!wayland.is_empty() && !fake_wayland)
        || env::var_os("WAYLAND_SOCKET").is_some();
    if desktop && graphical && env::var("PORTHOP_OPEN_ON_MAC").as_deref() != Ok("1") {
        if let Some(native) = crate::native::find("xdg-open") {
            use std::os::unix::process::CommandExt;
            return Err(Command::new(native).args(args).env_remove("BROWSER").exec());
        }
        return Err(io::Error::other(
            "desktop opener unavailable; use porthop-agent open URL for your Mac",
        ));
    }
    if wire::web_url(&args[0]).is_none() {
        return Err(io::Error::other("only HTTP and HTTPS URLs are supported"));
    }
    let mut stream = UnixStream::connect(root.join("agent.sock"))
        .map_err(|_| io::Error::other("enable Browser in Porthop’s Integration page first"))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(args[0].as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream.take(256).read_to_string(&mut reply)?;
    if reply != "ok" {
        return Err(io::Error::other(
            "browser request was not accepted; try again",
        ));
    }
    Ok(())
}
pub fn environment() -> io::Result<()> {
    let root = root()?;
    let quote = |s: &str| s.replace('\'', "'\\''");
    println!("export PATH=\"$HOME/.local/bin:$PATH\"");
    let features = fs::read_to_string(root.join("features")).unwrap_or_default();
    if features.split_whitespace().any(|f| f == "browser") {
        let home = env::var("HOME").map_err(io::Error::other)?;
        println!(
            "export BROWSER='{}'",
            quote(&format!("{home}/.local/bin/porthop-browser"))
        );
    }
    if features.split_whitespace().any(|f| f == "clipboard") {
        println!(
            "export WAYLAND_DISPLAY='{}'",
            quote(&root.join("wayland.sock").to_string_lossy())
        );
        if let Ok(display) = fs::read_to_string(root.join("display")) {
            println!("export DISPLAY='{}'", quote(display.trim()));
            println!(
                "export XAUTHORITY='{}'",
                quote(&root.join("Xauthority").to_string_lossy())
            );
        }
        println!("unset WAYLAND_SOCKET");
    }
    Ok(())
}
pub fn install() -> io::Result<()> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let dir = home.join(".local/bin");
    fs::create_dir_all(&dir)?;
    for name in ["xclip", "wl-paste", "xdg-open", "porthop-browser"] {
        let path = dir.join(name);
        let managed = fs::read_link(&path).ok().is_some_and(|v| {
            matches!(
                v.to_str(),
                Some("porthop-agent" | "porthop-clip" | "porthop-open")
            )
        });
        if managed {
            fs::remove_file(&path)?;
        }
        if name == "porthop-browser" && fs::symlink_metadata(&path).is_ok() {
            return Err(io::Error::other(
                "porthop-browser is occupied by an unrelated command",
            ));
        }
        if fs::symlink_metadata(&path).is_err() {
            symlink("porthop-agent", &path)?;
        }
    }
    // Remove only our obsolete Bash helpers, never unrelated commands.
    for (name, marker) in [
        ("porthop-clip", "# Porthop clipboard helper"),
        ("porthop-open", "# Porthop browser helper"),
    ] {
        let path = dir.join(name);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file())
            && fs::read_to_string(&path).is_ok_and(|s| s.lines().any(|l| l.starts_with(marker)))
        {
            fs::remove_file(path)?;
        }
    }
    let ours = env::current_exe()?.canonicalize()?;
    let resolved = env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("xclip"))
        .find(|p| p.is_file());
    let ready = resolved.and_then(|p| p.canonicalize().ok()).as_ref() == Some(&ours);
    println!(
        "PORTHOP_SHIM_PATH={}",
        if ready { "ready" } else { "missing" }
    );
    Ok(())
}
pub fn private_write(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
