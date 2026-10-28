//! Terminal palette and minimum-contrast correction: 4.5:1 for text, 2.25:1 for dim text.
//! The contrast stepping is derived from xterm.js Color.ts (MIT; assets/XTERM-LICENSE).
use newport_terminal::{Cell, Frame, PaletteIndex, RgbColor as Rgb, StyleColor, Theme};
use std::{collections::HashMap, sync::OnceLock};

pub(super) fn from_hex(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}
pub(super) fn hex(color: Rgb) -> u32 {
    ((color.r as u32) << 16) | ((color.g as u32) << 8) | color.b as u32
}

/// Default palette shared with OSC color-query replies. Explicit OSC overrides
/// are resolved from the terminal frame by the renderer.
pub fn palette(index: usize, dark: bool) -> Rgb {
    // The app theme overrides only default fg/bg, not the ANSI palette.
    const ANSI: [u32; 16] = [
        0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, 0x555753,
        0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
    ];
    match index {
        0..=15 => from_hex(ANSI[index]),
        16..=231 => {
            let n = index - 16;
            let component = |n: usize| if n == 0 { 0 } else { (55 + n * 40) as u8 };
            Rgb {
                r: component(n / 36),
                g: component(n / 6 % 6),
                b: component(n % 6),
            }
        }
        232..=255 => {
            let level = (8 + (index - 232) * 10) as u8;
            Rgb {
                r: level,
                g: level,
                b: level,
            }
        }
        257 => from_hex(if dark { 0x1e1f21 } else { 0xffffff }),
        259..=266 => blend(palette(257, dark), palette(index - 259, dark)),
        268 => blend(palette(257, dark), palette(256, dark)),
        _ => from_hex(if dark { 0xeeeeef } else { 0x25262a }),
    }
}

pub fn theme(dark: bool) -> Theme {
    Theme {
        foreground: palette(256, dark),
        background: palette(257, dark),
        cursor: palette(258, dark),
        palette: std::array::from_fn(|index| palette(index, dark)),
    }
}

pub(super) fn resolve(color: StyleColor, frame: &Frame, default: Rgb) -> Rgb {
    match color {
        StyleColor::None => default,
        StyleColor::Palette(index) => frame.palette[usize::from(index.0)],
        StyleColor::Rgb(color) => color,
    }
}

/// Blend opaque colors at a quantized 128/255 alpha (dim text and selection).
fn blend(background: Rgb, foreground: Rgb) -> Rgb {
    let channel = |bg: u8, fg: u8| ((bg as u32 * 127 + fg as u32 * 128 + 127) / 255) as u8;
    Rgb {
        r: channel(background.r, foreground.r),
        g: channel(background.g, foreground.g),
        b: channel(background.b, foreground.b),
    }
}
fn luminance(color: Rgb) -> f64 {
    static LINEAR: OnceLock<[f64; 256]> = OnceLock::new();
    let table = LINEAR.get_or_init(|| {
        std::array::from_fn(|i| {
            let s = i as f64 / 255.;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    table[color.r as usize] * 0.2126
        + table[color.g as usize] * 0.7152
        + table[color.b as usize] * 0.0722
}
fn ratio(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Reverse the default text/background roles inside a block cursor. Remote
/// OSC cursor colors can coincide with the background, so fall back to the
/// more legible of black and white when that reversal has insufficient contrast.
pub(super) fn cursor_colors(frame: &Frame) -> (Rgb, Rgb) {
    let background = frame.cursor_color.unwrap_or(frame.foreground);
    let background_luminance = luminance(background);
    let foreground = if ratio(background_luminance, luminance(frame.background)) >= 4.5 {
        frame.background
    } else if ratio(background_luminance, 0.) >= ratio(background_luminance, 1.) {
        from_hex(0)
    } else {
        from_hex(0xffffff)
    };
    (foreground, background)
}
fn adjust(background: Rgb, foreground: Rgb, minimum: f64) -> Option<Rgb> {
    let bg = luminance(background);
    let fg = luminance(foreground);
    if ratio(bg, fg) >= minimum {
        return None;
    }
    let step = |mut value: Rgb, increase: bool| {
        let channel = |c: u8| if increase {
            c as u16 + (255 - c as u16).div_ceil(10)
        } else { c as u16 - (c as u16).div_ceil(10) } as u8;
        while ratio(bg, luminance(value)) < minimum {
            let next = Rgb {
                r: channel(value.r),
                g: channel(value.g),
                b: channel(value.b),
            };
            if next == value {
                break;
            }
            value = next;
        }
        value
    };
    let first = step(foreground, fg >= bg);
    if ratio(bg, luminance(first)) >= minimum {
        return Some(first);
    }
    let second = step(foreground, fg < bg);
    Some(
        if ratio(bg, luminance(first)) > ratio(bg, luminance(second)) {
            first
        } else {
            second
        },
    )
}

#[derive(Default)]
pub(super) struct ContrastCache(HashMap<(u32, u32, bool), Option<Rgb>>);
impl ContrastCache {
    fn foreground(&mut self, bg: Rgb, fg: Rgb, dim: bool, decorative: bool) -> Rgb {
        if !decorative {
            let corrected = self
                .0
                .entry((hex(bg), hex(fg), dim))
                .or_insert_with(|| adjust(bg, fg, if dim { 2.25 } else { 4.5 }));
            if let Some(corrected) = corrected {
                return *corrected;
            }
        }
        if dim { blend(bg, fg) } else { fg }
    }
}
fn decorative(c: char) -> bool {
    matches!(c as u32, 0xe0a4..=0xe0d6 | 0x2500..=0x259f)
}
fn bright(color: StyleColor, bold: bool) -> StyleColor {
    match color {
        StyleColor::Palette(index) if bold && index.0 < 8 => {
            StyleColor::Palette(PaletteIndex(index.0 + 8))
        }
        _ => color,
    }
}

pub(super) fn cell_colors(
    cell: &Cell,
    frame: &Frame,
    dark: bool,
    search_background: Option<Rgb>,
    contrast: &mut ContrastCache,
) -> (Rgb, Rgb) {
    let style = cell.style;
    let (fg, bg, default_fg, default_bg) = if style.inverse {
        (
            style.bg_color,
            style.fg_color,
            frame.background,
            frame.foreground,
        )
    } else {
        (
            style.fg_color,
            style.bg_color,
            frame.foreground,
            frame.background,
        )
    };
    let mut foreground = resolve(bright(fg, style.bold), frame, default_fg);
    let original_bg = resolve(bg, frame, default_bg);
    let mut background = search_background.unwrap_or(original_bg);
    let decorative = cell.text.chars().next().is_some_and(decorative);
    if cell.selected {
        let selection = from_hex(if dark { 0x454349 } else { 0xdad9dd });
        background = if style.inverse || style.bg_color != StyleColor::None {
            blend(original_bg, selection)
        } else {
            selection
        };
        // Powerline/block glyphs are background-like shapes, not text labels.
        if decorative {
            foreground = if style.inverse && style.bg_color == StyleColor::None {
                selection
            } else {
                blend(resolve(fg, frame, default_fg), selection)
            };
        }
    }
    // Selected text is never dimmed, so it stays legible on the selection color.
    let dim = style.faint && !cell.selected;
    (
        contrast.foreground(background, foreground, dim, decorative),
        background,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn color_cube_and_grayscale_ramp_are_shared_by_both_themes() {
        for dark in [false, true] {
            for (index, expected) in [
                (16, 0x000000),
                (21, 0x0000ff),
                (67, 0x5f87af),
                (196, 0xff0000),
                (231, 0xffffff),
                (232, 0x080808),
                (244, 0x808080),
                (255, 0xeeeeee),
            ] {
                assert_eq!(hex(palette(index, dark)), expected, "palette {index}");
            }
        }
    }

    #[test]
    fn contrast_correction_reaches_the_minimum_or_the_best_extreme() {
        let backgrounds = [0xffffff, 0x1e1f21, 0x777777, 0x000080, 0xfce94f];
        for bg in backgrounds.map(from_hex) {
            let foregrounds = (0..256).map(|index| palette(index, true)).chain([
                bg,
                from_hex(0xeeeeef),
                from_hex(0x25262a),
            ]);
            for fg in foregrounds {
                for minimum in [4.5, 2.25] {
                    let before = ratio(luminance(bg), luminance(fg));
                    let Some(corrected) = adjust(bg, fg, minimum) else {
                        assert!(before >= minimum, "{:06x} on {:06x}", hex(fg), hex(bg));
                        continue;
                    };
                    assert!(before < minimum, "legible text is never changed");
                    let after = ratio(luminance(bg), luminance(corrected));
                    assert!(
                        after >= minimum || matches!(hex(corrected), 0x000000 | 0xffffff),
                        "{:06x} on {:06x} at {minimum}: {:06x} reaches {after}",
                        hex(fg),
                        hex(bg),
                        hex(corrected)
                    );
                    assert!(after > before);
                }
            }
        }
    }
    fn frame(dark: bool) -> Frame {
        let mut engine = newport_terminal::Engine::new(8, 2).unwrap();
        engine.set_theme(theme(dark)).unwrap();
        engine.frame().unwrap().clone()
    }

    #[test]
    fn block_cursor_text_remains_readable_in_both_themes_and_osc_overrides() {
        for dark in [false, true] {
            let mut engine = newport_terminal::Engine::new(8, 2).unwrap();
            engine.set_theme(theme(dark)).unwrap();
            let (fg, bg) = cursor_colors(engine.frame().unwrap());
            assert_eq!(fg, theme(dark).background);
            assert_eq!(bg, theme(dark).cursor);
            assert!(ratio(luminance(fg), luminance(bg)) >= 4.5);
            for color in [0x000000, 0xffffff, 0x25262a, 0x1e1f21, 0x777777, 0xffcc00] {
                engine
                    .advance(format!("\x1b]12;#{color:06x}\x07").as_bytes())
                    .unwrap();
                let (fg, bg) = cursor_colors(engine.frame().unwrap());
                assert_eq!(hex(bg), color, "preserve the remote cursor color");
                assert!(ratio(luminance(fg), luminance(bg)) >= 4.5);
            }
        }
    }
    fn cell(text: &str) -> Cell {
        Cell {
            text: text.into(),
            wide: newport_terminal::CellWide::Narrow,
            style: Default::default(),
            foreground: None,
            background: None,
            selected: false,
            hyperlink: None,
        }
    }
    #[test]
    fn selection_preserves_backgrounds_and_dim_uses_the_visible_background() {
        let mut cache = ContrastCache::default();
        let mut cell = cell("A");
        cell.selected = true;
        for (dark, selection) in [(false, 0xdad9dd), (true, 0x454349)] {
            let frame = frame(dark);
            assert_eq!(
                hex(cell_colors(&cell, &frame, dark, None, &mut cache).1),
                selection
            );
            cell.style.bg_color = StyleColor::Rgb(from_hex(0xff0000));
            assert_eq!(
                cell_colors(&cell, &frame, dark, None, &mut cache).1,
                blend(from_hex(0xff0000), from_hex(selection))
            );
            cell.style.bg_color = StyleColor::None;
        }
        let frame = frame(false);
        cell.selected = false;
        cell.style.faint = true;
        let (fg, bg) = cell_colors(&cell, &frame, false, None, &mut cache);
        assert_eq!(fg, blend(bg, palette(256, false)));
        assert!(fg.r > palette(256, false).r);
        cell.selected = true;
        assert_eq!(
            cell_colors(&cell, &frame, false, None, &mut cache).0,
            palette(256, false)
        );
    }
    #[test]
    fn bold_inverse_osc_and_decorative_glyphs_keep_their_color_roles() {
        let mut frame = frame(true);
        let mut cache = ContrastCache::default();
        let mut cell = cell("A");
        cell.style.fg_color = StyleColor::Palette(PaletteIndex::RED);
        cell.style.bold = true;
        frame.palette[9] = from_hex(0xffffff);
        assert_eq!(
            cell_colors(&cell, &frame, true, None, &mut cache).0,
            from_hex(0xffffff)
        );
        cell.style.inverse = true;
        cell.style.bg_color = StyleColor::Palette(PaletteIndex::GREEN);
        frame.palette[10] = from_hex(0xffffff);
        let (fg, bg) = cell_colors(&cell, &frame, true, None, &mut cache);
        assert_eq!(fg, from_hex(0xffffff));
        assert_eq!(bg, palette(1, true));
        cell.text = "█".into();
        cell.style = Default::default();
        cell.style.fg_color = StyleColor::Rgb(from_hex(0x222222));
        assert_eq!(
            cell_colors(&cell, &frame, true, None, &mut cache).0,
            from_hex(0x222222)
        );
        cell.text = "A".into();
        let (fg, bg) = cell_colors(&cell, &frame, true, None, &mut cache);
        assert!(ratio(luminance(fg), luminance(bg)) >= 4.5);
        let count = cache.0.len();
        cell_colors(&cell, &frame, true, None, &mut cache);
        assert_eq!(cache.0.len(), count);
    }
    #[test]
    fn terminal_frame_preserves_osc_defaults_and_underline_palette_overrides() {
        let mut engine = newport_terminal::Engine::new(8, 2).unwrap();
        engine.set_theme(theme(true)).unwrap();
        engine
            .advance(b"\x1b]10;#fafafa\x07\x1b]11;#102030\x07\x1b]4;1;#ffffff\x07\x1b[58;5;1mA")
            .unwrap();
        let frame = engine.frame().unwrap();
        let cell = &frame.rows[0].cells[0];
        let mut cache = ContrastCache::default();
        assert_eq!(
            cell_colors(cell, frame, true, None, &mut cache),
            (from_hex(0xfafafa), from_hex(0x102030))
        );
        assert_eq!(
            resolve(cell.style.underline_color, frame, frame.foreground),
            from_hex(0xffffff)
        );
    }
}
