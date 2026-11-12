//! `newport-agent clipboard`: reads the clipboard of the computer a request
//! follows (see `routing`), whatever display the shell names.
use std::io::{self, Write};

const USAGE: &str = "Usage: newport-agent clipboard -o [-t FORMAT]";

pub fn read(args: &[String]) -> io::Result<()> {
    let mut target = request(args)?;
    let selection = crate::routing::select_for("clipboard")?;
    let path = selection.root.join("snapshot.tar");
    let unavailable = |error: io::Error| selection.clipboard_unavailable(error);
    let (revision, formats) = crate::clipboard_source::offer(&path).map_err(&unavailable)?;
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
    io::stdout()
        .write_all(&crate::clipboard_source::get(&path, revision, &target).map_err(unavailable)?)
}

/// The format a read asks for.
fn request(args: &[String]) -> io::Result<String> {
    let mut target = "text/plain".to_owned();
    let mut read = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" => read = true,
            "-t" => target = args.next().ok_or_else(|| io::Error::other(USAGE))?.clone(),
            _ => return Err(io::Error::other(USAGE)),
        }
    }
    if !read {
        return Err(io::Error::other(USAGE));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn a_read_names_its_format_and_anything_else_shows_the_usage() {
        assert_eq!(request(&args(&["-o"])).unwrap(), "text/plain");
        assert_eq!(
            request(&args(&["-o", "-t", "image/png"])).unwrap(),
            "image/png"
        );
        assert_eq!(request(&args(&["-t", "TARGETS", "-o"])).unwrap(), "TARGETS");
        for refused in [
            &[][..],
            &["-i"],
            &["-o", "-t"],
            &["-selection", "clipboard", "-o"],
        ] {
            assert_eq!(request(&args(refused)).unwrap_err().to_string(), USAGE);
        }
    }
}
