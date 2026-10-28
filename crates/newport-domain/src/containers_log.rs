//! Container log severity and local filtering.
use crate::containers::ContainerLogEntry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Severity {
    #[default]
    Normal,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    #[default]
    All,
    Warnings,
    Errors,
}

impl Level {
    pub const fn includes(self, severity: Severity) -> bool {
        match self {
            Self::All => true,
            Self::Warnings => !matches!(severity, Severity::Normal),
            Self::Errors => matches!(severity, Severity::Error),
        }
    }
}

/// Infer severity from message text only. Stderr is not itself an error level.
/// Word boundaries are ASCII: only `[A-Za-z0-9_]` are word characters.
pub fn severity(message: &str) -> Severity {
    let lower = message.to_ascii_lowercase();
    let mut warning = false;
    for word in
        lower.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
    {
        match word {
            "error" | "err" | "fatal" | "panic" | "exception" | "traceback" | "critical"
            | "crit" | "failed" | "failure" => return Severity::Error,
            "warn" | "warning" => warning = true,
            _ => {}
        }
    }
    // These source patterns intentionally match prefixes even when a longer
    // value would not match a whole severity word (for example level=warning).
    if lower.contains("level=error") || lower.contains("level=fatal") {
        Severity::Error
    } else if warning || lower.contains("level=warn") || lower.contains("\"level\":\"warn") {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// A prepared local filter; normalize the query once when controls change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    level: Level,
    needle: String,
}

impl Filter {
    pub fn new(level: Level, query: &str) -> Self {
        Self {
            level,
            needle: query.trim_matches(query_whitespace).to_lowercase(),
        }
    }

    pub fn has_query(&self) -> bool {
        !self.needle.is_empty()
    }

    /// Search the displayed container source and message separated by one
    /// space. Stream and timestamp are not searchable source text.
    pub fn matches(&self, entry: &ContainerLogEntry) -> bool {
        if self.level != Level::All && !self.level.includes(severity(&entry.message)) {
            return false;
        }
        self.needle.is_empty()
            || format!(
                "{} {}",
                entry.container.as_deref().unwrap_or(""),
                entry.message
            )
            .to_lowercase()
            .contains(&self.needle)
    }
}

// ECMAScript String.trim whitespace. Rust's trim also accepts U+0085, and does
// not trim U+FEFF, so using it directly would change LogViewer search behavior.
fn query_whitespace(character: char) -> bool {
    matches!(character,
        '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containers::LogStream;

    fn entry(source: Option<&str>, stream: LogStream, message: &str) -> ContainerLogEntry {
        ContainerLogEntry {
            at: Some(1_700_000_000_000),
            stream,
            message: message.into(),
            container: source.map(str::to_owned),
        }
    }

    #[test]
    fn severity_matches_words_case_boundaries_and_error_precedence() {
        for word in [
            "error",
            "err",
            "fatal",
            "panic",
            "exception",
            "traceback",
            "critical",
            "crit",
            "failed",
            "failure",
        ] {
            assert_eq!(
                severity(&format!("[{}] request stopped", word.to_uppercase())),
                Severity::Error
            );
            assert_eq!(severity(&format!("[{word}]")), Severity::Error);
            assert_eq!(severity(&format!("{word}s")), Severity::Normal, "{word}");
            assert_eq!(severity(&format!("no_{word}")), Severity::Normal, "{word}");
        }
        assert_eq!(severity("WARN: retry"), Severity::Warning);
        assert_eq!(severity("warning before CRITICAL exit"), Severity::Error);
        assert_eq!(
            severity("errors warnings error_count failed1 prewarn"),
            Severity::Normal
        );
        assert_eq!(severity("非error常"), Severity::Error);
        assert_eq!(severity("éwarningé"), Severity::Warning);
        assert_eq!(severity("faİled"), Severity::Normal);
    }

    #[test]
    fn structured_patterns_preserve_the_source_prefix_matching() {
        for message in [
            "level=erroring",
            "xlevel=fatality",
            "{\"level\":\"ERROR\"}",
            "[WARN] level=errorish",
        ] {
            assert_eq!(severity(message), Severity::Error, "{message}");
        }
        for message in [
            "level=warner",
            "xLEVEL=WARNINGish",
            "{\"level\":\"warningish\"}",
        ] {
            assert_eq!(severity(message), Severity::Warning, "{message}");
        }
        for message in [
            "{\"level\":\"errorish\"}",
            "{\"level\":\"fatality\"}",
            "{\"level\": \"warningish\"}",
            "level = warningish",
        ] {
            assert_eq!(severity(message), Severity::Normal, "{message}");
        }
    }

    #[test]
    fn levels_include_errors_in_warnings_without_treating_stderr_as_a_level() {
        for stream in [LogStream::Stdout, LogStream::Stderr] {
            for (message, warning, error) in [
                ("server ready", false, false),
                ("warning: retry", true, false),
                ("fatal: stopped", true, true),
            ] {
                let line = entry(Some("error-source"), stream, message);
                assert!(Filter::new(Level::All, "").matches(&line));
                assert_eq!(Filter::new(Level::Warnings, "").matches(&line), warning);
                assert_eq!(Filter::new(Level::Errors, "").matches(&line), error);
            }
        }
    }

    #[test]
    fn search_covers_source_and_message_but_never_stream_timestamp_or_rendered_colon() {
        let line = entry(
            Some("API-WEB"),
            LogStream::Stderr,
            "Warning: request timed out",
        );
        for query in ["api-web", "REQUEST", "  WEB WARNING  ", "warning: request"] {
            assert!(
                Filter::new(Level::Warnings, query).matches(&line),
                "{query}"
            );
        }
        for query in [
            "stderr",
            "stdout",
            "1700000000000",
            "api-web: warning",
            "request  timed",
        ] {
            assert!(!Filter::new(Level::All, query).matches(&line), "{query}");
        }
        assert!(!Filter::new(Level::Errors, "request").matches(&line));
        let no_source = entry(None, LogStream::Stdout, "ready");
        assert!(Filter::default().matches(&no_source));
        assert!(!Filter::new(Level::All, "stdout").matches(&no_source));
    }

    #[test]
    fn search_preserves_unicode_lowercasing_and_ecmascript_trim_semantics() {
        let line = entry(
            Some("CAFÉ-API"),
            LogStream::Stdout,
            "İSTANBUL ΟΣ 東京 ready\nnext",
        );
        for query in [
            "café",
            "i\u{307}stanbul",
            "ος",
            "東京",
            "READY\nNEXT",
            "\u{feff}\u{a0} café \u{3000}",
        ] {
            assert!(Filter::new(Level::All, query).matches(&line), "{query:?}");
        }
        for query in ["cafe", "istanbul", "ready next", "\u{85}ready\u{85}"] {
            assert!(!Filter::new(Level::All, query).matches(&line), "{query:?}");
        }
        assert!(!Filter::new(Level::All, "\u{feff}\u{2028}\t ").has_query());
        assert!(Filter::new(Level::All, "\u{85}").has_query());
    }
}
