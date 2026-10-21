//! Offline visual fixture for the native terminal renderer. Opens no PTY.
//! cargo xtask env -- cargo run --locked -p newport-gpui --example terminal_render_probe
#[path = "../src/terminal_color.rs"]
mod terminal_color;
#[path = "../src/terminal_paint.rs"]
mod terminal_paint;
#[path = "../src/terminal_search_paint.rs"]
mod terminal_search_paint;
// The renderer reads the shared metrics; the probe doesn't draw headers.
#[allow(dead_code)]
#[path = "../src/ui.rs"]
mod ui;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{prelude::*, *};
use newport_terminal::Engine;

struct Probe {
    engine: Engine,
    reported_cell: Option<Size<Pixels>>,
}
impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.reported_cell.is_none() && std::env::args().any(|arg| arg == "--check-metrics") {
            check_metrics(window, cx);
            cx.quit();
        }
        let cell = terminal_paint::cell_size(window, cx);
        if self.reported_cell != Some(cell) {
            println!(
                "Terminal cell: {cell:?}, device scale: {}",
                window.scale_factor()
            );
            self.reported_cell = Some(cell);
        }
        let matches = self.engine.search("match", false).expect("search fixture");
        div()
            .size_full()
            .flex()
            .flex_col()
            .children([false, true].map(|dark| {
                self.engine
                    .set_theme(terminal_paint::theme(dark))
                    .expect("set fixture theme");
                let frame = self.engine.frame().expect("render fixture frame");
                div()
                    .w_full()
                    .h(px(270.))
                    .child(terminal_paint::render_grid(
                        &frame,
                        dark,
                        cell,
                        &matches.matches,
                        matches.matches.first(),
                        None,
                        true,
                    ))
            }))
    }
}

// Uses the real platform font shaper, not GPUI's fixed-width test text system.
fn check_metrics(window: &mut Window, cx: &App) {
    let base = terminal_paint::terminal_font(cx);
    let measure = |font: Font, text: &str| {
        window
            .text_system()
            .shape_line(
                text.to_owned().into(),
                px(12.),
                &[TextRun {
                    len: text.len(),
                    font,
                    color: rgb(0).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
    };
    let width = measure(base.clone(), "W");
    println!("Terminal font: {}, ASCII advance: {width:?}", base.family);
    for (weight, style) in [
        (FontWeight::NORMAL, FontStyle::Normal),
        (FontWeight::BOLD, FontStyle::Normal),
        (FontWeight::NORMAL, FontStyle::Italic),
        (FontWeight::BOLD, FontStyle::Italic),
    ] {
        let mut font = base.clone();
        font.weight = weight;
        font.style = style;
        for character in ' '..='~' {
            let actual = measure(font.clone(), &character.to_string());
            assert!(
                (f32::from(actual - width)).abs() < 0.01,
                "non-monospace terminal glyph {character:?}: {actual:?}, expected {width:?}, {weight:?}/{style:?}"
            );
        }
    }
    println!("Terminal font metrics passed (95 ASCII glyphs, regular/bold/italic/bold italic)");
}
fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() { cx.quit(); }
        }).detach();
        cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(100.), px(100.)), size(px(800.), px(540.))))),
            titlebar: Some(TitlebarOptions { title: Some("Terminal render probe — offline".into()), ..Default::default() }),
            ..Default::default()
        }, |_, cx| cx.new(|_| {
            let mut engine = Engine::new(100, 18).unwrap();
            engine.advance(concat!(
                "Terminal rendering fixture: light / dark\r\n",
                "Plain text | \x1b[1mbold\x1b[0m | \x1b[3mitalic\x1b[0m | \x1b[2mdim\x1b[0m\r\n",
                "\x1b[31mRed \x1b[32mGreen \x1b[34mBlue \x1b[0m| \x1b[7minverse\x1b[0m\r\n",
                "\x1b[4mUnderline\x1b[0m | \x1b[21mDouble underline\x1b[0m | \x1b[9mStrike\x1b[0m\r\n",
                "\x1b[4:3mCurly\x1b[0m | \x1b[4:4mDotted\x1b[0m | \x1b[4:5mDashed\x1b[0m | \x1b[4:3;58:2::255:100:0m界e\u{301}\x1b[0m\r\n",
                "Wide: 界中文 | Combining: e\u{301} a\u{308} | Emoji: 😀 🚀\r\n",
                "Find: match and another match\r\n",
                "\x1b[44mBlue background     \x1b[0m | \x1b[8mhidden\x1b[0m visible\r\n",
                "Wide block cursor: 界\x1b[2D"
            ).as_bytes()).expect("parse fixture");
            Probe { engine, reported_cell: None }
        })).expect("open offline renderer probe");
        cx.activate(true);
    });
}
