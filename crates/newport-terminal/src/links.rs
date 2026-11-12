//! Plain HTTP(S) links in the rendered viewport. Explicit OSC 8 links retain
//! precedence. Cell coordinates are retained across Unicode and soft wrapping.
use crate::{CellWide, Row};
use std::ops::{Range, RangeInclusive};

#[derive(Clone, Debug)]
pub(crate) struct DetectedLink {
    pub cells: RangeInclusive<usize>,
    pub url: String,
}

struct TextCell {
    bytes: Range<usize>,
    cells: RangeInclusive<usize>,
}

pub(crate) fn detect(rows: &[Row], columns: u16) -> Vec<DetectedLink> {
    let mut links = Vec::new();
    let mut text = String::new();
    let mut map = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        for (x, cell) in row.cells.iter().enumerate() {
            if matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                continue;
            }
            let start = text.len();
            if cell.style.invisible || cell.hyperlink.is_some() || cell.text.is_empty() {
                text.push(' ');
            } else {
                text.push_str(&cell.text);
            }
            let offset = y * usize::from(columns) + x;
            let end = offset + usize::from(cell.wide == CellWide::Wide);
            map.push(TextCell {
                bytes: start..text.len(),
                cells: offset..=end,
            });
        }
        if !row.wrapped || y + 1 == rows.len() {
            scan(&text, &map, row.wrapped, &mut links);
            text.clear();
            map.clear();
        }
    }
    links
}

fn delimiter(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '\\' | '|')
}

fn scan(text: &str, map: &[TextCell], clipped: bool, links: &mut Vec<DetectedLink>) {
    let mut skip_to = 0;
    for (start, _) in text.char_indices() {
        if start < skip_to {
            continue;
        }
        let rest = &text[start..];
        if !rest
            .as_bytes()
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case(b"http://"))
            && !rest
                .as_bytes()
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case(b"https://"))
        {
            continue;
        }
        if text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '/' | ':'))
        {
            continue;
        }
        let length = rest.find(delimiter).unwrap_or(rest.len());
        skip_to = start + length;
        if length > 16 * 1024 {
            continue;
        }
        // Do not open a truncated URL that continues below the viewport.
        if clipped && skip_to == text.len() {
            continue;
        }
        let candidate = trim_punctuation(&rest[..length]);
        let Some(url) = newport_domain::external_link::http_url(candidate) else {
            continue;
        };
        let end = start + candidate.len();
        let first = map.partition_point(|cell| cell.bytes.end <= start);
        let last = map.partition_point(|cell| cell.bytes.start < end);
        if let (Some(first), Some(last)) = (
            map.get(first),
            last.checked_sub(1).and_then(|last| map.get(last)),
        ) {
            links.push(DetectedLink {
                cells: *first.cells.start()..=*last.cells.end(),
                url,
            });
        }
    }
}

fn trim_punctuation(mut value: &str) -> &str {
    let mut balance = [0_i32; 3];
    for c in value.chars() {
        match c {
            '(' => balance[0] += 1,
            ')' => balance[0] -= 1,
            '[' => balance[1] += 1,
            ']' => balance[1] -= 1,
            '{' => balance[2] += 1,
            '}' => balance[2] -= 1,
            _ => {}
        }
    }
    loop {
        let trimmed = value.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if trimmed.len() != value.len() {
            value = trimmed;
            continue;
        }
        let Some(close) = value.chars().next_back() else {
            return value;
        };
        let index = match close {
            ')' => 0,
            ']' => 1,
            '}' => 2,
            _ => return value,
        };
        if balance[index] >= 0 {
            return value;
        }
        balance[index] += 1;
        value = &value[..value.len() - close.len_utf8()];
    }
}
