//! A retained, inert page image with selection in the extracted UTF-8 text.
//!
//! Glyph rectangles and the image share one coordinate system. There is no
//! second visible text rendering, no per-character element tree, and no PDF
//! action or annotation handling in this view.
use super::*;
use crate::file_pdf::{Glyph, TextLayer};
use gpui_kit::component::scroll::Scrollbar;
use std::{cell::Cell, ops::Range, rc::Rc, sync::Arc};

pub(crate) struct PdfPageView {
    focus: FocusHandle,
    scroll: ScrollHandle,
    image: Option<Arc<RenderImage>>,
    text: Option<Arc<TextLayer>>,
    geometry: Arc<SelectionGeometry>,
    natural: Size<f32>,
    available_width: f32,
    page_bounds: Rc<Cell<Bounds<Pixels>>>,
    anchor: usize,
    cursor: usize,
    selecting: bool,
    drag_epoch: u64,
    pending_drag_frame: Option<u64>,
}

impl PdfPageView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            image: None,
            text: None,
            geometry: Arc::new(SelectionGeometry::default()),
            natural: size(1., 1.),
            available_width: 0.,
            page_bounds: Rc::new(Cell::new(Bounds::default())),
            anchor: 0,
            cursor: 0,
            selecting: false,
            drag_epoch: 0,
            pending_drag_frame: None,
        }
    }

    /// Reusing the same Arcs preserves selection, focus and scroll on redraw.
    /// A different page's identity resets selection even if its text is equal.
    pub(crate) fn set_page(
        &mut self,
        image: Arc<RenderImage>,
        text: Arc<TextLayer>,
        natural_width: f32,
        natural_height: f32,
        cx: &mut Context<Self>,
    ) {
        let replaced = self
            .image
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &image))
            || self
                .text
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &text));
        let natural = size(
            valid_dimension(natural_width),
            valid_dimension(natural_height),
        );
        let resized = self.natural != natural;
        self.natural = natural;
        if replaced {
            self.anchor = 0;
            self.cursor = 0;
            self.end_drag();
            self.scroll.set_offset(point(px(0.), px(0.)));
            self.page_bounds.set(Bounds::default());
            self.image = Some(image);
            self.geometry = Arc::new(SelectionGeometry::new(&text));
            self.text = Some(text);
        }
        if replaced || resized {
            cx.notify();
        }
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    fn selected_text(&self) -> Option<&str> {
        let range = self.selection();
        (!range.is_empty())
            .then(|| self.text.as_ref()?.text.get(range))
            .flatten()
    }

    fn offset_at(&self, position: Point<Pixels>) -> Option<usize> {
        offset_at(
            self.text.as_ref()?,
            &self.geometry,
            self.page_bounds.get(),
            position,
        )
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.end_drag();
        self.focus.focus(window, cx);
        if self.page_bounds.get().contains(&event.position)
            && let Some(offset) = self.offset_at(event.position)
        {
            if event.click_count >= 2 {
                let text = &self.text.as_ref().expect("hit-tested page text").text;
                let range = if event.click_count == 2 {
                    word_range(text, offset)
                } else {
                    let end = line_end(text, offset);
                    line_start(text, offset)..if end < text.len() { end + 1 } else { end }
                };
                self.anchor = range.start;
                self.cursor = range.end;
                self.selecting = false;
            } else {
                if !event.modifiers.shift {
                    self.anchor = offset;
                }
                self.cursor = offset;
                self.selecting = true;
            }
        } else {
            self.anchor = 0;
            self.cursor = 0;
            self.selecting = false;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag();
            return;
        }
        if let Some(offset) = self.offset_at(event.position) {
            self.cursor = offset;
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            if let Some(offset) = self.offset_at(event.position) {
                self.cursor = offset;
            }
            self.end_drag();
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn end_drag(&mut self) {
        self.selecting = false;
        self.drag_epoch = self.drag_epoch.wrapping_add(1);
        self.pending_drag_frame = None;
    }

    fn drag_speed(&self, pointer: Point<Pixels>) -> f32 {
        let viewport = self.scroll.bounds();
        if !self.selecting
            || viewport.size.height <= px(0.)
            || pointer.x < viewport.left()
            || pointer.x >= viewport.right()
        {
            return 0.;
        }
        let edge = px(24.).min(viewport.size.height / 2.);
        if pointer.y < viewport.top() + edge {
            700. * ((viewport.top() + edge - pointer.y) / edge).clamp(0., 1.)
        } else if pointer.y > viewport.bottom() - edge {
            -700. * ((pointer.y - viewport.bottom() + edge) / edge).clamp(0., 1.)
        } else {
            0.
        }
    }

    /// Reserve one frame at a time; page replacement/release invalidates queued work.
    fn schedule_drag_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if !window.is_window_active() || !self.focus.is_focused(window) {
            self.end_drag();
            return;
        }
        let speed = self.drag_speed(window.mouse_position());
        let offset = self.scroll.offset().y;
        let can_scroll =
            (speed > 0. && offset < px(0.)) || (speed < 0. && offset > -self.scroll.max_offset().y);
        if !can_scroll || self.pending_drag_frame.is_some() {
            return;
        }
        let epoch = self.drag_epoch;
        self.pending_drag_frame = Some(epoch);
        let view = cx.weak_entity();
        let started = std::time::Instant::now();
        window.on_next_frame(move |window, cx| {
            let _ = view.update(cx, |view, cx| {
                if view.pending_drag_frame != Some(epoch) || view.drag_epoch != epoch {
                    return;
                }
                view.pending_drag_frame = None;
                if !window.is_window_active() || !view.focus.is_focused(window) {
                    view.end_drag();
                    return;
                }
                let pointer = window.mouse_position();
                let before = view.scroll.offset();
                view.scroll_by(px(
                    view.drag_speed(pointer) * started.elapsed().as_secs_f32().min(0.05)
                ));
                let delta = view.scroll.offset() - before;
                if delta.y != px(0.) {
                    // Hit-test the newly exposed text, even before the next layout.
                    let mut bounds = view.page_bounds.get();
                    bounds.origin += delta;
                    view.page_bounds.set(bounds);
                    if let Some(offset) = view.offset_at(pointer) {
                        view.cursor = offset;
                    }
                    cx.notify();
                }
            });
        });
    }

    fn scroll_by(&self, dy: Pixels) {
        let mut offset = self.scroll.offset();
        offset.y = (offset.y + dy).clamp(-self.scroll.max_offset().y, px(0.));
        self.scroll.set_offset(offset);
    }

    fn reveal_cursor(&self) {
        let Some(layer) = self.text.as_ref() else {
            return;
        };
        let Some(glyph) = layer
            .glyphs
            .iter()
            .find(|glyph| glyph.range.contains(&self.cursor) || glyph.range.end == self.cursor)
        else {
            return;
        };
        let bounds = glyph_bounds(self.page_bounds.get(), glyph);
        let viewport = self.scroll.bounds();
        if bounds.top() < viewport.top() + px(12.) {
            self.scroll_by(viewport.top() + px(12.) - bounds.top());
        } else if bounds.bottom() > viewport.bottom() - px(12.) {
            self.scroll_by(viewport.bottom() - px(12.) - bounds.bottom());
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let key = &event.keystroke;
        let command =
            key.modifiers.platform || (!cfg!(target_os = "macos") && key.modifiers.control);
        if command && key.key == "c" {
            if let Some(text) = self.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            }
        } else if command && key.key == "a" {
            self.anchor = 0;
            self.cursor = self.text.as_ref().map_or(0, |layer| layer.text.len());
        } else if key.key == "pageup" || key.key == "pagedown" || key.key == "space" {
            let up = key.key == "pageup" || (key.key == "space" && key.modifiers.shift);
            self.scroll_by(self.scroll.bounds().size.height * if up { 0.9 } else { -0.9 });
        } else if matches!(
            key.key.as_str(),
            "left" | "right" | "up" | "down" | "home" | "end"
        ) {
            let Some(layer) = self.text.as_ref() else {
                return;
            };
            let text = &layer.text;
            let backward = matches!(key.key.as_str(), "left" | "up" | "home");
            let range = self.selection();
            let next = if !key.modifiers.shift
                && !range.is_empty()
                && matches!(key.key.as_str(), "left" | "right")
                && !command
            {
                if backward { range.start } else { range.end }
            } else {
                match key.key.as_str() {
                    "left" if command => line_start(text, self.cursor),
                    "right" if command => line_end(text, self.cursor),
                    "left" => previous_char(text, self.cursor),
                    "right" => next_char(text, self.cursor),
                    "up" if command => 0,
                    "down" if command => text.len(),
                    "up" | "down" => vertical_offset(layer, self.cursor, backward),
                    "home" => {
                        if command {
                            0
                        } else {
                            line_start(text, self.cursor)
                        }
                    }
                    "end" => {
                        if command {
                            text.len()
                        } else {
                            line_end(text, self.cursor)
                        }
                    }
                    _ => self.cursor,
                }
            };
            self.cursor = next;
            if !key.modifiers.shift {
                self.anchor = next;
            }
            self.reveal_cursor();
        } else {
            return;
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
}

fn valid_dimension(value: f32) -> f32 {
    if value.is_finite() && value > 0. {
        value
    } else {
        1.
    }
}

fn page_size(natural: Size<f32>, available_width: f32) -> Size<Pixels> {
    let scale = 1.5_f32
        .min(available_width.max(1.) / natural.width)
        .min(1600. / natural.height);
    size(px(natural.width * scale), px(natural.height * scale))
}

fn glyph_bounds(page: Bounds<Pixels>, glyph: &Glyph) -> Bounds<Pixels> {
    rect_bounds(page, glyph.bounds)
}

fn rect_bounds(page: Bounds<Pixels>, rect: [f32; 4]) -> Bounds<Pixels> {
    let [x, y, width, height] = rect;
    Bounds::new(
        page.origin + point(page.size.width * x, page.size.height * y),
        size(page.size.width * width, page.size.height * height),
    )
}

#[derive(Clone, Copy, Default)]
enum Direction {
    #[default]
    Right,
    Down,
    Left,
    Up,
}

impl Direction {
    fn vertical(self) -> bool {
        matches!(self, Self::Down | Self::Up)
    }

    fn fraction(self, bounds: Bounds<Pixels>, position: Point<Pixels>) -> f32 {
        let (distance, extent) = match self {
            Self::Right => (position.x - bounds.left(), bounds.size.width),
            Self::Left => (bounds.right() - position.x, bounds.size.width),
            Self::Down => (position.y - bounds.top(), bounds.size.height),
            Self::Up => (bounds.bottom() - position.y, bounds.size.height),
        };
        (f32::from(distance) / f32::from(extent).max(0.01)).clamp(0., 1.)
    }

    fn slice(self, bounds: &mut Bounds<Pixels>, start: f32, end: f32) {
        match self {
            Self::Right => bounds.origin.x += bounds.size.width * start,
            Self::Left => bounds.origin.x += bounds.size.width * (1. - end),
            Self::Down => bounds.origin.y += bounds.size.height * start,
            Self::Up => bounds.origin.y += bounds.size.height * (1. - end),
        }
        if self.vertical() {
            bounds.size.height *= end - start;
        } else {
            bounds.size.width *= end - start;
        }
    }
}

struct SelectionSpan {
    range: Range<usize>,
    bounds: [f32; 4],
    direction: Direction,
}

#[derive(Default)]
struct SelectionGeometry {
    spans: Vec<SelectionSpan>,
}

impl SelectionGeometry {
    fn new(layer: &TextLayer) -> Self {
        let mut spans: Vec<SelectionSpan> = Vec::new();
        for glyph in &layer.glyphs {
            if glyph.range.is_empty()
                || layer.text.get(glyph.range.clone()).is_none()
                || !glyph.bounds.iter().all(|n| n.is_finite())
                || glyph.bounds[2] <= 0.
                || glyph.bounds[3] <= 0.
            {
                continue;
            }
            // Expanded ligatures can have several scalars sharing one box.
            // Group them so every scalar remains reachable by the pointer.
            if let Some(previous) = spans.last_mut()
                && previous.range.end == glyph.range.start
                && previous.bounds == glyph.bounds
            {
                previous.range.end = glyph.range.end;
                continue;
            }
            spans.push(SelectionSpan {
                range: glyph.range.clone(),
                bounds: glyph.bounds,
                direction: Direction::Right,
            });
        }
        for index in 0..spans.len() {
            let same_line = |a: usize, b: usize| {
                layer
                    .text
                    .get(spans[a].range.end..spans[b].range.start)
                    .is_some_and(|gap| !gap.contains(['\n', '\r']))
            };
            let pair = if index + 1 < spans.len() && same_line(index, index + 1) {
                Some((index, index + 1))
            } else if index > 0 && same_line(index - 1, index) {
                Some((index - 1, index))
            } else {
                None
            };
            if let Some((a, b)) = pair {
                let a = spans[a].bounds;
                let b = spans[b].bounds;
                let dx = b[0] + b[2] * 0.5 - a[0] - a[2] * 0.5;
                let dy = b[1] + b[3] * 0.5 - a[1] - a[3] * 0.5;
                spans[index].direction = if dy.abs() > dx.abs() {
                    if dy > 0. {
                        Direction::Down
                    } else {
                        Direction::Up
                    }
                } else if dx < 0. {
                    Direction::Left
                } else {
                    Direction::Right
                };
            }
        }
        Self { spans }
    }
}

fn offset_at(
    layer: &TextLayer,
    geometry: &SelectionGeometry,
    page: Bounds<Pixels>,
    position: Point<Pixels>,
) -> Option<usize> {
    if page.size.width <= px(0.) || page.size.height <= px(0.) {
        return None;
    }
    let distance = |glyph: &SelectionSpan| {
        let bounds = rect_bounds(page, glyph.bounds);
        let dy = (bounds.top() - position.y)
            .max(position.y - bounds.bottom())
            .max(px(0.));
        let dx = (bounds.left() - position.x)
            .max(position.x - bounds.right())
            .max(px(0.));
        // Prefer the visual line, including rotated pages and vertical writing.
        if glyph.direction.vertical() {
            f32::from(dx) * 1_000_000. + f32::from(dy)
        } else {
            f32::from(dy) * 1_000_000. + f32::from(dx)
        }
    };
    let glyph = geometry
        .spans
        .iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
    let bounds = rect_bounds(page, glyph.bounds);
    // Extractors may return a ligature or a multi-scalar span as one glyph.
    // Split its insertion positions by scalars, never by UTF-8 bytes.
    let text = &layer.text[glyph.range.clone()];
    let count = text.chars().count();
    let fraction = glyph.direction.fraction(bounds, position);
    let index = (fraction * count as f32).round() as usize;
    Some(
        glyph.range.start
            + text
                .char_indices()
                .nth(index)
                .map_or(text.len(), |(at, _)| at),
    )
}

fn previous_char(text: &str, at: usize) -> usize {
    text[..at]
        .char_indices()
        .next_back()
        .map_or(0, |(at, _)| at)
}

fn next_char(text: &str, at: usize) -> usize {
    at + text[at..].chars().next().map_or(0, char::len_utf8)
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |at| at + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..]
        .find('\n')
        .map_or(text.len(), |offset| at + offset)
}

fn word_range(text: &str, at: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let at = if at == text.len() {
        previous_char(text, at)
    } else {
        at
    };
    let class = |ch: char| {
        if ch.is_whitespace() {
            0
        } else if ch.is_alphanumeric()
            || ch == '_'
            || matches!(ch, '\u{0300}'..='\u{036f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{20d0}'..='\u{20ff}' | '\u{fe20}'..='\u{fe2f}')
        {
            1
        } else {
            2
        }
    };
    let kind = class(text[at..].chars().next().expect("character boundary"));
    let start = text[..at]
        .char_indices()
        .rev()
        .find(|(_, ch)| class(*ch) != kind)
        .map_or(0, |(at, ch)| at + ch.len_utf8());
    let end = text[at..]
        .char_indices()
        .find(|(_, ch)| class(*ch) != kind)
        .map_or(text.len(), |(offset, _)| at + offset);
    start..end
}

fn vertical_offset(layer: &TextLayer, at: usize, up: bool) -> usize {
    let Some(current) = layer
        .glyphs
        .iter()
        .find(|glyph| glyph.range.contains(&at))
        .or_else(|| {
            layer
                .glyphs
                .iter()
                .rev()
                .find(|glyph| glyph.range.end <= at)
        })
    else {
        return at;
    };
    let [x, y, _, h] = current.bounds;
    let target = layer
        .glyphs
        .iter()
        .filter(|glyph| {
            if up {
                glyph.bounds[1] + glyph.bounds[3] * 0.5 < y
            } else {
                glyph.bounds[1] + glyph.bounds[3] * 0.5 > y + h
            }
        })
        .min_by(|a, b| {
            let distance =
                |glyph: &Glyph| (glyph.bounds[1] - y).abs() * 1000. + (glyph.bounds[0] - x).abs();
            distance(a).total_cmp(&distance(b))
        });
    target.map_or(if up { 0 } else { layer.text.len() }, |glyph| {
        glyph.range.start
    })
}

/// The iterator filters before painting, so offscreen selection costs no quads.
fn highlight_bounds<'a>(
    layer: &'a TextLayer,
    geometry: &'a SelectionGeometry,
    selected: Range<usize>,
    page: Bounds<Pixels>,
    visible: Bounds<Pixels>,
) -> impl Iterator<Item = Bounds<Pixels>> + 'a {
    geometry.spans.iter().filter_map(move |glyph| {
        if selected.start >= glyph.range.end || selected.end <= glyph.range.start {
            return None;
        }
        let mut bounds = rect_bounds(page, glyph.bounds);
        // Keep partial selection of multi-scalar glyphs aligned with hit testing.
        if let Some(text) = layer.text.get(glyph.range.clone()) {
            let count = text.chars().count().max(1) as f32;
            let start = selected
                .start
                .saturating_sub(glyph.range.start)
                .min(text.len());
            let end = selected
                .end
                .saturating_sub(glyph.range.start)
                .min(text.len());
            if let (Some(before), Some(through)) = (text.get(..start), text.get(..end)) {
                let left = before.chars().count() as f32 / count;
                let right = through.chars().count() as f32 / count;
                glyph.direction.slice(&mut bounds, left, right);
            }
        }
        (bounds.intersects(&visible) && bounds.size.width > px(0.) && bounds.size.height > px(0.))
            .then_some(bounds)
    })
}

impl Render for PdfPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let padding = if window.viewport_size().width <= px(800.) {
            12.
        } else {
            24.
        };
        let available = if self.available_width > 0. {
            self.available_width
        } else {
            300.
        };
        let dimensions = page_size(self.natural, available - padding * 2.);
        let measure = cx.entity().downgrade();
        let measured_width = self.available_width;
        let page_bounds = self.page_bounds.clone();
        let text = self.text.clone();
        let geometry = self.geometry.clone();
        let accessible = self.text.clone();
        let selected = self.selection();
        let selection_color = cx.theme().ring.opacity(0.3);
        let ring = cx.theme().ring;
        let drag = cx.weak_entity();

        div()
            .id("pdf-page-view")
            .role(Role::Document)
            .aria_label("PDF page content")
            .a11y_synthetic_children(move |builder| {
                builder.parent_node().set_read_only();
                if let Some(layer) = accessible {
                    builder.parent_node().set_value(layer.text.clone());
                    if !layer.text.is_empty() {
                        let mut node = accesskit::Node::new(Role::TextRun);
                        node.set_value(layer.text.clone());
                        node.set_character_lengths(
                            layer
                                .text
                                .chars()
                                .map(|c| c.len_utf8() as u8)
                                .collect::<Vec<_>>(),
                        );
                        let id = builder.synthetic_node_id("pdf-page-text");
                        builder.push_child(id, node);
                    }
                }
            })
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .size_full()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(cx.theme().muted)
            .child(
                div()
                    .id("pdf-page-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(px(padding))
                    .child(
                        div().w_full().flex().justify_center().child(
                            div()
                                .id("pdf-sheet")
                                .relative()
                                .flex_shrink_0()
                                .w(dimensions.width)
                                .h(dimensions.height)
                                .bg(rgb(0xffffff))
                                .when(
                                    self.text
                                        .as_ref()
                                        .is_some_and(|layer| !layer.glyphs.is_empty()),
                                    |sheet| sheet.cursor_text(),
                                )
                                .shadow(vec![BoxShadow {
                                    color: rgba(0x00000018).into(),
                                    offset: point(px(0.), px(2.)),
                                    blur_radius: px(8.),
                                    spread_radius: px(0.),
                                    inset: false,
                                }])
                                .children(self.image.clone().map(|image| img(image).size_full()))
                                .child(
                                    canvas(
                                        move |bounds, _, _| page_bounds.set(bounds),
                                        move |bounds, _, window, _| {
                                            if let Some(layer) = &text {
                                                for rect in highlight_bounds(
                                                    layer,
                                                    &geometry,
                                                    selected,
                                                    bounds,
                                                    window.content_mask().bounds,
                                                ) {
                                                    window.paint_quad(fill(rect, selection_color));
                                                }
                                            }
                                        },
                                    )
                                    .absolute()
                                    .size_full()
                                    .top_0()
                                    .left_0(),
                                ),
                        ),
                    ),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let width = f32::from(bounds.size.width);
                        if (measured_width - width).abs() <= 0.1 {
                            return;
                        }
                        window.defer(cx, move |_, cx| {
                            let _ = measure.update(cx, |view, cx| {
                                if (view.available_width - width).abs() > 0.1 {
                                    view.available_width = width;
                                    cx.notify();
                                }
                            });
                        });
                    },
                    move |_, _, window, cx| {
                        // Capture movement/release outside the page as well as inside it.
                        let moved = drag.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ =
                                    moved.update(cx, |view, cx| view.mouse_move(event, window, cx));
                            }
                        });
                        let released = drag.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                let _ = released
                                    .update(cx, |view, cx| view.mouse_up(event, window, cx));
                            }
                        });
                        let _ = drag.update(cx, |view, cx| view.schedule_drag_frame(window, cx));
                    },
                )
                .absolute()
                .size_full()
                .top_0()
                .left_0(),
            )
            .child(Scrollbar::vertical(&self.scroll).id("pdf-page-scrollbar"))
            .focus_visible(move |style| {
                style.shadow(vec![BoxShadow {
                    color: ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: true,
                }])
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{PdfPageView, SelectionGeometry, glyph_bounds, highlight_bounds, page_size};
    use crate::file_pdf::{Glyph, TextLayer};
    use gpui::{
        AppContext, Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, Pixels, Point,
        RenderImage, TestAppContext, VisualTestContext, point, px, size,
    };
    use gpui_kit as gpui;
    use std::sync::Arc;

    fn layer(text: &str) -> Arc<TextLayer> {
        let mut glyphs = Vec::new();
        let mut row = 0;
        let mut column = 0;
        for (at, ch) in text.char_indices() {
            if ch == '\n' {
                row += 1;
                column = 0;
            } else {
                glyphs.push(Glyph {
                    range: at..at + ch.len_utf8(),
                    bounds: [
                        0.1 + column as f32 * 0.025,
                        0.1 + row as f32 * 0.05,
                        0.025,
                        0.035,
                    ],
                });
                column += 1;
            }
        }
        Arc::new(TextLayer {
            text: text.into(),
            glyphs,
        })
    }

    fn image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_pixel(8, 12, image::Rgba([255; 4])),
        )]))
    }

    fn fixture<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<PdfPageView>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|cx| {
                let mut view = PdfPageView::new(cx);
                view.set_page(image(), layer(text), 400., 600., cx);
                view
            });
            view = Some(content.clone());
            gpui::base::Root::new(content, window, cx)
        });
        cx.simulate_resize(size(px(600.), px(300.)));
        cx.update(|window, _| window.activate_window());
        draw(cx);
        (view.unwrap(), cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    fn at(view: &Entity<PdfPageView>, offset: usize, cx: &mut VisualTestContext) -> Point<Pixels> {
        view.read_with(cx, |view, _| {
            let text = view.text.as_ref().unwrap();
            let glyph = text
                .glyphs
                .iter()
                .find(|glyph| glyph.range.start == offset)
                .or_else(|| text.glyphs.iter().find(|glyph| glyph.range.end == offset))
                .unwrap();
            let bounds = glyph_bounds(view.page_bounds.get(), glyph);
            point(
                if glyph.range.start == offset {
                    bounds.left() + px(0.1)
                } else {
                    bounds.right() - px(0.1)
                },
                bounds.top() + bounds.size.height * 0.5,
            )
        })
    }

    fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        draw(cx);
    }

    fn copy(cx: &mut VisualTestContext) -> String {
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        cx.update(|_, cx| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
        })
    }

    #[cfg(any(target_os = "macos", windows))]
    #[gpui::test]
    fn native_pdf_pixels_and_text_support_pointer_selection(cx: &mut TestAppContext) {
        let page = crate::file_pdf::render(
            Arc::new(
                include_bytes!("../../newport-platform/tests/fixtures/pdf/plain.pdf").to_vec(),
            ),
            1,
        )
        .unwrap();
        let (view, cx) = fixture(cx, "");
        view.update(cx, |view, cx| {
            view.set_page(
                page.image,
                page.text,
                page.data["naturalWidth"].as_f64().unwrap() as f32,
                page.data["naturalHeight"].as_f64().unwrap() as f32,
                cx,
            )
        });
        draw(cx);
        let start = at(&view, 0, cx);
        let end = at(&view, 7, cx);
        drag(cx, start, end);
        assert_eq!(copy(cx), "Newport");
    }

    fn drag_frame(cx: &mut VisualTestContext) {
        std::thread::sleep(std::time::Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        draw(cx);
    }

    #[gpui::test]
    fn stationary_edge_drag_extends_and_retracts_then_stops_at_boundary(cx: &mut TestAppContext) {
        let text = (0..16)
            .map(|i| format!("line {i:02}\n"))
            .collect::<String>();
        let (view, cx) = fixture(cx, &text);
        cx.update(|window, _| window.activate_window());
        let from = at(&view, 0, cx);
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        draw(cx);
        let bottom = view.read_with(cx, |view, _| view.scroll.bounds().bottom());
        let below = point(from.x, bottom + px(20.));
        cx.simulate_mouse_move(below, Some(MouseButton::Left), Modifiers::default());
        draw(cx);
        let initial = view.read_with(cx, |view, _| view.cursor);
        for _ in 0..25 {
            drag_frame(cx);
        }
        view.read_with(cx, |view, _| {
            assert!(
                view.scroll.offset().y < px(-200.),
                "offset {:?}, selecting {}, pending {:?}, cursor {}",
                view.scroll.offset(),
                view.selecting,
                view.pending_drag_frame,
                view.cursor
            );
            assert!(
                view.cursor > initial,
                "stationary pointer must reach new text"
            );
            assert_eq!(view.anchor, 0);
        });
        let top = view.read_with(cx, |view, _| view.scroll.bounds().top());
        cx.simulate_mouse_move(
            point(from.x, top - px(20.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        draw(cx);
        for _ in 0..35 {
            drag_frame(cx);
        }
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, px(0.));
            assert_eq!(view.cursor, 0);
            assert!(
                view.pending_drag_frame.is_none(),
                "no animation at the boundary"
            );
        });
        cx.simulate_mouse_up(
            point(from.x, top - px(20.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        draw(cx);
        assert!(!view.read_with(cx, |view, _| view.selecting));
    }

    #[gpui::test]
    fn queued_drag_cannot_survive_release_focus_loss_or_page_replacement(cx: &mut TestAppContext) {
        let text = (0..16)
            .map(|i| format!("line {i:02}\n"))
            .collect::<String>();
        let (view, cx) = fixture(cx, &text);
        cx.update(|window, _| window.activate_window());
        for stop in ["release", "missed release", "focus", "page"] {
            view.update(cx, |view, cx| {
                view.set_page(image(), layer(&text), 400., 600., cx)
            });
            draw(cx);
            let from = at(&view, 0, cx);
            cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            draw(cx);
            let bottom = view.read_with(cx, |view, _| view.scroll.bounds().bottom());
            let below = point(from.x, bottom + px(20.));
            cx.simulate_mouse_move(below, Some(MouseButton::Left), Modifiers::default());
            draw(cx);
            assert!(view.read_with(cx, |view, _| view.pending_drag_frame.is_some()));
            match stop {
                "release" => cx.simulate_mouse_up(
                    point(px(900.), below.y),
                    MouseButton::Left,
                    Modifiers::default(),
                ),
                "missed release" => cx.simulate_mouse_move(below, None, Modifiers::default()),
                "focus" => cx.update(|window, cx| window.blur(cx)),
                _ => view.update(cx, |view, cx| {
                    view.set_page(image(), layer("new page"), 400., 600., cx)
                }),
            }
            let before = view.read_with(cx, |view, _| (view.scroll.offset(), view.selection()));
            for _ in 0..3 {
                drag_frame(cx);
            }
            view.read_with(cx, |view, _| {
                assert_eq!((view.scroll.offset(), view.selection()), before, "{stop}");
                assert!(!view.selecting, "{stop}");
                assert!(view.pending_drag_frame.is_none(), "{stop}");
            });
        }
    }

    #[gpui::test]
    fn rendered_drag_copies_literal_unicode_and_reverse_selection(cx: &mut TestAppContext) {
        let text = "Café **literal** <tag>\n服务 👩🏽‍💻 e\u{301} tail";
        let (view, cx) = fixture(cx, text);
        let start = text.find("é").unwrap();
        let end = text.find(" tail").unwrap();
        let from = at(&view, start, cx);
        let to = at(&view, end, cx);
        drag(cx, from, to);
        assert_eq!(copy(cx), &text[start..end]);
        drag(cx, to, from);
        assert_eq!(copy(cx), &text[start..end]);
    }

    #[gpui::test]
    fn keyboard_selection_is_utf8_safe_and_page_is_read_only(cx: &mut TestAppContext) {
        let text = "Aé服务🦀\nsecond";
        let (view, cx) = fixture(cx, text);
        let from = at(&view, 0, cx);
        cx.simulate_click(from, Modifiers::default());
        cx.simulate_keystrokes("shift-right shift-right shift-right");
        assert_eq!(copy(cx), "Aé服");
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        });
        assert_eq!(copy(cx), text);
        cx.simulate_keystrokes("backspace x");
        assert_eq!(copy(cx), text);
        view.read_with(cx, |view, _| {
            assert_eq!(view.text.as_ref().unwrap().text, text)
        });
    }

    #[gpui::test]
    fn resize_and_redraw_retain_selection_but_new_page_resets(cx: &mut TestAppContext) {
        let text = "first\nsecond";
        let (view, cx) = fixture(cx, text);
        let from = at(&view, 0, cx);
        let to = at(&view, 4, cx);
        drag(cx, from, to);
        let before = view.read_with(cx, |view, _| view.page_bounds.get());
        cx.simulate_resize(size(px(400.), px(300.)));
        draw(cx);
        assert_eq!(copy(cx), "firs");
        view.read_with(cx, |view, _| {
            let after = view.page_bounds.get();
            assert!(after.size.width < before.size.width);
            assert!((after.size.height / after.size.width - 1.5).abs() < 0.01);
            let selected: Vec<_> = highlight_bounds(
                view.text.as_ref().unwrap(),
                &view.geometry,
                view.selection(),
                after,
                after,
            )
            .collect();
            assert_eq!(selected.len(), 4);
            assert_eq!(
                selected[0],
                glyph_bounds(after, &view.text.as_ref().unwrap().glyphs[0])
            );
        });
        view.update(cx, |view, cx| {
            view.scroll.set_offset(point(px(0.), px(-80.)));
            view.set_page(
                view.image.clone().unwrap(),
                view.text.clone().unwrap(),
                400.,
                600.,
                cx,
            );
            assert_eq!(view.selected_text(), Some("firs"));
            assert_eq!(view.scroll.offset().y, px(-80.));
            view.set_page(image(), layer(text), 400., 600., cx);
            assert!(view.selected_text().is_none());
            assert_eq!(view.scroll.offset(), point(px(0.), px(0.)));
        });
        draw(cx);
        cx.update(|window, cx| assert!(view.read(cx).focus.is_focused(window)));
    }

    #[gpui::test]
    fn scanned_page_has_no_selection_and_keyboard_scroll_stays_in_page(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, "");
        let position = view.read_with(cx, |view, _| {
            view.page_bounds.get().origin + point(px(10.), px(10.))
        });
        drag(cx, position, position + point(px(70.), px(50.)));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        });
        view.read_with(cx, |view, _| assert!(view.selected_text().is_none()));
        cx.simulate_keystrokes("pagedown");
        draw(cx);
        view.read_with(cx, |view, _| assert!(view.scroll.offset().y < px(0.)));
    }

    #[gpui::test]
    fn rotated_pages_and_reverse_reading_hit_the_correct_insertion_edges(cx: &mut TestAppContext) {
        let text = "Aé服务🦀Z";
        let (view, cx) = fixture(cx, text);
        cx.simulate_resize(size(px(600.), px(1000.)));
        for rotation in [90, 180, 270] {
            let mut rotated = (*layer(text)).clone();
            for glyph in &mut rotated.glyphs {
                let [x, y, w, h] = glyph.bounds;
                glyph.bounds = match rotation {
                    90 => [1. - y - h, x, h, w],
                    180 => [1. - x - w, 1. - y - h, w, h],
                    _ => [y, 1. - x - w, h, w],
                };
            }
            view.update(cx, |view, cx| {
                view.set_page(image(), Arc::new(rotated), 400., 600., cx)
            });
            draw(cx);
            let insertion = |offset, cx: &mut VisualTestContext| {
                view.read_with(cx, |view, _| {
                    let glyph = view
                        .text
                        .as_ref()
                        .unwrap()
                        .glyphs
                        .iter()
                        .find(|g| g.range.start == offset)
                        .unwrap();
                    let bounds = glyph_bounds(view.page_bounds.get(), glyph);
                    match rotation {
                        90 => point(
                            bounds.left() + bounds.size.width * 0.5,
                            bounds.top() + px(0.1),
                        ),
                        180 => point(
                            bounds.right() - px(0.1),
                            bounds.top() + bounds.size.height * 0.5,
                        ),
                        _ => point(
                            bounds.left() + bounds.size.width * 0.5,
                            bounds.bottom() - px(0.1),
                        ),
                    }
                })
            };
            let from = insertion(1, cx);
            let to = insertion(text.find('Z').unwrap(), cx);
            drag(cx, from, to);
            assert_eq!(copy(cx), "é服务🦀", "rotation {rotation}");
            drag(cx, to, from);
            assert_eq!(copy(cx), "é服务🦀", "reverse rotation {rotation}");
        }
    }

    #[gpui::test]
    fn expanded_ligature_sharing_one_box_remains_individually_selectable(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, "office");
        let mut ligature = (*layer("office")).clone();
        for glyph in &mut ligature.glyphs[1..4] {
            glyph.bounds = [0.125, 0.1, 0.075, 0.035];
        }
        view.update(cx, |view, cx| {
            view.set_page(image(), Arc::new(ligature), 400., 600., cx)
        });
        draw(cx);
        let (from, to) = view.read_with(cx, |view, _| {
            let bounds = glyph_bounds(
                view.page_bounds.get(),
                &view.text.as_ref().unwrap().glyphs[1],
            );
            let y = bounds.top() + bounds.size.height * 0.5;
            (
                point(bounds.left() + bounds.size.width / 3., y),
                point(bounds.right() - px(0.1), y),
            )
        });
        drag(cx, from, to);
        assert_eq!(copy(cx), "fi");
        view.read_with(cx, |view, _| {
            assert_eq!(view.geometry.spans.len(), 4);
            let page = view.page_bounds.get();
            let highlights: Vec<_> = highlight_bounds(
                view.text.as_ref().unwrap(),
                &view.geometry,
                view.selection(),
                page,
                page,
            )
            .collect();
            assert_eq!(highlights.len(), 1, "shared ligature box paints only once");
            let expected = glyph_bounds(page, &view.text.as_ref().unwrap().glyphs[1])
                .size
                .width
                * (2. / 3.);
            assert!((f32::from(highlights[0].size.width - expected)).abs() < 0.001);
        });
    }

    #[gpui::test]
    fn double_click_selects_unicode_word_and_triple_click_selects_literal_line(
        cx: &mut TestAppContext,
    ) {
        let text = "one Cafe\u{301} 服务\nnext line";
        let (view, cx) = fixture(cx, text);
        let position = at(&view, text.find('f').unwrap(), cx);
        for (click_count, expected) in [(2, "Cafe\u{301}"), (3, "one Cafe\u{301} 服务\n")] {
            cx.simulate_event(MouseDownEvent {
                position,
                modifiers: Modifiers::default(),
                button: MouseButton::Left,
                click_count,
                first_mouse: false,
            });
            cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
            draw(cx);
            assert_eq!(copy(cx), expected);
        }
    }

    #[test]
    fn page_scale_and_visible_highlights_are_bounded() {
        assert_eq!(page_size(size(400., 600.), 2000.), size(px(600.), px(900.)));
        let tall = page_size(size(400., 6000.), 2000.);
        assert!((f32::from(tall.width) - 1600. / 15.).abs() < 0.001);
        assert!((f32::from(tall.height) - 1600.).abs() < 0.001);
        let text = layer("line one\nline two\nline three");
        let page = Bounds::new(point(px(30.), px(-300.)), size(px(400.), px(600.)));
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(200.)));
        assert_eq!(
            highlight_bounds(
                &text,
                &SelectionGeometry::new(&text),
                0..text.text.len(),
                page,
                visible
            )
            .count(),
            0
        );
    }
}
