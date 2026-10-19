//! Literal, virtualized log text using Kit's window selection coordinator.
//!
//! Only visible glyph layouts are retained. Endpoints are UTF-8 document offsets,
//! so selection/copy does not depend on whether its rows are currently mounted.
use gpui::base::{
    SelectableText, TextSelection, TextSelectionContentKey, TextSelectionCoverage,
    TextSelectionHandle, TextSelectionRegistration, TextSelectionRun, TextSelectionSnapshot,
};
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, FocusHandle, GlobalElementId, HighlightStyle,
    Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point,
    SharedString, StyledText, Subscription, TextLayout, Window, WindowId,
};
use gpui_kit as gpui;
use std::{cell::RefCell, ops::Range, rc::Rc};
#[path = "selectable_log_tabs.rs"]
mod tabs;
pub(crate) use tabs::TabText;

#[derive(Default)]
struct Document {
    text: String,
    rows: Vec<Option<Range<usize>>>,
    selected: Option<Range<usize>>,
    bounds: Bounds<Pixels>,
    runs: Vec<Run>,
}
struct Run {
    range: Range<usize>,
    layout: TextLayout,
    bounds: Bounds<Pixels>,
    tabs: Option<Rc<TabText>>,
}
impl Document {
    fn offset_at(&self, point: Point<Pixels>) -> Option<usize> {
        let point = point + self.bounds.origin;
        // Prefer the correct wrapped line, then its nearest column. This also
        // treats the whitespace between timestamp and message as a text gap.
        let run = self
            .runs
            .iter()
            .min_by(|a, b| distance(a.bounds, point).total_cmp(&distance(b.bounds, point)))?;
        let local = run.layout.index_for_position(point).unwrap_or_else(|at| at);
        let local = run
            .tabs
            .as_ref()
            .map_or(local, |tabs| tabs.source_offset(local));
        let mut offset = run.range.start + local.min(run.range.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        Some(offset)
    }
    fn selected_text(&self) -> String {
        self.selected
            .as_ref()
            .and_then(|range| self.text.get(range.clone()))
            .unwrap_or_default()
            .to_owned()
    }
}
fn distance(bounds: Bounds<Pixels>, point: Point<Pixels>) -> f32 {
    let dx = (bounds.left() - point.x)
        .max(point.x - bounds.right())
        .max(gpui::px(0.));
    let dy = (bounds.top() - point.y)
        .max(point.y - bounds.bottom())
        .max(gpui::px(0.));
    // A timestamp is a separate run, but belongs to the message's first line.
    // A wrapped message line below it must never resolve into the timestamp.
    f32::from(dy) * 1_000_000. + f32::from(dx)
}

/// Retain one instance alongside the list, not one instance per mounted row.
pub(crate) struct SelectableLog {
    document: Rc<RefCell<Document>>,
    handle: TextSelectionHandle,
    window: WindowId,
    _subscription: Subscription,
}
impl SelectableLog {
    pub(crate) fn new(focus: FocusHandle, window: &Window, cx: &mut App) -> Self {
        let document = Rc::new(RefCell::new(Document::default()));
        let handle = TextSelectionHandle::new("", cx);
        let id = handle.entity_id();
        let window_handle = window.window_handle();
        let state = document.clone();
        let subscription = handle.subscribe(
            move |event, cx| {
                use gpui::base::TextSelectionEvent;
                match event {
                    TextSelectionEvent::SelectionChanged(snapshot) => {
                        let mut state = state.borrow_mut();
                        match snapshot {
                            None => state.selected = None,
                            Some(snapshot) => {
                                // Base publishes geometry first, then resolves stable
                                // content keys outside its entity update. Keep the last
                                // range during that brief intermediate publication.
                                if let Some(range) = selected_range(*snapshot, id, state.text.len())
                                {
                                    state.selected = Some(range);
                                }
                            }
                        }
                        drop(state);
                        let _ = window_handle.update(cx, |_, window, _| window.refresh());
                    }
                    TextSelectionEvent::Cleared => state.borrow_mut().selected = None,
                    _ => {}
                }
            },
            cx,
        );
        let state = document.clone();
        handle.resolve_content_key_with(
            move |point, _| {
                state
                    .borrow()
                    .offset_at(point)
                    .map(|at| TextSelectionContentKey::new(at as u64))
            },
            cx,
        );
        let state = document.clone();
        handle.copy_with(move |_| state.borrow().selected_text(), cx);
        handle.focus_with(move |window, cx| focus.focus(window, cx), cx);
        Self {
            document,
            handle,
            window: window.window_handle().window_id(),
            _subscription: subscription,
        }
    }

    /// `None` rows are decorative separators. Each text row is copied literally,
    /// separated by one newline. Pure appends retain selection; replacement and
    /// prefix eviction clear it rather than silently selecting different content.
    pub(crate) fn set_rows(&mut self, rows: Vec<Option<String>>, cx: &mut App) {
        self.set_segments(rows.into_iter().map(|row| (row, false)).collect(), cx);
    }
    /// Soft-wrapped segments share a logical line and stable document offsets.
    pub(crate) fn set_segments(&mut self, rows: Vec<(Option<String>, bool)>, cx: &mut App) {
        let mut text = String::new();
        let mut has_row = false;
        let ranges = rows
            .into_iter()
            .map(|(row, continuation)| {
                row.map(|row| {
                    if has_row && !continuation {
                        text.push('\n');
                    }
                    has_row = true;
                    let start = text.len();
                    text.push_str(&row);
                    start..text.len()
                })
            })
            .collect();
        let replaced = !text.starts_with(&self.document.borrow().text);
        if replaced {
            self.clear(cx);
        }
        let mut state = self.document.borrow_mut();
        state.text = text;
        state.rows = ranges;
        // Old layouts must not hit-test a replacement document before repaint.
        state.runs.clear();
    }

    /// Call on a semantic dataset change, even if its current text is identical.
    pub(crate) fn clear(&mut self, cx: &mut App) {
        if self.handle.snapshot(cx).is_some() || self.handle.has_local_selection(cx) {
            TextSelection::clear_for_window(self.window, cx);
        }
        self.document.borrow_mut().selected = None;
    }

    /// A styled literal portion of a row. Ranges are local UTF-8 byte offsets;
    /// timestamps and messages can retain their existing separate columns.
    pub(crate) fn text(&self, row: usize, range: Range<usize>) -> SelectableLogText {
        let state = self.document.borrow();
        let row = state
            .rows
            .get(row)
            .and_then(Option::as_ref)
            .expect("selectable log text requires a text row");
        assert!(
            range.start <= range.end && range.end <= row.len(),
            "log text range outside row"
        );
        let range = row.start + range.start..row.start + range.end;
        let text: SharedString = state.text[range.clone()].to_owned().into();
        SelectableLogText {
            document: self.document.clone(),
            range,
            text: StyledText::new(text),
            selection_color: None,
            tabs: None,
        }
    }

    pub(crate) fn full_row(&self, row: usize) -> SelectableLogText {
        let len = self.document.borrow().rows[row]
            .as_ref()
            .expect("text row")
            .len();
        self.text(row, 0..len)
    }

    /// Wrap the styled virtual list itself. This adds no layout or scroll layer.
    pub(crate) fn viewport<T: IntoElement>(&self, child: T) -> impl IntoElement + use<T> {
        LogViewport {
            document: self.document.clone(),
            handle: self.handle.clone(),
            child: child.into_any_element(),
        }
    }

    #[cfg(test)]
    pub(crate) fn text_position(&self, row: usize, byte: usize) -> Option<Point<Pixels>> {
        let document = self.document.borrow();
        let row = document.rows.get(row)?.as_ref()?;
        if byte > row.len() {
            return None;
        }
        let offset = row.start + byte;
        let run = document
            .runs
            .iter()
            .find(|run| run.range.contains(&offset) || offset == run.range.end)?;
        let local = offset - run.range.start;
        run.layout.position_for_index(
            run.tabs
                .as_ref()
                .map_or(local, |tabs| tabs.display_offset(local)),
        )
    }
}

fn selected_range(
    snapshot: TextSelectionSnapshot,
    id: gpui::EntityId,
    len: usize,
) -> Option<Range<usize>> {
    let endpoint = |endpoint: gpui::base::TextSelectionEndpoint| {
        (endpoint.entity_id() == Some(id))
            .then(|| {
                endpoint
                    .content_key()
                    .map(|key| (key.value() as usize).min(len))
            })
            .flatten()
    };
    let (anchor, cursor) = (endpoint(snapshot.anchor()), endpoint(snapshot.cursor()));
    match snapshot.coverage() {
        TextSelectionCoverage::Full => Some(0..len),
        TextSelectionCoverage::FromStart => Some(0..anchor.or(cursor)?),
        TextSelectionCoverage::ToEnd => Some(anchor.or(cursor)?..len),
        TextSelectionCoverage::Bounded => {
            let (anchor, cursor) = (anchor?, cursor?);
            Some(anchor.min(cursor)..anchor.max(cursor))
        }
    }
}

struct LogViewport {
    document: Rc<RefCell<Document>>,
    handle: TextSelectionHandle,
    child: AnyElement,
}
impl IntoElement for LogViewport {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for LogViewport {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;
    fn id(&self) -> Option<ElementId> {
        Some(("selectable-log", self.handle.entity_id()).into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        {
            let mut state = self.document.borrow_mut();
            state.bounds = bounds;
            state.runs.clear();
        }
        // Insert before children so ordinary text hitboxes remain on top.
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.child.prepaint(window, cx);
        let state = self.document.borrow();
        let runs = state
            .runs
            .iter()
            .map(|run| {
                TextSelectionRun::new(
                    run.tabs.as_ref().map_or_else(
                        || state.text[run.range.clone()].to_owned(),
                        |tabs| tabs.text.clone(),
                    ),
                    run.layout.clone(),
                    run.bounds,
                )
            })
            .collect::<Vec<_>>();
        let text_bounds = state.runs.iter().map(|run| run.bounds).collect();
        drop(state);
        self.handle.update_runs(&runs, cx);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(text_bounds)
            .with_rendered_element(&self.handle, window, cx);
        self.handle.register(registration, window, cx);
        hitbox
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

pub(crate) struct SelectableLogText {
    document: Rc<RefCell<Document>>,
    range: Range<usize>,
    text: StyledText,
    selection_color: Option<Hsla>,
    tabs: Option<Rc<TabText>>,
}
impl SelectableLogText {
    /// Call before adding highlights; only the display changes, never copied text.
    pub(crate) fn with_tabs(mut self, size: usize) -> Self {
        let document = self.document.borrow();
        let source = &document.text[self.range.clone()];
        if source.contains('\t') {
            let tabs = Rc::new(TabText::new(source, size));
            self.text = StyledText::new(tabs.text.clone());
            self.tabs = Some(tabs);
        }
        drop(document);
        self
    }
    pub(crate) fn with_highlights(
        mut self,
        highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Self {
        self.text = self
            .text
            .with_highlights(highlights.into_iter().map(|(range, style)| {
                (
                    self.tabs
                        .as_ref()
                        .map_or_else(|| range.clone(), |tabs| tabs.display_range(range.clone())),
                    style,
                )
            }));
        self
    }
    pub(crate) fn selection_color(mut self, color: Hsla) -> Self {
        self.selection_color = Some(color);
        self
    }
}
impl IntoElement for SelectableLogText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SelectableLogText {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.text.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .prepaint(id, inspector, bounds, layout, window, cx);
        self.document.borrow_mut().runs.push(Run {
            range: self.range.clone(),
            layout: self.text.layout().clone(),
            bounds,
            tabs: self.tabs.clone(),
        });
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut (),
        prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let selected = self.document.borrow().selected.clone();
        if let Some(selected) = selected {
            let start = selected.start.max(self.range.start);
            let end = selected.end.min(self.range.end);
            if start < end {
                let color = self
                    .selection_color
                    .unwrap_or_else(|| gpui::base::Theme::global(cx).tokens.colors.selection);
                SelectableText::paint_selection(
                    self.text.layout(),
                    self.tabs.as_ref().map_or_else(
                        || start - self.range.start..end - self.range.start,
                        |tabs| tabs.display_range(start - self.range.start..end - self.range.start),
                    ),
                    color,
                    window,
                );
            }
        }
        self.text
            .paint(id, inspector, bounds, layout, prepaint, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AppContext, Context, Entity, Focusable, InteractiveElement, ListAlignment, ListOffset,
        ListState, Modifiers, MouseButton, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, div, point, px, size,
    };
    use gpui_kit::component::ActiveTheme;

    struct Fixture {
        selection: SelectableLog,
        rows: Vec<Option<String>>,
        list: ListState,
        focus: FocusHandle,
        input: Option<Entity<gpui::component::input::InputState>>,
    }
    impl Fixture {
        fn row(&self, index: usize) -> AnyElement {
            let Some(text) = &self.rows[index] else {
                return div()
                    .h(px(24.))
                    .child("Decorative day header")
                    .into_any_element();
            };
            let timestamp = text.find(' ').unwrap();
            let body = &text[timestamp + 1..];
            let highlight = body.find("Café").map(|start| {
                (
                    start..start + "Café".len(),
                    HighlightStyle {
                        background_color: Some(gpui::rgb(0xffdd00).into()),
                        ..Default::default()
                    },
                )
            });
            div()
                .flex()
                .w_full()
                .gap(px(10.))
                .py(px(2.))
                .child(
                    div()
                        .w(px(64.))
                        .flex_shrink_0()
                        .child(self.selection.text(index, 0..timestamp)),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        self.selection
                            .text(index, timestamp + 1..text.len())
                            .with_highlights(highlight)
                            .selection_color(gpui::rgb(0xabcdef).into()),
                    ),
                )
                .into_any_element()
        }
        fn replace(&mut self, rows: Vec<Option<String>>, cx: &mut Context<Self>) {
            self.selection.set_rows(rows.clone(), cx);
            self.list.reset(rows.len());
            self.rows = rows;
            cx.notify();
        }
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let list = gpui::list(
                self.list.clone(),
                cx.processor(|this, index, _, _| this.row(index)),
            )
            .size_full();
            div()
                .size_full()
                .flex()
                .flex_col()
                .track_focus(&self.focus)
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(crate::ui::FONT_SIZE))
                .line_height(px(20.))
                .children(self.input.as_ref().map(gpui::component::input::Input::new))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(self.selection.viewport(list)),
                )
        }
    }
    fn fixture(
        cx: &mut TestAppContext,
        rows: Vec<Option<String>>,
    ) -> (Entity<Fixture>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|cx| {
                let focus = cx.focus_handle();
                let mut selection = SelectableLog::new(focus.clone(), window, cx);
                selection.set_rows(rows.clone(), cx);
                Fixture {
                    selection,
                    list: ListState::new(rows.len(), ListAlignment::Top, px(0.)),
                    focus,
                    rows,
                    input: None,
                }
            });
            view = Some(content.clone());
            gpui::base::Root::new(content, window, cx)
        });
        cx.simulate_resize(size(px(280.), px(140.)));
        draw(cx);
        (view.unwrap(), cx)
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    fn at(
        view: &Entity<Fixture>,
        row: usize,
        byte: usize,
        cx: &mut VisualTestContext,
    ) -> Point<Pixels> {
        view.read_with(cx, |view, _| {
            view.selection
                .text_position(row, byte)
                .unwrap_or_else(|| panic!("row {row} offset {byte} not laid out"))
                + point(px(0.1), px(5.))
        })
    }
    fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        draw(cx);
    }
    fn selected(cx: &mut VisualTestContext) -> String {
        cx.update(TextSelection::selected_text)
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

    #[gpui::test]
    fn literal_unicode_columns_wrapping_highlights_and_copy(cx: &mut TestAppContext) {
        let first = "12:00:00 agent[3]: Café **literal** <tag> 👩🏽‍💻".to_owned();
        let second = "12:00:01 服务: İ e\u{301} 🦀 tail".to_owned();
        let (view, cx) = fixture(cx, vec![Some(first.clone()), Some(second.clone())]);
        let start = first.find("Café").unwrap();
        let end = second.find(" tail").unwrap();
        let from = at(&view, 0, start, cx);
        let to = at(&view, 1, end, cx);
        drag(cx, from, to);
        let expected = format!("{}\n{}", &first[start..], &second[..end]);
        assert_eq!(selected(cx), expected);
        assert_eq!(copy(cx), expected);
        // A resize reflows glyphs without changing the document-byte endpoints.
        cx.simulate_resize(size(px(420.), px(160.)));
        draw(cx);
        assert_eq!(selected(cx), expected);
        // Reverse selection follows the same literal document ordering.
        let from = at(&view, 1, end, cx);
        let to = at(&view, 0, start, cx);
        drag(cx, from, to);
        assert_eq!(selected(cx), expected);
    }

    #[gpui::test]
    fn virtualized_selection_copies_unmounted_rows_and_survives_append(cx: &mut TestAppContext) {
        let rows: Vec<_> = (0..60)
            .map(|n| Some(format!("12:00:{n:02} service: line {n} 🦀")))
            .collect();
        let (view, cx) = fixture(cx, rows.clone());
        let from = at(&view, 0, 0, cx);
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        view.update(cx, |view, cx| {
            view.list.scroll_to(ListOffset {
                item_ix: 30,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        draw(cx);
        let end = rows[30].as_ref().unwrap().len();
        let to = at(&view, 30, end, cx);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        draw(cx);
        let expected = rows[..=30]
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(selected(cx), expected);
        assert_eq!(copy(cx), expected);
        view.read_with(cx, |view, _| {
            assert!(
                view.selection.document.borrow().runs.len() < 30,
                "fixture must actually virtualize"
            )
        });
        view.update(cx, |view, cx| {
            let mut appended = view.rows.clone();
            appended.push(None); // decorative date labels are not copied
            appended.push(Some("12:01:01 appended".into()));
            view.replace(appended, cx);
        });
        draw(cx);
        assert_eq!(selected(cx), expected);
        assert_eq!(copy(cx), expected);
        // Eviction cannot leave offsets pointing into different retained rows.
        view.update(cx, |view, cx| {
            view.replace(view.rows[5..].to_vec(), cx);
        });
        draw(cx);
        assert_eq!(selected(cx), "");
    }

    #[gpui::test]
    fn replacement_and_semantic_reset_clear_only_this_documents_selection(cx: &mut TestAppContext) {
        let (view, cx) = fixture(
            cx,
            vec![
                Some("12:00:00 old output".into()),
                None,
                Some("12:00:01 next".into()),
            ],
        );
        let from = at(&view, 0, 0, cx);
        let to = at(&view, 2, 13, cx);
        drag(cx, from, to);
        assert_eq!(selected(cx), "12:00:00 old output\n12:00:01 next");
        view.update(cx, |view, cx| {
            view.replace(vec![Some("12:00:00 replacement".into())], cx)
        });
        draw(cx);
        assert_eq!(selected(cx), "");
        let from = at(&view, 0, 0, cx);
        let to = at(&view, 0, "12:00:00 replacement".len(), cx);
        drag(cx, from, to);
        assert_eq!(selected(cx), "12:00:00 replacement");
        view.update(cx, |view, cx| view.selection.clear(cx));
        draw(cx);
        assert_eq!(selected(cx), "");
    }

    #[gpui::test]
    fn log_refresh_does_not_clear_an_unrelated_input_selection(cx: &mut TestAppContext) {
        use gpui::component::input::InputState;
        let (view, cx) = fixture(cx, vec![Some("12:00:00 old".into())]);
        let input = cx.update(|window, cx| {
            cx.new(|cx| InputState::new(window, cx).default_value("Unrelated input"))
        });
        view.update(cx, |view, cx| {
            view.input = Some(input.clone());
            cx.notify();
        });
        draw(cx);
        cx.update(|window, cx| input.read(cx).focus_handle(cx).focus(window, cx));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        });
        assert_eq!(copy(cx), "Unrelated input");
        view.update(cx, |view, cx| {
            view.replace(vec![Some("12:00:01 replacement".into())], cx)
        });
        draw(cx);
        assert_eq!(copy(cx), "Unrelated input");
    }

    #[gpui::test]
    fn blank_only_drag_does_not_select_log_text(cx: &mut TestAppContext) {
        let (_, cx) = fixture(cx, vec![Some("12:00:00 short".into())]);
        drag(cx, point(px(200.), px(100.)), point(px(220.), px(110.)));
        assert_eq!(selected(cx), "");
    }

    #[gpui::test]
    fn copy_preserves_literal_indentation_and_selected_spaces(cx: &mut TestAppContext) {
        let first = "12:00:00   literal text  ";
        let spaces = "12:00:01    ";
        let (view, cx) = fixture(cx, vec![Some(first.into()), Some(spaces.into())]);
        let from = at(&view, 0, 9, cx);
        let to = at(&view, 0, first.len(), cx);
        drag(cx, from, to);
        assert_eq!(copy(cx), "  literal text  ");
        let from = at(&view, 1, 9, cx);
        let to = at(&view, 1, spaces.len(), cx);
        drag(cx, from, to);
        assert_eq!(selected(cx), "   ");
        assert_eq!(copy(cx), "   ");
    }
}
