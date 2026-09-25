//! Read-only xclip and wl-paste command compatibility.
use crate::paths::root;
use std::io::{self, Write};
pub fn read(args: &[String]) -> io::Result<()> {
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
    let path = root()?.join("snapshot.tar");
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
        target,
        "UTF8_STRING" | "STRING" | "TEXT" | "text/plain;charset=utf-8"
    ) {
        target = "text/plain";
    }
    io::stdout().write_all(&crate::clipboard_source::get(&path, revision, target)?)
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
    read(&xargs)
}
