//! Bounded, opt-in stdout/stderr display. Stdin and argument values are never logged.
use super::protocol::CommandLog;
use std::{cell::RefCell, ffi::OsString, time::Duration};
thread_local! { static LOGS: RefCell<Option<Vec<CommandLog>>> = const { RefCell::new(None) }; }
pub fn begin(enabled: bool) {
    LOGS.with(|logs| *logs.borrow_mut() = enabled.then(Vec::new));
}
pub fn take() -> Vec<CommandLog> {
    LOGS.with(|logs| logs.borrow_mut().take().unwrap_or_default())
}
// Command labels deliberately omit values: URLs, messages and configuration may
// contain secrets. Diagnostics are best-effort redacted and never persisted.
fn label(args: &[OsString]) -> String {
    let verbs = [
        "symbolic-ref",
        "merge-base",
        "check-ref-format",
        "status",
        "rev-parse",
        "rev-list",
        "cat-file",
        "diff",
        "for-each-ref",
        "config",
        "ls-files",
        "ls-remote",
        "worktree",
        "fetch",
        "merge",
        "push",
        "commit",
        "add",
        "reset",
        "restore",
        "checkout",
        "checkout-index",
        "update-index",
        "update-ref",
        "branch",
        "tag",
        "stash",
        "rebase",
        "cherry-pick",
        "revert",
        "clone",
        "init",
        "apply",
        "log",
        "show",
        "remote",
        "rm",
    ];
    format!(
        "git {}",
        args.iter()
            .filter_map(|a| a.to_str())
            .find(|a| verbs.contains(a))
            .unwrap_or("[command]")
    )
}
fn redact(bytes: &[u8], already_truncated: bool) -> String {
    // Bound work before decoding: a diff/blob can contain tens of megabytes.
    const LIMIT: usize = 4096;
    let prefix = &bytes[..bytes.len().min(LIMIT)];
    let mut output = String::new();
    for line in String::from_utf8_lossy(prefix).split_inclusive(['\n', '\0']) {
        let lower = line.to_ascii_lowercase();
        if [
            "password",
            "token",
            "authorization",
            "secret",
            "credential",
            "private key",
        ]
        .iter()
        .any(|word| lower.contains(word))
        {
            output.push_str("[sensitive output omitted]");
            if line.ends_with(['\n', '\0']) {
                output.push('\n');
            }
            continue;
        }
        // Preserve spacing, indentation and line breaks. URLs are replaced
        // whole, including credentials and query strings, before transmission.
        for token in line.split_inclusive(char::is_whitespace) {
            if token.contains("://") {
                output.push_str("[URL omitted]");
                output.push_str(&token[token.trim_end_matches(char::is_whitespace).len()..]);
            } else {
                for c in token.chars() {
                    match c {
                        '\n' | '\t' | '\r' | '\u{8}' | '\u{1b}' => output.push(c),
                        '\0' => output.push('\n'),
                        c if c.is_control() => output.extend(c.escape_default()),
                        c => output.push(c),
                    }
                }
            }
        }
    }
    let display_truncated = output.len() > 12000;
    if display_truncated {
        let mut end = 12000;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
    }
    if bytes.len() > LIMIT || already_truncated || display_truncated {
        output.push_str("\n[Output truncated: showing at most 4 KiB of this stream]\n");
    }
    output
}

pub fn record(
    args: &[OsString],
    elapsed: Duration,
    exit_code: Option<i32>,
    stdout: (&[u8], bool),
    stderr: (&[u8], bool),
    interrupted: bool,
) {
    LOGS.with(|logs| {
        let mut logs = logs.borrow_mut();
        if let Some(logs) = logs.as_mut() {
            if logs.len() < 32 {
                logs.push(CommandLog {
                    command: label(args),
                    duration_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
                    exit_code,
                    output: [
                        (!stdout.0.is_empty() || stdout.1).then(|| redact(stdout.0, stdout.1)),
                        (!stderr.0.is_empty() || stderr.1).then(|| redact(stderr.0, stderr.1)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n"),
                    interrupted,
                });
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_are_opt_in_bounded_and_redacted() {
        begin(false);
        record(
            &[],
            Duration::ZERO,
            Some(0),
            (b"ignored", false),
            (b"", false),
            false,
        );
        assert!(take().is_empty());
        begin(true);
        for _ in 0..40 {
            record(
                &["push".into(), "https://user:pass@host/repo".into()],
                Duration::ZERO,
                Some(1),
                (b"  raw\tstdout\n", false),
                (
                    b"fatal: https://user:pass@host/repo\nAuthorization: Bearer abc\nnormal output",
                    false,
                ),
                false,
            );
        }
        let logs = take();
        assert_eq!(logs.len(), 32);
        assert_eq!(logs[0].command, "git push");
        assert!(!logs[0].output.contains("pass"));
        assert!(!logs[0].output.contains("abc"));
        assert!(logs[0].output.contains("normal output"));
    }
    #[test]
    fn raw_output_preserves_whitespace_escapes_controls_and_marks_truncation() {
        assert_eq!(
            redact(b"  one\ttwo\npath\0progress\r", false),
            "  one\ttwo\npath\nprogress\r"
        );
        assert!(redact(&vec![b'x'; 5000], false).contains("Output truncated"));
        assert!(redact(&vec![1; 4096], false).len() < 12500);
        assert!(redact(b"normal\0secret=value\0another\0", false).contains("another\n"));
        assert!(redact(b"partial", true).contains("Output truncated"));
        assert!(!redact(b"url https://user:pass@host/repo?auth=abc\n", false).contains("abc"));
    }
}
