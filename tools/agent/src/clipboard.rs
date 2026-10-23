//! `xclip`, `xsel`, and `wl-paste` as Needle's commands. Like `xdg-open`, they serve
//! what Newport provides, reading the desktop's clipboard through whichever
//! session is live when they run, and hand everything else to the server's
//! own command: writes, other selections, and a desktop of the server's own.
use std::{
    io::{self, Write},
    os::unix::process::CommandExt,
    process::Command,
};

pub fn read(args: &[String]) -> io::Result<()> {
    serve("xclip", args, xclip_request(args))
}

pub fn wl_paste(args: &[String]) -> io::Result<()> {
    serve("wl-paste", args, wl_paste_request(args))
}

pub fn xsel(args: &[String]) -> io::Result<()> {
    serve("xsel", args, xsel_request(args))
}

/// clipboardy (used by OpenCode v1) reads text with `xsel --clipboard --output`.
/// xsel defaults to PRIMARY, so both the selection and read must be explicit.
fn xsel_request(args: &[String]) -> io::Result<String> {
    let mut clipboard = false;
    let mut output = false;
    for arg in args {
        match arg.as_str() {
            "--clipboard" => clipboard = true,
            "--output" => output = true,
            short if short.starts_with('-') && short.len() > 1 => {
                for option in short[1..].chars() {
                    match option {
                        'b' => clipboard = true,
                        'o' => output = true,
                        _ => return Err(io::Error::other("unsupported xsel option")),
                    }
                }
            }
            _ => return Err(io::Error::other("unsupported xsel option")),
        }
    }
    if !clipboard || !output {
        return Err(io::Error::other(
            "usage: xsel --clipboard --output; clipboard is read-only",
        ));
    }
    Ok("text/plain".into())
}

/// The format an `xclip` read asks for, or why Newport doesn't serve it.
fn xclip_request(args: &[String]) -> io::Result<String> {
    let mut target = "text/plain".to_owned();
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
                    .clone()
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
    Ok(target)
}

/// The format a `wl-paste` read asks for, or why Newport doesn't serve it.
fn wl_paste_request(args: &[String]) -> io::Result<String> {
    let mut target = "text/plain".to_owned();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" | "--no-newline" => (),
            "-l" | "--list-types" => target = "TARGETS".into(),
            "-t" | "--type" => {
                target = args
                    .next()
                    .ok_or_else(|| io::Error::other("missing type"))?
                    .clone()
            }
            _ => return Err(io::Error::other("unsupported wl-paste option")),
        }
    }
    Ok(target)
}

fn serve(name: &str, args: &[String], request: io::Result<String>) -> io::Result<()> {
    let selection = crate::routing::select(Some("clipboard"));
    let root = selection.as_ref().ok().map(|s| s.root.as_path());
    if request.is_err() || crate::native::real_desktop(root) {
        if let Some(native) = crate::native::find(name) {
            return Err(Command::new(native).args(args).exec());
        }
    }
    let mut target = request?;
    let path = selection?.root.join("snapshot.tar");
    let (revision, formats) = crate::clipboard_source::offer(&path)?;
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
        target.as_str(),
        "UTF8_STRING" | "STRING" | "TEXT" | "text/plain;charset=utf-8"
    ) {
        target = "text/plain".into();
    }
    io::stdout().write_all(&crate::clipboard_source::get(&path, revision, &target)?)
}
