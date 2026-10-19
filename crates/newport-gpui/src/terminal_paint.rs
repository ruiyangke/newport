//! Native fixed-cell painting of an owned Ghostty viewport snapshot.
use gpui_kit::component::ActiveTheme;
use gpui_kit::{prelude::*, *};
use newport_terminal::{CellWide, CursorVisualStyle, Frame, Position, Style, Underline};
use std::collections::HashMap;

const FONT_SIZE: f32 = crate::ui::FONT_SIZE;

pub use crate::terminal_color::theme;
use crate::terminal_color::{from_hex, hex, resolve};

pub fn terminal_font(cx: &App) -> Font {
    // Font::fallbacks only supplies missing glyphs after the primary face loads.
    // A missing Menlo on Windows otherwise resolves to proportional system text.
    // Kit resolves its platform monospace family against installed fonts once.
    let mut font = font(cx.theme().mono_font_family.clone());
    let mut fallbacks = vec![];
    if cfg!(target_os = "macos") {
        fallbacks.push("Monaco".into());
    }
    fallbacks.push("Symbols Nerd Font Mono".into());
    font.fallbacks = Some(FontFallbacks::from_fonts(fallbacks));
    font
}

fn run(text: &str, foreground: u32, style: Style, cx: &App) -> TextRun {
    let mut font = terminal_font(cx);
    if style.bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len: text.len(),
        font,
        color: rgb(foreground).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

pub fn cell_size(window: &mut Window, cx: &App) -> Size<Pixels> {
    let sample = window.text_system().shape_line(
        "W".into(),
        px(FONT_SIZE),
        &[run("W", 0, Style::default(), cx)],
        None,
    );
    device_cell_size(
        size(sample.width, sample.ascent + sample.descent),
        window.scale_factor(),
    )
}

/// Match xterm's WebGL cell dimensions with lineHeight=1 and letterSpacing=0.
/// Snap in device pixels, then convert back to logical coordinates so paint,
/// hit testing and PTY sizing all use the same grid, including after a display move.
fn device_cell_size(measured: Size<Pixels>, scale: f32) -> Size<Pixels> {
    size(
        px((f32::from(measured.width) * scale).floor().max(1.) / scale),
        px((f32::from(measured.height) * scale).ceil().max(1.) / scale),
    )
}

struct PaintCell {
    row: i32,
    column: usize,
    text: String,
    foreground: u32,
    background: u32,
    underline: u32,
    style: Style,
    wide: bool,
    spacer: bool,
    cursor: Option<CursorVisualStyle>,
}

pub fn render_grid(
    frame: &Frame,
    dark: bool,
    cell_size: Size<Pixels>,
    matches: &[std::ops::RangeInclusive<Position>],
    active: Option<&std::ops::RangeInclusive<Position>>,
    hovered_link: Option<&str>,
    focused: bool,
) -> AnyElement {
    let background = hex(frame.background);
    let (cursor_foreground, cursor_background) = crate::terminal_color::cursor_colors(frame);
    let cursor_color = hex(cursor_background);
    let search_borders = crate::terminal_search_paint::segments(
        matches,
        active,
        frame.viewport_top,
        frame.rows.len(),
        usize::from(frame.columns),
    );
    let mut cells = Vec::new();
    let mut contrast = crate::terminal_color::ContrastCache::default();
    for (row, line) in frame.rows.iter().enumerate() {
        for (column, cell) in line.cells.iter().enumerate() {
            let position = Position {
                row: (frame.viewport_top + row) as u32,
                column: column as u16,
            };
            let mut style = cell.style;
            if !style.invisible
                && style.underline == Underline::None
                && hovered_link.is_some()
                && hovered_link == frame.link_at(column, row)
            {
                style.underline = Underline::Single;
            }
            let mut search_background = None;
            let index = matches.partition_point(|range| range.end() < &position);
            if matches
                .get(index)
                .is_some_and(|range| range.contains(&position))
            {
                search_background = Some(from_hex(if dark { 0x8a7a2a } else { 0xf6e3a1 }));
            }
            if active.is_some_and(|range| range.contains(&position)) {
                search_background = Some(from_hex(if dark { 0xc77800 } else { 0xffb74d }));
            }
            let (mut foreground, mut bg) = crate::terminal_color::cell_colors(
                cell,
                frame,
                dark,
                search_background,
                &mut contrast,
            );
            let cursor = cursor_at(frame, row, column, focused);
            let mut underline = resolve(style.underline_color, frame, foreground);
            if cursor == Some(CursorVisualStyle::Block) {
                foreground = cursor_foreground;
                bg = cursor_background;
                underline = cursor_foreground;
            }
            let text = if style.invisible
                || matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead)
            {
                String::new()
            } else {
                cell.text.clone()
            };
            cells.push(PaintCell {
                row: row as i32,
                column,
                text,
                foreground: hex(foreground),
                background: hex(bg),
                underline: hex(underline),
                style,
                wide: cell.wide == CellWide::Wide,
                spacer: matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead),
                cursor,
            });
        }
    }
    let underlines = underline_runs(&cells);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                window.paint_quad(fill(bounds, rgb(background)));
                // Paint backgrounds before glyphs so a wide glyph cannot be erased
                // by its trailing spacer's background.
                window.paint_layer(bounds, |window| {
                    for cell in &cells {
                        let origin = bounds.origin
                            + point(
                                cell_size.width * cell.column as f32,
                                cell_size.height * cell.row as f32,
                            );
                        window.paint_quad(fill(
                            Bounds::new(origin, size(cell_size.width, cell_size.height)),
                            rgb(cell.background),
                        ));
                    }
                });
                // A wide block cursor also covers its spacer cell. Draw it
                // after all cell backgrounds and before the glyph layer.
                for cell in cells
                    .iter()
                    .filter(|cell| cell.cursor == Some(CursorVisualStyle::Block))
                {
                    let origin = bounds.origin
                        + point(
                            cell_size.width * cell.column as f32,
                            cell_size.height * cell.row as f32,
                        );
                    let width = cell_size.width * if cell.wide { 2. } else { 1. };
                    window.paint_quad(fill(
                        Bounds::new(origin, size(width, cell_size.height)),
                        rgb(cursor_color),
                    ));
                }
                // Repeated glyph/style combinations are shaped once in this frame.
                // Each cluster is positioned explicitly; ligatures cannot change
                // subsequent terminal columns.
                let mut shaped = HashMap::new();
                let mut glyphs = Vec::with_capacity(cells.len());
                for cell in &cells {
                    let origin = bounds.origin
                        + point(
                            cell_size.width * cell.column as f32,
                            cell_size.height * cell.row as f32,
                        );
                    if !cell.text.is_empty() && cell.text != " " {
                        let key = (
                            cell.text.clone(),
                            cell.foreground,
                            (cell.style.bold, cell.style.italic),
                        );
                        let line = shaped.entry(key).or_insert_with(|| {
                            window.text_system().shape_line(
                                cell.text.clone().into(),
                                px(FONT_SIZE),
                                &[run(&cell.text, cell.foreground, cell.style, cx)],
                                None,
                            )
                        });
                        collect_cell_glyphs(
                            line,
                            origin,
                            cell_size.height,
                            rgb(cell.foreground).into(),
                            &mut glyphs,
                        );
                    }
                }
                // GPUI batches monochrome and color sprites separately within a
                // layer. Split at each transition to preserve terminal paint order:
                // an overflowing emoji must not cover the following text. Ordinary
                // text still shares one layer rather than allocating one per cell.
                for batch in glyphs.chunk_by(|a, b| a.is_emoji == b.is_emoji) {
                    window.paint_layer(bounds, |window| {
                        for glyph in batch {
                            glyph.paint(window);
                        }
                    });
                }
                // Decorations must overlay glyphs, including color emoji.
                window.paint_layer(bounds, |window| {
                    for underline in &underlines {
                        if let Some(path) = underline_path(
                            underline,
                            cell_size,
                            bounds.origin,
                            window.scale_factor(),
                        ) {
                            window.paint_path(path, rgb(underline.color));
                        }
                    }
                    for cell in &cells {
                        let origin = bounds.origin
                            + point(
                                cell_size.width * cell.column as f32,
                                cell_size.height * cell.row as f32,
                            );
                        let width = cell_size.width * if cell.wide { 2. } else { 1. };
                        if !cell.style.invisible && !cell.spacer && cell.style.strikethrough {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(origin.x, origin.y + cell_size.height * 0.5),
                                    size(width, px(1.)),
                                ),
                                rgb(cell.foreground),
                            ));
                        }
                        let cursor_bounds = match cell.cursor {
                            Some(CursorVisualStyle::Bar) => {
                                Some(Bounds::new(origin, size(px(2.), cell_size.height)))
                            }
                            Some(CursorVisualStyle::Underline) => Some(Bounds::new(
                                point(origin.x, origin.y + cell_size.height - px(2.)),
                                size(width, px(2.)),
                            )),
                            _ => None,
                        };
                        if let Some(rect) = cursor_bounds {
                            window.paint_quad(fill(rect, rgb(cursor_color)));
                        }
                        if cell.cursor == Some(CursorVisualStyle::BlockHollow) {
                            for rect in [
                                Bounds::new(origin, size(width, px(1.))),
                                Bounds::new(
                                    point(origin.x, origin.y + cell_size.height - px(1.)),
                                    size(width, px(1.)),
                                ),
                                Bounds::new(origin, size(px(1.), cell_size.height)),
                                Bounds::new(
                                    point(origin.x + width - px(1.), origin.y),
                                    size(px(1.), cell_size.height),
                                ),
                            ] {
                                window.paint_quad(fill(rect, rgb(cursor_color)));
                            }
                        }
                    }
                    for (row, column, columns, active) in &search_borders {
                        let rect = Bounds::new(
                            bounds.origin
                                + point(
                                    cell_size.width * *column as f32,
                                    cell_size.height * *row as f32,
                                ),
                            size(cell_size.width * *columns as f32, cell_size.height),
                        );
                        window.paint_quad(outline(
                            rect,
                            rgb(crate::terminal_search_paint::border_color(dark, *active)),
                            BorderStyle::default(),
                        ));
                    }
                });
            });
        },
    )
    .size_full()
    .into_any_element()
}

#[derive(Clone, Debug, PartialEq)]
struct UnderlineRun {
    row: i32,
    column: usize,
    columns: usize,
    style: Underline,
    color: u32,
}

/// Join adjacent cells before tessellation. Wide tails inherit their owner's
/// decoration; painting them again would erase curly/dotted gaps or darken edges.
fn underline_runs(cells: &[PaintCell]) -> Vec<UnderlineRun> {
    let mut runs: Vec<UnderlineRun> = Vec::new();
    for cell in cells {
        if cell.spacer || cell.style.invisible || cell.style.underline == Underline::None {
            continue;
        }
        let columns = if cell.wide { 2 } else { 1 };
        if let Some(last) = runs.last_mut()
            && last.row == cell.row
            && last.column + last.columns == cell.column
            && last.style == cell.style.underline
            && last.color == cell.underline
        {
            last.columns += columns;
        } else {
            runs.push(UnderlineRun {
                row: cell.row,
                column: cell.column,
                columns,
                style: cell.style.underline,
                color: cell.underline,
            });
        }
    }
    runs
}

/// Build the actual filled geometry submitted to GPUI. Patterns are anchored to
/// grid columns, so a color/selection boundary cannot restart a dotted pattern.
/// The five styles and device-pixel stroke width follow xterm's TextureAtlas.
fn underline_path(
    run: &UnderlineRun,
    cell: Size<Pixels>,
    origin: Point<Pixels>,
    scale: f32,
) -> Option<Path<Pixels>> {
    if run.columns == 0 || run.style == Underline::None {
        return None;
    }
    let unit = f32::from(cell.width);
    let height = f32::from(cell.height);
    if unit <= 0. || height <= 0. || !scale.is_finite() || scale <= 0. {
        return None;
    }
    let stroke = (FONT_SIZE * scale / 15.).floor().max(1.) / scale;
    let left = run.column as f32 * unit;
    let right = left + run.columns as f32 * unit;
    let baseline = (run.row + 1) as f32 * height - 2.;
    let mut path = PathBuilder::fill();
    let mut bar = |x: f32, width: f32, y: f32| {
        if width <= 0. {
            return;
        }
        path.add_polygon(
            &[
                point(px(x), px(y)),
                point(px(x + width), px(y)),
                point(px(x + width), px(y + stroke)),
                point(px(x), px(y + stroke)),
            ],
            true,
        );
    };
    match run.style {
        Underline::Double => {
            bar(left, right - left, baseline);
            bar(left, right - left, baseline - 2.);
        }
        Underline::Dotted => {
            let period = stroke * 2.;
            let mut x = (left / period).floor() * period;
            while x < right {
                let start = x.max(left);
                bar(start, (x + stroke).min(right) - start, baseline);
                x += period;
            }
        }
        Underline::Dashed => {
            // Repeat xterm's 60% line / 30% gap / remaining end segment per cell.
            let length = (unit * scale * 0.6).floor().max(1.) / scale;
            let gap = (unit * scale * 0.3).floor().max(1.) / scale;
            for column in 0..run.columns {
                let x = left + column as f32 * unit;
                bar(x, length.min(unit), baseline);
                let end = (length + gap).min(unit);
                bar(x + end, unit - end, baseline);
            }
        }
        Underline::Curly => {
            // Filled parallel curves stay inside the cell horizontally, unlike
            // stroked end caps, and join without seams across color boundaries.
            let center = baseline - 0.5;
            let amplitude = 4. / 3.; // Cubic controls yield one logical pixel of amplitude.
            let top = center - stroke / 2.;
            let bottom = center + stroke / 2.;
            path.move_to(point(px(left), px(top)));
            for column in 0..run.columns {
                let x = left + column as f32 * unit;
                path.cubic_bezier_to(
                    point(px(x + unit / 2.), px(top)),
                    point(px(x + unit / 6.), px(top + amplitude)),
                    point(px(x + unit / 3.), px(top + amplitude)),
                );
                path.cubic_bezier_to(
                    point(px(x + unit), px(top)),
                    point(px(x + unit * 2. / 3.), px(top - amplitude)),
                    point(px(x + unit * 5. / 6.), px(top - amplitude)),
                );
            }
            path.line_to(point(px(right), px(bottom)));
            for column in (0..run.columns).rev() {
                let x = left + column as f32 * unit;
                path.cubic_bezier_to(
                    point(px(x + unit / 2.), px(bottom)),
                    point(px(x + unit * 5. / 6.), px(bottom - amplitude)),
                    point(px(x + unit * 2. / 3.), px(bottom - amplitude)),
                );
                path.cubic_bezier_to(
                    point(px(x), px(bottom)),
                    point(px(x + unit / 3.), px(bottom + amplitude)),
                    point(px(x + unit / 6.), px(bottom + amplitude)),
                );
            }
            path.close();
        }
        _ => bar(left, right - left, baseline),
    }
    path.translate(origin);
    path.build().ok()
}

/// Ghostty reports a cursor on a wide tail explicitly. Paint on the owning
/// cell so the cursor spans the same glyph, selection and IME grid geometry.
fn cursor_at(frame: &Frame, row: usize, column: usize, focused: bool) -> Option<CursorVisualStyle> {
    if !frame.cursor.visible {
        return None;
    }
    let position = frame.cursor.viewport?;
    let owner = position.x.saturating_sub(u16::from(position.at_wide_tail));
    if usize::from(position.y) != row || usize::from(owner) != column {
        return None;
    }
    Some(if focused {
        frame.cursor.visual_style
    } else {
        CursorVisualStyle::BlockHollow
    })
}

struct CellGlyph {
    origin: Point<Pixels>,
    font_id: FontId,
    id: GlyphId,
    font_size: Pixels,
    color: Hsla,
    is_emoji: bool,
}

impl CellGlyph {
    fn paint(&self, window: &mut Window) {
        let _ = if self.is_emoji {
            window.paint_emoji(self.origin, self.font_id, self.id, self.font_size)
        } else {
            window.paint_glyph(
                self.origin,
                self.font_id,
                self.id,
                self.font_size,
                self.color,
            )
        };
    }
}

/// The grid already owns decorations and clipping. GPUI still owns shaping and
/// glyph rasterization; only its per-cell layer bookkeeping is bypassed.
fn collect_cell_glyphs(
    line: &ShapedLine,
    origin: Point<Pixels>,
    line_height: Pixels,
    color: Hsla,
    glyphs: &mut Vec<CellGlyph>,
) {
    let baseline = origin
        + point(
            px(0.),
            (line_height - line.ascent - line.descent) / 2. + line.ascent,
        );
    for run in &line.runs {
        for glyph in &run.glyphs {
            glyphs.push(CellGlyph {
                origin: baseline + glyph.position,
                font_id: run.font_id,
                id: glyph.id,
                font_size: line.font_size,
                color,
                is_emoji: glyph.is_emoji,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PaintCell, UnderlineRun, cursor_at, device_cell_size, underline_path, underline_runs,
    };
    use gpui_kit::{Path, Pixels, point, px, size};
    use newport_terminal::{CursorVisualStyle, Engine, ScrollViewport, Style, Underline};

    #[test]
    fn terminal_grid_uses_xterm_device_pixel_rounding() {
        let measured = size(px(7.2246094), px(13.96875));
        for (scale, width, height) in [(1., 7., 14.), (2., 7., 14.), (1.5, 10. / 1.5, 21. / 1.5)] {
            let cell = device_cell_size(measured, scale);
            assert_eq!(cell, size(px(width), px(height)));
            assert_eq!((f32::from(cell.width) * scale).fract(), 0.);
            assert_eq!((f32::from(cell.height) * scale).fract(), 0.);
        }
    }
    #[test]
    fn cursor_uses_wide_owner_and_respects_hidden_scrolled_and_unfocused_states() {
        let mut engine = Engine::new(8, 2).unwrap();
        engine.advance("界\x1b[2G\x1b[5 q".as_bytes()).unwrap();
        {
            let frame = engine.frame().unwrap();
            assert_eq!(cursor_at(&frame, 0, 0, true), Some(CursorVisualStyle::Bar));
            assert_eq!(cursor_at(&frame, 0, 1, true), None);
            assert_eq!(
                cursor_at(&frame, 0, 0, false),
                Some(CursorVisualStyle::BlockHollow)
            );
        }
        engine.advance(b"\x1b[?25l").unwrap();
        assert_eq!(cursor_at(&engine.frame().unwrap(), 0, 0, true), None);
        engine.advance(b"\x1b[?25h\r\n\r\n\r\n").unwrap();
        engine.scroll(ScrollViewport::Top);
        let frame = engine.frame().unwrap();
        for row in 0..frame.rows.len() {
            for column in 0..usize::from(frame.columns) {
                assert_eq!(cursor_at(&frame, row, column, true), None);
            }
        }
    }

    fn underline(style: Underline, column: usize, columns: usize) -> UnderlineRun {
        UnderlineRun {
            row: 0,
            column,
            columns,
            style,
            color: 0x336699,
        }
    }

    // Inspect the real GPUI tessellation sent to the renderer rather than a
    // second implementation of dash/curve generation. Sample away from edges.
    fn covered(path: &Path<Pixels>, x: f32, y: f32) -> bool {
        path.vertices.as_chunks::<3>().0.iter().any(|triangle| {
            let points: [_; 3] = std::array::from_fn(|index| {
                let position = triangle[index].xy_position;
                (f32::from(position.x), f32::from(position.y))
            });
            let cross = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
                (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
            };
            if cross(points[0], points[1], points[2]).abs() < 0.00001 {
                return false;
            }
            let values = [
                cross(points[0], points[1], (x, y)),
                cross(points[1], points[2], (x, y)),
                cross(points[2], points[0], (x, y)),
            ];
            values.iter().all(|v| *v >= -0.00001) || values.iter().all(|v| *v <= 0.00001)
        })
    }

    #[test]
    fn tessellated_underline_styles_have_distinct_strokes_gaps_and_curves() {
        let styles = [
            Underline::Single,
            Underline::Double,
            Underline::Curly,
            Underline::Dotted,
            Underline::Dashed,
        ];
        let paths: Vec<_> = styles
            .into_iter()
            .map(|style| {
                underline_path(
                    &underline(style, 0, 1),
                    size(px(7.), px(14.)),
                    point(px(0.), px(0.)),
                    1.,
                )
                .unwrap()
            })
            .collect();
        let masks: Vec<Vec<_>> = paths
            .iter()
            .map(|path| {
                (0..28)
                    .flat_map(|x| {
                        (0..24).map(move |y| {
                            covered(path, x as f32 / 4. + 0.125, 8. + y as f32 / 4. + 0.125)
                        })
                    })
                    .collect()
            })
            .collect();
        for (index, mask) in masks.iter().enumerate() {
            assert!(mask.iter().any(|pixel| *pixel), "{:?}", styles[index]);
            for other in masks.iter().skip(index + 1) {
                assert_ne!(mask, other);
            }
        }
        assert!(covered(&paths[0], 0.25, 12.5));
        assert!(!covered(&paths[0], 0.25, 10.5));
        assert!(covered(&paths[1], 0.25, 10.5));
        assert!(covered(&paths[1], 0.25, 12.5));
        assert!(!covered(&paths[1], 0.25, 11.5));
        assert!(covered(&paths[2], 1.75, 12.5));
        assert!(covered(&paths[2], 5.25, 10.5));
        assert!(!covered(&paths[2], 1.75, 10.5));
        assert!(covered(&paths[3], 0.25, 12.5));
        assert!(!covered(&paths[3], 1.25, 12.5));
        assert!(covered(&paths[4], 2.25, 12.5));
        assert!(!covered(&paths[4], 4.25, 12.5));
        assert!(covered(&paths[4], 6.25, 12.5));
        assert!(
            underline_path(
                &underline(Underline::None, 0, 1),
                size(px(7.), px(14.)),
                point(px(0.), px(0.)),
                1.
            )
            .is_none()
        );
    }

    #[test]
    fn pattern_phase_and_wide_width_survive_color_boundaries_and_display_scales() {
        for scale in [1., 1.5, 2.] {
            let cell = device_cell_size(size(px(7.2246094), px(13.96875)), scale);
            let width = f32::from(cell.width);
            for style in [
                Underline::Single,
                Underline::Double,
                Underline::Curly,
                Underline::Dotted,
                Underline::Dashed,
            ] {
                let origin = point(px(17.), px(23.));
                let wide = underline_path(&underline(style, 1, 2), cell, origin, scale).unwrap();
                let first = underline_path(&underline(style, 1, 1), cell, origin, scale).unwrap();
                let second = underline_path(&underline(style, 2, 1), cell, origin, scale).unwrap();
                for vertex in &wide.vertices {
                    let x = f32::from(vertex.xy_position.x);
                    let y = f32::from(vertex.xy_position.y);
                    assert!(x >= 17. + width - 0.001 && x <= 17. + width * 3. + 0.001);
                    assert!(y >= 23. && y <= 23. + f32::from(cell.height));
                }
                for x in 0..48 {
                    for y in 0..20 {
                        let x = 17. + width * (1. + (x as f32 + 0.37) / 24.);
                        let y = 23. + f32::from(cell.height) - 5. + (y as f32 + 0.19) / 4.;
                        assert_eq!(
                            covered(&wide, x, y),
                            covered(&first, x, y) || covered(&second, x, y),
                            "style={style:?} scale={scale} x={x} y={y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn underline_runs_preserve_color_and_skip_hidden_cells_and_wide_spacers() {
        let cell = |column, wide, spacer, invisible, color| PaintCell {
            row: 0,
            column,
            text: String::new(),
            foreground: 0,
            background: 0,
            underline: color,
            style: Style {
                underline: Underline::Curly,
                invisible,
                ..Default::default()
            },
            wide,
            spacer,
            cursor: None,
        };
        let runs = underline_runs(&[
            cell(0, true, false, false, 0xff0000),
            cell(1, false, true, false, 0xff0000),
            cell(2, false, false, false, 0xff0000),
            cell(3, false, false, true, 0xff0000),
            cell(4, false, false, false, 0x00ff00),
            cell(5, false, false, false, 0x0000ff),
        ]);
        assert_eq!(
            runs,
            [
                UnderlineRun {
                    color: 0xff0000,
                    ..underline(Underline::Curly, 0, 3)
                },
                UnderlineRun {
                    color: 0x00ff00,
                    ..underline(Underline::Curly, 4, 1)
                },
                UnderlineRun {
                    color: 0x0000ff,
                    ..underline(Underline::Curly, 5, 1)
                },
            ]
        );
        let mut engine = Engine::new(30, 2).unwrap();
        engine
            .advance(b"\x1b[4:1ma\x1b[4:2mb\x1b[4:3mc\x1b[4:4md\x1b[4:5me")
            .unwrap();
        assert_eq!(
            engine.frame().unwrap().rows[0].cells[..5]
                .iter()
                .map(|cell| cell.style.underline)
                .collect::<Vec<_>>(),
            [
                Underline::Single,
                Underline::Double,
                Underline::Curly,
                Underline::Dotted,
                Underline::Dashed
            ]
        );
    }
}
