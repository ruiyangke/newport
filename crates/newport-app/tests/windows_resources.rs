#[path = "../build_support/windows.rs"]
mod windows;
use std::path::Path;

#[test]
fn windows_version_fields_reject_overflow_and_resource_injection() {
    for version in ["1.2", "1.2.3.4", "65536.0.0", "1.-2.3", "1.2.3\nEND"] {
        assert!(windows::resource_script("Newport", version, Path::new("icon.ico")).is_err());
    }
    assert!(windows::resource_script("Newport\nEND", "1.2.3", Path::new("icon.ico")).is_err());
    assert!(windows::resource_script("Newport", "1.2.3", Path::new("bad\nicon.ico")).is_err());
    let script = windows::resource_script(
        "Newport",
        "1.2.3-beta.1+build.2",
        Path::new(r#"C:\project space\icon.ico"#),
    )
    .unwrap();
    assert!(script.contains("FILEVERSION 1,2,3,0"));
    assert!(script.contains("FILEFLAGS 2"));
    assert!(script.contains("\"1.2.3-beta.1+build.2\""));
    assert!(script.contains(r#""C:\\project space\\icon.ico""#));
}
