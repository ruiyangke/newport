use std::path::Path;

fn quoted(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("Windows resource strings cannot contain control characters".into());
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub fn resource_script(product: &str, version: &str, icon: &Path) -> Result<String, String> {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let numbers = core
        .split('.')
        .map(str::parse::<u16>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Windows version components must fit in 16 bits")?;
    if numbers.len() != 3 {
        return Err("Windows application versions require major.minor.patch".into());
    }
    let numeric = format!("{},{},{},0", numbers[0], numbers[1], numbers[2]);
    let flags = if version.split('+').next().unwrap_or_default().contains('-') {
        2
    } else {
        0
    };
    let product = quoted(product)?;
    let version = quoted(version)?;
    let icon = quoted(icon.to_str().ok_or("Icon path must be UTF-8")?)?;
    // GPUI supplies the DPI/common-controls manifest. Do not embed a second
    // manifest here. Icon group 1 is the resource GPUI loads for its windows.
    Ok(format!(
        r#"#pragma code_page(65001)
LANGUAGE 9, 1
1 ICON {icon}
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3f
FILEFLAGS {flags}
FILEOS 0x40004
FILETYPE 1
FILESUBTYPE 0
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "newport"
            VALUE "FileDescription", {product}
            VALUE "FileVersion", {version}
            VALUE "InternalName", "newport"
            VALUE "OriginalFilename", "newport.exe"
            VALUE "ProductName", {product}
            VALUE "ProductVersion", {version}
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#
    ))
}
