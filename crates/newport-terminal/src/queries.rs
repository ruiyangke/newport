//! Replies to the queries Alacritty's parser does not dispatch: terminfo
//! capabilities (XTGETTCAP), selected settings (DECRQSS) and the terminal
//! version (XTVERSION). A second parser watches the same bytes and stops after
//! each query, so the engine answers it in order with the rest of the output.
use crate::engine::Listener;
use alacritty_terminal::{
    Term,
    term::cell::Flags,
    vte::{
        Params, Parser, Perform,
        ansi::{Color, CursorShape, NamedColor},
    },
};

/// Upper bound for a query's payload; longer requests are ignored.
const MAX_REQUEST: usize = 1024;

/// Capabilities answered for XTGETTCAP, in terminfo string syntax.
const CAPABILITIES: &[(&str, Option<&str>)] = &[
    ("TN", Some("xterm-256color")),
    ("Co", Some("256")),
    ("colors", Some("256")),
    ("RGB", Some("8")),
    ("Tc", None),
    ("Ms", Some("\\E]52;%p1%s;%p2%s\\007")),
    ("Su", None),
    ("Smulx", Some("\\E[4:%p1%dm")),
    (
        "Setulc",
        Some("\\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m"),
    ),
    ("setrgbf", Some("\\E[38:2::%p1%d:%p2%d:%p3%dm")),
    ("setrgbb", Some("\\E[48:2::%p1%d:%p2%d:%p3%dm")),
    ("Ss", Some("\\E[%p1%d q")),
    ("Se", Some("\\E[2 q")),
    ("Sync", Some("\\E[?2026%?%p1%{1}%-%tl%eh%;")),
];

#[derive(Default)]
pub(crate) struct Queries {
    parser: Parser,
    watcher: Watcher,
}
impl Queries {
    /// Bytes up to and including the next complete query, and that query.
    /// Text between sequences is skipped: every query starts with ESC.
    pub fn next(&mut self, bytes: &[u8]) -> (usize, Option<Query>) {
        let mut offset = 0;
        while offset < bytes.len() {
            if self.watcher.between_sequences {
                match bytes[offset..].iter().position(|&byte| byte == 0x1b) {
                    Some(escape) => offset += escape,
                    None => return (bytes.len(), None),
                }
                self.watcher.between_sequences = false;
            }
            offset += self
                .parser
                .advance_until_terminated(&mut self.watcher, &bytes[offset..]);
            if let Some(query) = self.watcher.found.take() {
                return (offset, Some(query));
            }
        }
        (offset, None)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Query {
    Capabilities(Vec<u8>),
    Setting(Vec<u8>),
    Version,
}
impl Query {
    pub fn reply(&self, term: &Term<Listener>) -> String {
        match self {
            Self::Version => format!("\x1bP>|Newport {}\x1b\\", env!("CARGO_PKG_VERSION")),
            Self::Capabilities(request) => request
                .split(|byte| *byte == b';')
                .map(capability)
                .collect(),
            Self::Setting(request) => match setting(term, request) {
                Some(value) => format!("\x1bP1$r{value}\x1b\\"),
                None => "\x1bP0$r\x1b\\".into(),
            },
        }
    }
}

struct Watcher {
    pending: Option<Query>,
    found: Option<Query>,
    /// The parser finished a sequence and only text has followed.
    between_sequences: bool,
}
impl Default for Watcher {
    fn default() -> Self {
        Self {
            pending: None,
            found: None,
            between_sequences: true,
        }
    }
}
impl Perform for Watcher {
    fn print(&mut self, _: char) {
        self.between_sequences = true;
    }
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {
        self.between_sequences = true;
    }
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {
        self.between_sequences = true;
    }
    fn hook(&mut self, _: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.pending = match (intermediates, action) {
            _ if ignore => None,
            (b"+", 'q') => Some(Query::Capabilities(Vec::new())),
            (b"$", 'q') => Some(Query::Setting(Vec::new())),
            _ => None,
        };
    }
    fn put(&mut self, byte: u8) {
        if let Some(Query::Capabilities(request) | Query::Setting(request)) = &mut self.pending {
            if request.len() < MAX_REQUEST {
                request.push(byte);
            } else {
                self.pending = None;
            }
        }
    }
    fn unhook(&mut self) {
        self.found = self.pending.take();
        self.between_sequences = self.found.is_none();
    }
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.between_sequences = true;
        let version = !ignore
            && intermediates == b">"
            && action == 'q'
            && params.iter().all(|param| param == [0]);
        if version {
            self.found = Some(Query::Version);
        }
    }
    fn terminated(&self) -> bool {
        self.found.is_some() || self.between_sequences
    }
}

fn capability(hex_name: &[u8]) -> String {
    let known = decode_hex(hex_name).and_then(|name| {
        CAPABILITIES
            .iter()
            .find(|(capability, _)| capability.as_bytes() == name)
    });
    let name = String::from_utf8_lossy(hex_name).to_ascii_uppercase();
    match known {
        Some((_, Some(value))) => format!("\x1bP1+r{name}={}\x1b\\", encode_hex(value)),
        Some((_, None)) => format!("\x1bP1+r{name}\x1b\\"),
        None => format!("\x1bP0+r{name}\x1b\\"),
    }
}

fn setting(term: &Term<Listener>, request: &[u8]) -> Option<String> {
    match request {
        b"m" => Some(format!("{}m", sgr(term))),
        b" q" => {
            let style = term.cursor_style();
            let shape = match style.shape {
                CursorShape::Block | CursorShape::HollowBlock | CursorShape::Hidden => 1,
                CursorShape::Underline => 3,
                CursorShape::Beam => 5,
            };
            Some(format!("{} q", shape + u8::from(!style.blinking)))
        }
        _ => None,
    }
}

/// The SGR parameters that recreate the cursor's current attributes.
fn sgr(term: &Term<Listener>) -> String {
    let template = &term.grid().cursor.template;
    let mut parameters = vec!["0".to_owned()];
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::DOUBLE_UNDERLINE, "4:2"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if template.flags.contains(flag) {
            parameters.push(code.into());
        }
    }
    parameters.extend(color(template.fg, 30, NamedColor::Foreground));
    parameters.extend(color(template.bg, 40, NamedColor::Background));
    if let Some(underline) = template.underline_color() {
        parameters.extend(color(underline, 50, NamedColor::Foreground));
    }
    parameters.join(";")
}

/// SGR parameter for a color; `base` is 30 (foreground), 40 or 50 (underline).
fn color(color: Color, base: u8, default: NamedColor) -> Option<String> {
    let extended = base + 8;
    match color {
        Color::Named(named) if named == default => None,
        Color::Named(named) => {
            let index = named as usize;
            match index {
                0..=7 if base != 50 => Some((base as usize + index).to_string()),
                8..=15 if base != 50 => Some((base as usize + 60 + index - 8).to_string()),
                0..=15 => Some(format!("{extended}:5:{index}")),
                _ => None,
            }
        }
        Color::Indexed(index) => Some(format!("{extended}:5:{index}")),
        Color::Spec(rgb) => Some(format!("{extended}:2::{}:{}:{}", rgb.r, rgb.g, rgb.b)),
    }
}

fn decode_hex(hex: &[u8]) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

fn encode_hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queries(input: &[u8]) -> Vec<(usize, Query)> {
        let mut queries = Queries::default();
        let mut found = Vec::new();
        let mut offset = 0;
        while offset < input.len() {
            let (read, query) = queries.next(&input[offset..]);
            offset += read;
            if let Some(query) = query {
                found.push((offset, query));
            }
        }
        found
    }

    #[test]
    fn queries_end_where_their_sequence_ends() {
        let input = b"a\x1bP+q5463\x1b\\b\x1b[>qc\x1bP$qm\x1b\\";
        assert_eq!(
            queries(input),
            [
                // A string ends at the ESC that starts its terminator.
                (10, Query::Capabilities(b"5463".to_vec())),
                (16, Query::Version),
                (23, Query::Setting(b"m".to_vec())),
            ]
        );
    }

    #[test]
    fn queries_split_across_chunks_are_found_once() {
        let mut queries = Queries::default();
        let mut found = Vec::new();
        for byte in b"\x1bP+q546372\x1b\\" {
            let (read, query) = queries.next(std::slice::from_ref(byte));
            assert_eq!(read, 1);
            found.extend(query);
        }
        assert_eq!(found, [Query::Capabilities(b"546372".to_vec())]);
    }

    #[test]
    fn other_sequences_and_oversized_requests_are_not_queries() {
        assert!(queries(b"\x1b[q\x1b[1q\x1bPq#0\x1b\\\x1bP$p\x1b\\").is_empty());
        let mut long = b"\x1bP+q".to_vec();
        long.extend(std::iter::repeat_n(b'4', MAX_REQUEST + 2));
        long.extend(b"\x1b\\");
        assert!(queries(&long).is_empty());
    }

    #[test]
    fn capabilities_report_known_names_and_reject_unknown_ones() {
        assert_eq!(capability(b"5463"), "\x1bP1+r5463\x1b\\");
        assert_eq!(capability(b"524742"), "\x1bP1+r524742=38\x1b\\");
        assert_eq!(capability(b"6e6f6e65"), "\x1bP0+r6E6F6E65\x1b\\");
        assert_eq!(capability(b"zz"), "\x1bP0+rZZ\x1b\\");
    }
}
