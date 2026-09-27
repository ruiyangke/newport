//! JSON-lines probe: keep one unmodified arboard Clipboard alive across reads.
use arboard::Clipboard;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{self, BufRead, Write};

fn read(clipboard: &mut Clipboard, command: &str) -> Value {
    match command {
        "read-text" => match clipboard.get_text() {
            Ok(text) => json!({"ok": true, "text": text}),
            Err(error) => json!({"ok": false, "error": error.to_string()}),
        },
        "read-image" => match clipboard.get_image() {
            Ok(image) => json!({
                "ok": true, "width": image.width, "height": image.height,
                "sha256": format!("{:x}", Sha256::digest(image.bytes.as_ref())),
            }),
            Err(error) => json!({"ok": false, "error": error.to_string()}),
        },
        _ => json!({"ok": false, "error": "unknown probe command"}),
    }
}
fn emit(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}
fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "read-sequence".into());
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            emit(json!({"ok": false, "error": error.to_string(), "phase": "initialize"}));
            std::process::exit(1);
        }
    };
    if mode != "read-sequence" {
        emit(read(&mut clipboard, &mode));
        return;
    }
    emit(json!({"ready": true, "pid": std::process::id(), "arboard": "3.6.1"}));
    for command in io::stdin().lock().lines() {
        let command = command.unwrap();
        if command == "quit" {
            break;
        }
        emit(read(&mut clipboard, &command));
    }
}
