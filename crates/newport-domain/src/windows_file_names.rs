//! Windows rules for a file name made from text a person or a server chose:
//! no characters Windows refuses, no device names, no trailing dot or space,
//! and a full path short enough for the shell, which still keeps to
//! `MAX_PATH`.

/// The longest path the shell takes, in UTF-16 units, its terminating NUL
/// included.
pub const MAX_PATH: usize = 260;
/// The longest name of one file, in UTF-16 units.
pub const MAX_COMPONENT: usize = 255;
const INVALID: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Names Windows keeps for devices, with or without an extension.
const RESERVED: [&str; 30] = [
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Whether `name` is a device name to Windows: its part before the first
/// dot, trailing spaces aside, is one, in any case.
pub fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    RESERVED
        .iter()
        .chain(&["CONIN$", "CONOUT$"])
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
}

/// `text` as a file name, without an extension: characters Windows refuses
/// become spaces, runs of spaces become one, and leading spaces and trailing
/// dots and spaces go. A device name gets `_` after its stem. Empty text
/// gives `fallback`.
pub fn file_name(text: &str, fallback: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if c.is_control() || INVALID.contains(&c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let joined = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = trim_end(&joined);
    if name.is_empty() {
        return fallback.to_owned();
    }
    if is_reserved(name) {
        return match name.split_once('.') {
            Some((stem, rest)) => format!("{}_.{rest}", stem.trim_end_matches(' ')),
            None => format!("{name}_"),
        };
    }
    name.to_owned()
}

fn trim_end(name: &str) -> &str {
    name.trim_end_matches(['.', ' '])
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `name` shortened so `<directory>\<name><extension>` fits [`MAX_PATH`] and
/// the file name fits [`MAX_COMPONENT`]; `directory_length` is the
/// directory's length in UTF-16 units. `None` when not even one character
/// fits. Shortening never splits a character and trims what it leaves at the
/// end as [`file_name`] does.
pub fn fit(name: &str, extension: &str, directory_length: usize) -> Option<String> {
    let budget = (MAX_PATH - 1)
        .checked_sub(directory_length + 1 + utf16_len(extension))?
        .min(MAX_COMPONENT.checked_sub(utf16_len(extension))?);
    let mut used = 0;
    let mut end = 0;
    for (index, c) in name.char_indices() {
        if used + c.len_utf16() > budget {
            break;
        }
        used += c.len_utf16();
        end = index + c.len_utf8();
    }
    let fitted = trim_end(&name[..end]);
    (!fitted.is_empty()).then(|| fitted.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_windows_refuses_become_spaces() {
        assert_eq!(file_name("Calculator", "App"), "Calculator");
        assert_eq!(file_name("Notes: work/home", "App"), "Notes work home");
        assert_eq!(file_name(r#"a<b>c"d\e|f?g*h"#, "App"), "a b c d e f g h");
        assert_eq!(file_name("tab\there\nnow", "App"), "tab here now");
        assert_eq!(file_name("  padded  ", "App"), "padded");
        assert_eq!(file_name("Émile’s café", "App"), "Émile’s café");
    }

    #[test]
    fn a_name_never_ends_in_a_dot_or_space() {
        assert_eq!(file_name("Version 2.", "App"), "Version 2");
        assert_eq!(file_name("dots... . ", "App"), "dots");
        assert_eq!(file_name("...", "App"), "App");
        assert_eq!(file_name("", "App"), "App");
        assert_eq!(file_name("???", "App"), "App");
    }

    #[test]
    fn device_names_are_changed() {
        for name in [
            "CON", "con", "Aux", "NUL", "COM1", "lpt9", "COM²", "CONIN$", "conout$",
        ] {
            assert!(is_reserved(name), "{name}");
            let safe = file_name(name, "App");
            assert!(!is_reserved(&safe), "{safe}");
            assert!(!is_reserved(&format!("{safe}.lnk")), "{safe}");
        }
        assert_eq!(file_name("CON", "App"), "CON_");
        assert_eq!(file_name("aux.notes", "App"), "aux_.notes");
        assert_eq!(file_name("nul .txt", "App"), "nul_.txt");
        assert!(is_reserved("CON.lnk"));
        assert!(is_reserved("con .lnk"));
        for name in ["CONSOLE", "COM10", "LPT", "Calculator", "auxiliary"] {
            assert!(!is_reserved(name), "{name}");
            assert_eq!(file_name(name, "App"), name);
        }
    }

    #[test]
    fn a_path_fits_the_shells_limit() {
        let directory = format!(
            r"C:\Users\{}\Start Menu\Programs\Newport Apps",
            "a".repeat(150)
        );
        let length = utf16_len(&directory);
        let long = "Calculator ".repeat(20);
        let fitted = fit(&long, ".lnk", length).unwrap();
        let path = format!(r"{directory}\{fitted}.lnk");
        assert!(utf16_len(&path) < MAX_PATH, "{}", utf16_len(&path));
        assert!(!fitted.ends_with(' '));
        assert!(long.starts_with(&fitted));
        assert_eq!(fit("Calculator", ".lnk", 40).as_deref(), Some("Calculator"));
        // Nothing fits in a directory at the limit.
        assert_eq!(fit("Calculator", ".lnk", MAX_PATH - 6), None);
        assert_eq!(
            fit("Calculator", ".lnk", MAX_PATH - 7).as_deref(),
            Some("C")
        );
        // One file's name is at most 255 units however short the directory.
        let fitted = fit(&"x".repeat(400), ".lnk", 3).unwrap();
        assert_eq!(utf16_len(&fitted) + 4, MAX_COMPONENT);
    }

    #[test]
    fn shortening_keeps_whole_characters() {
        // Each of these is two UTF-16 units.
        let name = "🧮".repeat(10);
        let fitted = fit(&name, ".lnk", MAX_PATH - 1 - 1 - 4 - 5).unwrap();
        assert_eq!(fitted, "🧮".repeat(2));
        assert_eq!(
            fit("a.  b", ".lnk", MAX_PATH - 1 - 1 - 4 - 4).as_deref(),
            Some("a")
        );
    }
}
