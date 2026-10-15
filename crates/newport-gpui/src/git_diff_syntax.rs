//! Highlight the loaded preview, never fetch a full file for presentation.
//! Parsing and queries run off the UI thread. Results belong to one source and
//! theme generation; a late parse cannot recolor another file or comparison.
use super::*;
use gpui_kit::{
    base::input::Rope,
    component::highlighter::{HighlightTheme, SyntaxHighlighter},
};
use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc, time::Duration};

type Span = (Range<usize>, HighlightStyle);
type Rows = Vec<Option<String>>;
type RowStyles = Vec<Vec<Span>>;
pub(super) type SharedStyles = Rc<RefCell<Styles>>;

#[derive(Default)]
pub(super) struct Styles {
    generation: u64,
    documents: Vec<RowStyles>,
}
impl Styles {
    pub(super) fn row(&self, side: usize, row: usize) -> &[Span] {
        self.documents
            .get(side)
            .and_then(|rows| rows.get(row))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

pub(super) struct Source {
    documents: Vec<Rows>,
    // In unified mode parse old/new separately so deleted code cannot change
    // the grammar context of the new code. Both retain display row indices.
    unified_removed: Option<Vec<bool>>,
}
pub(super) fn project(p: &git_diff::Presentation, rows: &[Rows]) -> Source {
    if rows.len() == 1 {
        Source {
            documents: [false, true]
                .map(|new| {
                    p.lines
                        .iter()
                        .map(|r| {
                            (!r.heading && r.origin != if new { "-" } else { "+" })
                                .then(|| r.text())
                        })
                        .collect()
                })
                .into(),
            unified_removed: Some(p.lines.iter().map(|r| r.origin == "-").collect()),
        }
    } else {
        Source {
            documents: rows.to_vec(),
            unified_removed: None,
        }
    }
}

#[derive(Default)]
pub(super) struct Cache {
    pub styles: SharedStyles,
    source: Option<Arc<Source>>,
    language: Option<&'static str>,
    theme: Option<Arc<HighlightTheme>>,
    task: Option<Task<()>>,
}
impl Cache {
    pub(super) fn clear(&mut self) {
        self.task = None;
        self.source = None;
        self.theme = None;
        let mut styles = self.styles.borrow_mut();
        styles.generation += 1;
        styles.documents.clear();
    }
    pub(super) fn set_source(&mut self, path: &str, source: Source, window: &Window, cx: &mut App) {
        self.clear();
        self.language = language(path);
        self.source = self.language.map(|_| Arc::new(source));
        self.refresh(window, cx);
    }
    pub(super) fn refresh(&mut self, window: &Window, cx: &mut App) {
        let theme = cx.theme().highlight_theme.clone();
        if self
            .theme
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &theme))
        {
            return;
        }
        self.task = None;
        let generation = {
            let mut styles = self.styles.borrow_mut();
            styles.generation += 1;
            styles.documents.clear();
            styles.generation
        };
        self.theme = Some(theme.clone());
        let (Some(source), Some(language)) = (self.source.clone(), self.language) else {
            return;
        };
        let result = Rc::downgrade(&self.styles);
        let window = window.window_handle();
        let theme = diff_theme(&theme, cx.theme().is_dark());
        let parse = cx
            .background_executor()
            .spawn(async move { highlight(&source, language, &theme) });
        self.task = Some(cx.spawn(async move |cx| {
            let documents = parse.await;
            let Some(result) = result.upgrade() else {
                return;
            };
            let mut styles = result.borrow_mut();
            if styles.generation != generation {
                return;
            }
            styles.documents = documents;
            drop(styles);
            let _ = window.update(cx, |_, window, _| window.refresh());
        }));
    }
}

fn language(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next()?.to_ascii_lowercase();
    Some(match name.as_str() {
        "makefile" | "gnumakefile" => "make",
        "cmakelists.txt" => "cmake",
        ".bashrc" | ".bash_profile" | ".zshrc" => "bash",
        _ => match name.rsplit_once('.')?.1 {
            "rs" => "rust",
            "js" | "jsx" | "mjs" | "cjs" => "javascript",
            "ts" | "mts" | "cts" => "typescript",
            "tsx" => "tsx",
            "json" => "json",
            "py" | "pyi" => "python",
            "sh" | "bash" | "zsh" => "bash",
            "c" | "h" => "c",
            "cpp" | "hpp" | "cc" | "cxx" => "cpp",
            "cs" => "csharp",
            "go" => "go",
            "html" | "htm" => "html",
            "css" => "css",
            "toml" => "toml",
            "yaml" | "yml" => "yaml",
            "md" | "markdown" => "markdown",
            "sql" => "sql",
            "swift" => "swift",
            "java" => "java",
            "kt" | "kts" => "kotlin",
            "rb" => "ruby",
            "php" => "php",
            "lua" => "lua",
            "proto" => "proto",
            "graphql" | "gql" => "graphql",
            "svelte" => "svelte",
            "astro" => "astro",
            "ex" | "exs" => "elixir",
            "scala" => "scala",
            "zig" => "zig",
            "cmake" => "cmake",
            "diff" | "patch" => "diff",
            _ => return None,
        },
    })
}

fn highlight(source: &Source, language: &str, theme: &HighlightTheme) -> Vec<RowStyles> {
    #[cfg(test)]
    crate::grammars::init().expect("bundled test grammars");
    // Large previews remain selectable/searchable plain text. This bounds the
    // optional parse work independently of agent pagination settings.
    if source
        .documents
        .iter()
        .flatten()
        .flatten()
        .map(String::len)
        .sum::<usize>()
        > 2 * 1024 * 1024
    {
        return Vec::new();
    }
    let mut documents: Vec<_> = source
        .documents
        .iter()
        .map(|rows| {
            let mut text = String::new();
            let mut ranges = Vec::with_capacity(rows.len());
            for row in rows {
                let start = text.len();
                if let Some(row) = row {
                    text.push_str(row);
                }
                ranges.push(start..text.len());
                text.push('\n');
            }
            let mut parser = SyntaxHighlighter::new(language);
            if !parser.update(
                None,
                &Rope::from_str(&text),
                Some(Duration::from_millis(100)),
            ) {
                return vec![vec![]; rows.len()];
            }
            let spans = parser.styles(&(0..text.len()), theme);
            ranges
                .iter()
                .map(|row| {
                    let first = spans.partition_point(|(r, _)| r.end <= row.start);
                    spans[first..]
                        .iter()
                        .take_while(|(r, _)| r.start < row.end)
                        .filter_map(|(r, style)| {
                            let start = r.start.max(row.start);
                            let end = r.end.min(row.end);
                            (start < end).then_some((
                                start - row.start..end - row.start,
                                HighlightStyle {
                                    // Keep the fixed-width layout and diff background unchanged.
                                    color: style.color,
                                    ..Default::default()
                                },
                            ))
                        })
                        .collect()
                })
                .collect::<RowStyles>()
        })
        .collect();
    if let Some(removed) = &source.unified_removed {
        let new = documents.pop().unwrap_or_default();
        let old = documents.pop().unwrap_or_default();
        vec![
            old.into_iter()
                .zip(new)
                .zip(removed)
                .map(|((old, new), removed)| if *removed { old } else { new })
                .collect(),
        ]
    } else {
        documents
    }
}

/// StyledText requires ordered, disjoint spans. Search backgrounds overlay
/// syntax foregrounds without replacing them or producing overlapping runs.
pub(super) fn overlay(syntax: &[Span], search: &[Span]) -> Vec<Span> {
    if search.is_empty() {
        return syntax.to_vec();
    }
    let mut edges: Vec<_> = syntax
        .iter()
        .chain(search)
        .flat_map(|(r, _)| [r.start, r.end])
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let (mut s, mut q) = (0, 0);
    let mut result = Vec::new();
    for edge in edges.windows(2) {
        let (start, end) = (edge[0], edge[1]);
        while s < syntax.len() && syntax[s].0.end <= start {
            s += 1;
        }
        while q < search.len() && search[q].0.end <= start {
            q += 1;
        }
        let base = syntax
            .get(s)
            .filter(|(r, _)| r.start <= start)
            .map(|(_, s)| *s);
        let mark = search
            .get(q)
            .filter(|(r, _)| r.start <= start)
            .map(|(_, s)| *s);
        if base.is_some() || mark.is_some() {
            result.push((
                start..end,
                base.unwrap_or_default().highlight(mark.unwrap_or_default()),
            ));
        }
    }
    result
}

fn diff_theme(source: &HighlightTheme, dark: bool) -> HighlightTheme {
    let mut theme = source.clone();
    if !dark {
        // CodeMirror's default light vocabulary, used by the original UI.
        let color = |hex| json!({"color": Hsla::from(rgb(hex))});
        theme.style.syntax = serde_json::from_value(json!({
            "comment": color(0x994400), "comment_doc": color(0x994400),
            "keyword": color(0x770088), "boolean": color(0x221199),
            "number": color(0x116644), "string": color(0xaa1111),
            "string.escape": color(0xee4400), "string.regex": color(0xee4400),
            "type": color(0x008855), "constructor": color(0x116677),
            "function": color(0x0000cc), "constant": color(0x0000cc),
            "variable.special": color(0x225566), "tag": color(0x117700),
            "attribute": color(0x0000cc)
        }))
        .expect("static Git syntax palette");
    }
    theme
}

#[cfg(test)]
mod tests {
    use super::{Cache, HighlightTheme, Source, highlight, language, overlay};
    use gpui_kit::{self as gpui, Context, HighlightStyle, IntoElement, Render, Window, div, rgb};
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    #[gpui::test]
    fn cache_reuses_repaint_and_invalidates_file_theme_and_clear(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, cx) = cx.add_window_view(|_, _| Empty);
        let mut cache = Cache::default();
        cx.update(|window, cx| {
            cache.set_source("a.rs", source(&[Some("fn main() {}")]), window, cx);
        });
        cx.run_until_parked();
        let light = cache.styles.borrow().row(0, 0).to_vec();
        assert!(!light.is_empty());
        let generation = cache.styles.borrow().generation;
        cx.update(|window, cx| cache.refresh(window, cx));
        assert_eq!(cache.styles.borrow().generation, generation);
        cx.update(|window, cx| {
            crate::theme::install(true, cx);
            cache.refresh(window, cx);
        });
        cx.run_until_parked();
        assert_ne!(cache.styles.borrow().row(0, 0), light);
        // Replace an in-flight parse with an unsupported file before delivery.
        cx.update(|window, cx| {
            cache.set_source("b.rs", source(&[Some("let b = 42;")]), window, cx);
            cache.set_source("plain.txt", source(&[Some("plain text")]), window, cx);
        });
        cx.run_until_parked();
        assert!(cache.styles.borrow().row(0, 0).is_empty());
        cx.update(|window, cx| {
            cache.set_source("a.rs", source(&[Some("fn main() {}")]), window, cx);
            cache.clear();
        });
        cx.run_until_parked();
        assert!(cache.styles.borrow().row(0, 0).is_empty());
    }
    fn source(rows: &[Option<&str>]) -> Source {
        Source {
            documents: vec![rows.iter().map(|r| r.map(str::to_owned)).collect()],
            unified_removed: None,
        }
    }
    #[test]
    fn filename_resolution_and_unknown_fallback() {
        for (path, expected) in [
            ("src/Main.TSX", Some("tsx")),
            ("Makefile", Some("make")),
            ("build/CMakeLists.txt", Some("cmake")),
            (".bashrc", Some("bash")),
            ("dir.rs/plain", None),
            ("asset.png", None),
            ("", None),
        ] {
            assert_eq!(language(path), expected);
        }
    }
    #[test]
    fn unicode_multiline_styles_preserve_row_coordinates_and_theme() {
        let source = source(&[
            None,
            Some("/* comment"),
            Some("雪 comment */"),
            None,
            Some("fn main() { let café = \"été\"; }"),
        ]);
        let light = highlight(&source, "rust", &HighlightTheme::default_light());
        let dark = highlight(&source, "rust", &HighlightTheme::default_dark());
        assert!(light[0][0].is_empty());
        assert!(light[0][3].is_empty());
        assert_eq!(light[0][1][0].1.color, light[0][2][0].1.color);
        assert_ne!(light[0][1][0].1.color, light[0][4][0].1.color);
        assert_ne!(light, dark);
        for (row, spans) in source.documents[0].iter().zip(&light[0]) {
            for (range, _) in spans {
                let text = row.as_ref().unwrap();
                assert!(range.start < range.end && range.end <= text.len());
                assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            }
            assert!(
                spans
                    .windows(2)
                    .all(|pair| pair[0].0.end <= pair[1].0.start)
            );
        }
    }
    #[test]
    fn unified_deleted_comment_does_not_recolor_new_code() {
        let source = Source {
            documents: vec![
                vec![Some("/* removed unterminated comment".into()), None],
                vec![None, Some("fn main() {}".into())],
            ],
            unified_removed: Some(vec![true, false]),
        };
        let result = highlight(&source, "rust", &HighlightTheme::default_light());
        assert_eq!(result.len(), 1);
        assert_ne!(result[0][0][0].1.color, result[0][1][0].1.color);
    }
    #[test]
    fn search_overlay_preserves_foreground_and_disjoint_utf8_ranges() {
        let red = HighlightStyle {
            color: Some(rgb(0xff0000).into()),
            ..Default::default()
        };
        let mark = HighlightStyle {
            background_color: Some(rgb(0xeeee00).into()),
            ..Default::default()
        };
        let merged = overlay(
            &[(0..5, red), (5..9, red)],
            &[(0..0, mark), (2..7, mark), (9..12, mark)],
        );
        assert_eq!(
            merged.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            [0..2, 2..5, 5..7, 7..9, 9..12]
        );
        assert_eq!(merged[1].1.color, red.color);
        assert_eq!(merged[1].1.background_color, mark.background_color);
        assert_eq!(merged[3].1.background_color, None);
        assert_eq!(merged[4].1.color, None);
    }
    #[test]
    fn oversized_preview_falls_back_to_plain_text() {
        let source = Source {
            documents: vec![vec![Some("x".repeat(2 * 1024 * 1024 + 1))]],
            unified_removed: None,
        };
        assert!(highlight(&source, "rust", &HighlightTheme::default_light()).is_empty());
    }
}
