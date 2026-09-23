//! Shell exports for the currently enabled integrations.
use crate::paths::root;
use std::{env, fs, io};
pub fn print() -> io::Result<()> {
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
