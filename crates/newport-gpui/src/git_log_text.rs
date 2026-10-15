//! Render untrusted command output with terminal semantics, without terminal side effects.
use newport_terminal::{CellWide, Engine, StyleColor, Underline};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogColor {
    Palette(usize),
    Rgb([u8; 3]),
}
impl LogColor {
    fn from_style(color: StyleColor) -> Option<Self> {
        match color {
            StyleColor::None => None,
            StyleColor::Rgb(c) => Some(Self::Rgb([c.r, c.g, c.b])),
            StyleColor::Palette(i) => Some(Self::Palette(usize::from(i.0))),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub foreground: LogColor,
    pub background: LogColor,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub inverse: bool,
    pub underline: bool,
    pub underline_color: Option<LogColor>,
    pub undercurl: bool,
    pub strike: bool,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            foreground: LogColor::Palette(256),
            background: LogColor::Palette(257),
            bold: false,
            italic: false,
            dim: false,
            inverse: false,
            underline: false,
            underline_color: None,
            undercurl: false,
            strike: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub text: String,
    pub spans: Vec<Span>,
    /// This display row continues on the next row, without a source newline.
    pub wraps: bool,
}
impl From<&str> for LogLine {
    fn from(text: &str) -> Self {
        Self {
            text: text.into(),
            spans: vec![],
            wraps: false,
        }
    }
}

/// Match Tauri's log allowlist: styling and horizontal progress controls only.
fn sanitize(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    let mut sequence = String::new();
                    for c in chars.by_ref() {
                        sequence.push(c);
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                    if sequence.chars().last().is_some_and(|c| "mKGCD".contains(c))
                        && sequence[..sequence.len() - 1]
                            .chars()
                            .all(|c| c.is_ascii_digit() || ";:".contains(c))
                    {
                        out.push_str("\x1b[");
                        out.push_str(&sequence);
                    }
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => (),
            }
        } else if c == '\0' || c == '\n' {
            out.push_str("\r\n");
        } else if matches!(c, '\x08' | '\t' | '\r')
            || (c >= ' ' && !('\x7f'..='\u{9f}').contains(&c))
        {
            out.push(c);
        }
    }
    out
}
pub fn render(text: &str) -> Result<Vec<String>, String> {
    Ok(render_styled(text)?
        .into_iter()
        .map(|line| line.text)
        .collect())
}
pub fn render_styled(text: &str) -> Result<Vec<LogLine>, String> {
    render_styled_at(text, 160)
}
pub fn render_styled_at(text: &str, columns: u16) -> Result<Vec<LogLine>, String> {
    let mut engine = Engine::new(columns, 24).map_err(|error| error.to_string())?;
    // Effects are intentionally discarded: untrusted log output cannot send
    // replies, change window titles, ring the bell, or access the clipboard.
    engine
        .advance(sanitize(text).as_bytes())
        .map_err(|error| error.to_string())?;
    let rows = engine.history_rows().map_err(|error| error.to_string())?;
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let mut line = LogLine::default();
        let mut content_end = 0;
        for cell in row.cells {
            if matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                continue;
            }
            let start = line.text.len();
            if cell.style.invisible {
                line.text.push(' ');
                if cell.wide == CellWide::Wide {
                    line.text.push(' ');
                }
            } else if cell.text.is_empty() {
                line.text.push(' ');
            } else {
                line.text.push_str(&cell.text);
            }
            let style = Style {
                foreground: LogColor::from_style(cell.style.fg_color)
                    .unwrap_or(LogColor::Palette(256)),
                background: LogColor::from_style(cell.style.bg_color)
                    .unwrap_or(LogColor::Palette(257)),
                bold: cell.style.bold,
                italic: cell.style.italic,
                dim: cell.style.faint,
                inverse: cell.style.inverse,
                underline: cell.style.underline != Underline::None,
                underline_color: LogColor::from_style(cell.style.underline_color),
                undercurl: cell.style.underline == Underline::Curly,
                strike: cell.style.strikethrough,
            };
            // Colored/decorated blanks are visible terminal content, including
            // an otherwise empty line. Only trim unpainted trailing whitespace.
            if line.text[start..].chars().any(|c| !c.is_whitespace())
                || style.background != LogColor::Palette(257)
                || style.inverse
                || style.underline
                || style.strike
            {
                content_end = line.text.len();
            }
            if style != Style::default() {
                if let Some(last) = line.spans.last_mut()
                    && last.range.end == start
                    && last.style == style
                {
                    last.range.end = line.text.len();
                } else {
                    line.spans.push(Span {
                        range: start..line.text.len(),
                        style,
                    });
                }
            }
        }
        line.wraps = row.wrapped;
        if !line.wraps {
            line.text.truncate(content_end);
        }
        for span in &mut line.spans {
            span.range.end = span.range.end.min(line.text.len());
        }
        line.spans.retain(|span| span.range.start < span.range.end);
        lines.push(line);
    }
    while lines.last().is_some_and(|line| line.text.is_empty()) {
        lines.pop();
    }
    Ok(lines)
}
#[cfg(test)]
mod tests {
    use super::{LogColor, plain_text, render, render_styled, render_styled_at};
    #[test]
    fn styles_keep_utf8_ranges_and_resets_without_escaping_into_copied_text() {
        let text = "Café 雪 🦀";
        let lines = render_styled(&format!("\x1b[1;3;4;9;31;44m{text}\x1b[0m plain")).unwrap();
        assert_eq!(lines[0].text, format!("{text} plain"));
        assert_eq!(lines[0].spans.len(), 1);
        let span = &lines[0].spans[0];
        assert_eq!(span.range, 0..text.len());
        assert!(span.style.bold && span.style.italic && span.style.underline && span.style.strike);
        assert_eq!(span.style.foreground, LogColor::Palette(1));
        assert_eq!(span.style.background, LogColor::Palette(4));
        let encoded = serde_json::to_vec(&lines).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<super::LogLine>>(&encoded).unwrap(),
            lines
        );
    }
    #[test]
    fn soft_wraps_preserve_unicode_spaces_styles_and_logical_newlines() {
        let source = "ab  cd  EF雪🦀gh\n\nlast";
        for width in [8, 9, 12, 80] {
            let lines = render_styled_at(&format!("\x1b[31m{source}\x1b[0m"), width).unwrap();
            assert_eq!(plain_text(&lines), source, "width {width}");
            assert_eq!(
                lines.iter().filter(|line| line.wraps).count() > 0,
                width < 80
            );
            for line in &lines {
                for span in &line.spans {
                    assert!(line.text.get(span.range.clone()).is_some());
                }
            }
        }
        // A wide glyph at the right edge leaves a spacer, not a copied blank.
        assert_eq!(
            plain_text(&render_styled_at("1234567雪!", 8).unwrap()),
            "1234567雪!"
        );
        assert!(render_styled_at("text", 1).is_err());
    }
    #[test]
    fn underline_color_and_curl_survive_terminal_processing() {
        let lines = render_styled("\x1b[4:3;58:2::10:20:30mcurled\x1b[0m plain").unwrap();
        let span = &lines[0].spans[0];
        assert_eq!(span.range, 0..6);
        assert!(span.style.underline && span.style.undercurl);
        assert_eq!(
            span.style.underline_color,
            Some(LogColor::Rgb([10, 20, 30]))
        );
    }
    #[test]
    fn concealed_wide_characters_keep_their_cells_without_copying_hidden_text() {
        assert_eq!(render("a\x1b[8m雪🦀\x1b[0mz").unwrap(), ["a    z"]);
        assert_eq!(render("\x1b[8m雪🦀\x1b[0m").unwrap(), Vec::<String>::new());
    }
    #[test]
    fn painted_and_decorated_trailing_spaces_survive_plain_padding_is_trimmed() {
        for code in ["41", "7", "4", "9"] {
            let lines = render_styled(&format!("x\x1b[{code}m   \x1b[0m   ")).unwrap();
            assert_eq!(lines[0].text, "x   ");
            assert_eq!(lines[0].spans[0].range, 1..4);
            let blank = render_styled(&format!("\x1b[{code}m   \x1b[0m")).unwrap();
            assert_eq!(blank[0].text, "   ");
        }
        assert_eq!(render("x\x1b[31m   \x1b[0m").unwrap(), ["x"]);
    }
    #[test]
    fn progress_replaces_styles_and_extended_colors_survive() {
        let lines =
            render_styled("\x1b[31mold\r\x1b[2K\x1b[0;38;2;10;20;30;48;5;99;2;7mnew\x1b[0m\nplain")
                .unwrap();
        assert_eq!(lines[0].text, "new");
        assert_eq!(lines[0].spans.len(), 1);
        let style = &lines[0].spans[0].style;
        assert_eq!(style.foreground, LogColor::Rgb([10, 20, 30]));
        assert_eq!(style.background, LogColor::Palette(99));
        assert!(style.dim && style.inverse);
        assert_eq!(lines[1].text, "plain");
        assert!(lines[1].spans.is_empty());
    }
    #[test]
    fn progress_overwrites_and_nul_delimited_paths_become_lines() {
        assert_eq!(
            render("count 1\rcount 25\r\x1b[2Kdone\nfile-a\0file-b\0").unwrap(),
            ["done", "file-a", "file-b"]
        );
    }
    #[test]
    fn styled_scrollback_retains_output_before_the_last_screen() {
        let source = (0..64)
            .map(|index| format!("row {index}: 雪🦀 e\u{301}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_styled_at(&format!("\x1b[31m{source}\x1b[0m"), 12).unwrap();
        assert!(lines.len() > 64);
        assert_eq!(plain_text(&lines), source);
        assert_eq!(lines[0].spans[0].style.foreground, LogColor::Palette(1));
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style.foreground == LogColor::Palette(1))
        );
    }
    #[test]
    fn cursor_screen_clipboard_and_hyperlink_controls_cannot_escape_log() {
        assert_eq!(render("keep\n\x1b[2J\x1b[H\x1b]52;c;secret\x07\x1b]8;;https://invalid\x1b\\visible\x1b]8;;\x1b\\").unwrap(), ["keep","visible"]);
        assert_eq!(
            render("\x1b[8msecret\x1b[0m visible").unwrap(),
            ["       visible"]
        );
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogRecord {
    pub source: String,
    pub lines: Vec<LogLine>,
}
/// Copy terminal text without inserting newlines at soft wraps.
pub fn plain_text(lines: &[LogLine]) -> String {
    let mut output = String::new();
    let mut wrapped = true;
    for line in lines {
        if !wrapped {
            output.push('\n');
        }
        output.push_str(&line.text);
        wrapped = line.wraps;
    }
    output
}
/// Called on a worker; filter before terminal parsing to avoid processing another repo's output.
pub fn batch(
    value: serde_json::Value,
    server: &str,
    repo: &str,
) -> Result<serde_json::Value, String> {
    batch_at(value, server, repo, 160)
}
pub fn batch_at(
    value: serde_json::Value,
    server: &str,
    repo: &str,
    columns: u16,
) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let cursor = value["cursor"].as_u64().ok_or("Invalid Git log cursor")?;
    let events = value["entries"]
        .as_array()
        .ok_or("Invalid Git log events")?;
    if events.len() > 100 {
        return Err("Git log batch exceeds its limit".into());
    }
    let mut records = Vec::new();
    let mut omitted = false;
    for event in events {
        if event["serverId"] != server || event["repoId"] != repo {
            continue;
        }
        let entry = &event["entry"];
        let Some(command) = entry["command"].as_str() else {
            omitted = true;
            continue;
        };
        let output = entry["output"].as_str().unwrap_or("");
        if command.len() > 1024 || output.len() > 32768 {
            omitted = true;
            continue;
        }
        let status = if entry["interrupted"] == true {
            "Interrupted".into()
        } else if entry["exitCode"] == 0 {
            "Done".into()
        } else {
            format!("Exit {}", entry["exitCode"])
        };
        let source = format!(
            "$ {command}\n{status} · {} ms\n{output}\x1b[0m",
            entry["durationMs"]
        );
        let lines = render_styled_at(&source, columns)?;
        records.push(LogRecord { source, lines });
    }
    Ok(
        json!({"cursor":cursor,"eventCount":events.len(),"records":records,"omitted":omitted,"columns":columns}),
    )
}

#[cfg(test)]
mod batch_tests {
    use super::batch;
    use serde_json::json;
    #[test]
    fn filters_exact_scope_before_parsing_and_preserves_global_cursor() {
        let event = |server, repo, output| json!({"serverId":server,"repoId":repo,"entry":{"command":"git status","durationMs":3,"exitCode":0,"output":output}});
        let result = batch(json!({"cursor":42,"entries":[event("a","one","first\rfinal"),event("b","one","foreign server"),event("a","two","foreign repo")]}),"a","one").unwrap();
        assert_eq!(result["cursor"], 42);
        assert_eq!(result["eventCount"], 3);
        let records: Vec<super::LogRecord> =
            serde_json::from_value(result["records"].clone()).unwrap();
        assert_eq!(
            records[0]
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["$ git status", "Done · 3 ms", "final"]
        );
    }
    #[test]
    fn rejects_malformed_batches_and_reports_oversized_output() {
        assert!(batch(json!({"entries":[]}), "a", "one").is_err());
        assert!(
            batch(
                json!({"cursor":0,"entries":vec![json!({});101]}),
                "a",
                "one"
            )
            .is_err()
        );
        let result=batch(json!({"cursor":1,"entries":[{"serverId":"a","repoId":"one","entry":{"command":"git log","output":"x".repeat(32769)}}]}),"a","one").unwrap();
        assert_eq!(result["omitted"], true);
        assert_eq!(result["records"], json!([]));
    }
}
