use std::{fs, os::unix::fs::symlink, path::Path};

/// Keep the executable immutable: concurrent process launches can inherit a
/// writer for a freshly generated script and make Linux exec fail with ETXTBSY.
pub fn script(path: &Path, body: &str) {
    let mut source = path.as_os_str().to_os_string();
    source.push(".sh");
    fs::write(source, body).unwrap();
    if !path.exists() {
        symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run-script.sh"),
            path,
        )
        .unwrap();
    }
}
