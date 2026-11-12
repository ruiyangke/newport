//! Wide strings and command lines as `CreateProcessW` takes them.
#[cfg(windows)]
use std::{ffi::OsStr, io, os::windows::ffi::OsStrExt};

const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// One argument as `CommandLineToArgvW` and the MSVC runtime read it back:
/// always in quotes, with the backslashes before a quote or the closing quote
/// doubled.
pub fn quote(argument: &[u16]) -> Vec<u16> {
    let mut result = vec![QUOTE];
    let mut slashes = 0;
    for &unit in argument {
        if unit == BACKSLASH {
            slashes += 1;
            continue;
        }
        let escaped = if unit == QUOTE {
            slashes * 2 + 1
        } else {
            slashes
        };
        result.extend(std::iter::repeat_n(BACKSLASH, escaped));
        slashes = 0;
        result.push(unit);
    }
    result.extend(std::iter::repeat_n(BACKSLASH, slashes * 2));
    result.push(QUOTE);
    result
}

#[cfg(windows)]
fn units(value: &OsStr) -> io::Result<Vec<u16>> {
    let units: Vec<u16> = value.encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows strings cannot contain NUL",
        ));
    }
    Ok(units)
}

/// `value` NUL-terminated. A NUL inside it is refused.
#[cfg(windows)]
pub fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut units = units(value)?;
    units.push(0);
    Ok(units)
}

/// The NUL-terminated command line that runs `program` with `arguments`,
/// each quoted.
#[cfg(windows)]
pub fn command_line<'a>(
    program: &OsStr,
    arguments: impl IntoIterator<Item = &'a OsStr>,
) -> io::Result<Vec<u16>> {
    let mut line = quote(&units(program)?);
    for argument in arguments {
        line.push(b' ' as u16);
        line.extend(quote(&units(argument)?));
    }
    line.push(0);
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::quote;

    fn quoted(argument: &str) -> String {
        String::from_utf16(&quote(&argument.encode_utf16().collect::<Vec<_>>())).unwrap()
    }

    #[test]
    fn arguments_read_back_unchanged() {
        assert_eq!(quoted(""), r#""""#);
        assert_eq!(quoted("plain"), r#""plain""#);
        assert_eq!(quoted("with space"), r#""with space""#);
        assert_eq!(quoted(r"C:\Program Files\"), r#""C:\Program Files\\""#);
        assert_eq!(quoted(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quoted(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quoted(r"a\\b"), r#""a\\b""#);
    }
}
