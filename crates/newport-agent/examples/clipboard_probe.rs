// Reports format availability without printing clipboard contents.
#[cfg(target_os = "linux")]
fn main() {
    let mut clipboard = arboard::Clipboard::new().expect("clipboard connection");
    println!(
        "file list: {}",
        match clipboard.get().file_list() {
            Ok(files) => format!("{} entries", files.len()),
            Err(e) => e.to_string(),
        }
    );
    match clipboard.get_image() {
        Ok(image) => println!(
            "image: {} x {}, {} decoded bytes",
            image.width,
            image.height,
            image.bytes.len()
        ),
        Err(error) => {
            eprintln!("image: {error}");
            std::process::exit(1);
        }
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This probe runs on Linux.");
}
