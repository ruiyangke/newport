use super::{ListingBody, file_name, file_name_label, listing_panel};
use gpui_kit as gpui;
use gpui_kit::component::Disableable;
use gpui_kit::{
    Context, IntoElement, Render, Role, TestAppContext, VisualTestContext, Window, div, prelude::*,
    px,
};
use serde_json::json;

struct ListingHost {
    state: ListingBody,
    width: f32,
}
impl Render for ListingHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = json!({"name":"linked-readme.md", "kind":"symlink"});
        div()
            .w(px(self.width))
            .h(px(280.))
            .flex()
            .child(listing_panel(
                self.state,
                self.width >= 520.,
                usize::from(matches!(self.state, ListingBody::Rows)),
                div()
                    .id("loaded-file-rows")
                    .debug_selector(|| "loaded-file-rows".into())
                    .flex_1()
                    .child(
                        crate::controls::button("linked-file")
                            .accessibility_label(file_name_label(&entry))
                            .child(file_name(&entry)),
                    )
                    .into_any_element(),
                cx,
            ))
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Focus notification requests scroll reveal for the following layout.
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

struct ReorderedFiles {
    paths: Vec<&'static str>,
    activated: Option<(String, usize)>,
}
impl Render for ReorderedFiles {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(self.paths.iter().enumerate().map(|(index, path)| {
                let owner = cx.entity().downgrade();
                super::retained_row(
                    super::file_row_identity("server", path),
                    (path.to_string(), index),
                    move |(path, index), _, _| {
                        let path = path.clone();
                        let index = *index;
                        let owner = owner.clone();
                        crate::controls::button("file-entry")
                            .label(path.clone())
                            .debug_selector({
                                let path = path.clone();
                                move || path.clone()
                            })
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |this, _| {
                                    this.activated = Some((path.clone(), index))
                                });
                            })
                            .into_any_element()
                    },
                    window,
                    cx,
                )
            }))
    }
}

#[gpui::test]
fn file_focus_follows_identity_after_insertion_and_reordering(cx: &mut TestAppContext) {
    use gpui::AppContext;
    cx.update(|cx| gpui::component::init(cx));
    let mut host = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| ReorderedFiles {
            paths: vec!["b.txt", "c.txt"],
            activated: None,
        });
        host = Some(view.clone());
        gpui::base::Root::new(view, window, cx)
    });
    let host = host.unwrap();
    cx.update(|window, _| window.activate_window());
    draw(cx);
    let focused = cx.update(|window, cx| {
        window.blur(cx);
        window.focus_next(cx);
        window.focused(cx).expect("file row has a tab stop")
    });
    draw(cx);
    for (paths, expected_index) in [
        (vec!["a.txt", "b.txt", "c.txt"], 1),
        (vec!["c.txt", "b.txt", "a.txt"], 1),
        (vec!["b.txt", "c.txt"], 0),
    ] {
        host.update(cx, |host, cx| {
            host.paths = paths;
            host.activated = None;
            cx.notify();
        });
        draw(cx);
        cx.update(|window, _| {
            assert!(focused.is_focused(window), "row focus must survive reorder")
        });
        let keystroke = gpui::Keystroke::parse("enter").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        draw(cx);
        assert_eq!(
            host.read_with(cx, |host, _| host.activated.clone()),
            Some(("b.txt".into(), expected_index))
        );
    }
    assert_ne!(
        super::file_row_identity("one", "/b.txt"),
        super::file_row_identity("two", "/b.txt")
    );
    assert_ne!(
        super::file_row_identity("a:b", "c"),
        super::file_row_identity("a", "b:c")
    );
}

#[gpui::test]
fn loading_and_empty_states_occupy_rows_without_moving_column_header(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_reduce_motion(true);
    });
    let (host, cx) = cx.add_window_view(|_, _| ListingHost {
        state: ListingBody::Loading,
        width: 640.,
    });
    for width in [640., 280.] {
        host.update(cx, |host, cx| {
            host.width = width;
            host.state = ListingBody::Loading;
            cx.notify();
        });
        draw(cx);
        let header = cx.debug_bounds("file-list-columns").unwrap();
        let panel = cx.debug_bounds("file-list").unwrap();
        let first = cx.debug_bounds("file-skeleton-row-0").unwrap();
        let last = cx.debug_bounds("file-skeleton-row-5").unwrap();
        assert_eq!(
            header.top(),
            panel.top(),
            "loading must not precede the columns"
        );
        assert_eq!(first.top(), header.bottom());
        assert_eq!(first.size.height, px(36.));
        assert!(last.bottom() <= panel.bottom());
        assert!(first.right() <= panel.right());
        assert!(cx.debug_bounds("loaded-file-rows").is_none());

        for (state, expected) in [
            (ListingBody::Empty, "Empty folder."),
            (ListingBody::NoMatches, "No matching files."),
        ] {
            host.update(cx, |host, cx| {
                host.state = state;
                cx.notify();
            });
            draw(cx);
            assert_eq!(cx.debug_bounds("file-list-columns").unwrap(), header);
            assert_eq!(
                cx.debug_bounds("file-list-empty").unwrap().top(),
                header.bottom()
            );
            assert!(cx.debug_bounds("file-list-loading").is_none());
            cx.update(|window, _| {
                assert!(
                    gpui_kit::base::test_support::snapshots(window)
                        .iter()
                        .any(|node| {
                            node.role() == Some(Role::Status) && node.label() == Some(expected)
                        })
                );
            });
        }
        host.update(cx, |host, cx| {
            host.state = ListingBody::Rows;
            cx.notify();
        });
        draw(cx);
        assert_eq!(cx.debug_bounds("file-list-columns").unwrap(), header);
        assert_eq!(
            cx.debug_bounds("loaded-file-rows").unwrap().top(),
            header.bottom()
        );
        assert!(cx.debug_bounds("file-list-loading").is_none());
        assert!(cx.debug_bounds("file-list-empty").is_none());
        cx.update(|window, _| {
            assert!(
                gpui_kit::base::test_support::snapshots(window)
                    .iter()
                    .any(|node| {
                        node.role() == Some(Role::Button)
                            && node.label() == Some("linked-readme.md · Link")
                    })
            );
        });
    }
}

struct KeyboardListingHost {
    scroll: crate::modal::FormScroll,
    before: gpui::FocusHandle,
    after: gpui::FocusHandle,
    controls: Vec<[gpui::FocusHandle; 3]>,
    activated: Option<usize>,
    offset: usize,
    cached: bool,
    renders: std::rc::Rc<std::cell::Cell<usize>>,
}
impl Render for KeyboardListingHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                crate::controls::button("before-files")
                    .label("Before files")
                    .track_focus(&self.before),
            )
            .child(super::paged_rows(
                &self.scroll,
                self.controls.iter().enumerate().map(|(row, handles)| {
                    let owner = cx.entity().downgrade();
                    let handles = handles.clone();
                    let props = (row, self.offset);
                    let renders = self.renders.clone();
                    let render = move |&(row, offset): &(usize, usize),
                                       _: &mut Window,
                                       _: &mut gpui::App| {
                        renders.set(renders.get() + 1);
                        div()
                            .id(("keyboard-file-row", row))
                            .debug_selector(move || format!("keyboard-file-row-{row}"))
                            .h(px(36.))
                            .flex()
                            .items_center()
                            .children(handles.iter().enumerate().map(|(column, focus)| {
                                crate::controls::button(("file-keyboard-control", row * 3 + column))
                                    .track_focus(focus)
                                    .label(format!("{}/{column}", row + offset))
                                    // A file without a transfer action, or an ongoing transfer.
                                    .disabled(row == 7 && column == 1)
                                    .on_click({
                                        let owner = owner.clone();
                                        move |_, _, cx| {
                                            let _ = owner.update(cx, |this, _| {
                                                this.activated = Some(row + offset)
                                            });
                                        }
                                    })
                            }))
                            .into_any_element()
                    };
                    if self.cached {
                        super::retained_row(("test-retained-row", row), props, render, window, cx)
                    } else {
                        render(&props, window, cx)
                    }
                }),
            ))
            .child(
                crate::controls::button("after-files")
                    .label("After files")
                    .track_focus(&self.after),
            )
    }
}

fn keyboard_fixture(
    cx: &mut TestAppContext,
) -> (gpui::Entity<KeyboardListingHost>, &mut VisualTestContext) {
    keyboard_fixture_count(cx, super::PAGE_SIZE)
}
fn keyboard_fixture_count(
    cx: &mut TestAppContext,
    count: usize,
) -> (gpui::Entity<KeyboardListingHost>, &mut VisualTestContext) {
    use gpui::{AppContext, size};
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
    });
    let mut host = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| KeyboardListingHost {
            scroll: crate::modal::FormScroll::new(count, window, cx),
            before: cx.focus_handle(),
            after: cx.focus_handle(),
            controls: (0..count)
                .map(|_| std::array::from_fn(|_| cx.focus_handle()))
                .collect(),
            activated: None,
            offset: 0,
            cached: true,
            renders: Default::default(),
        });
        host = Some(view.clone());
        gpui::base::Root::new(view, window, cx)
    });
    let host = host.unwrap();
    cx.simulate_resize(size(px(640.), px(260.)));
    cx.update(|window, _| window.activate_window());
    draw(cx);
    (host, cx)
}

#[gpui::test]
fn tab_reaches_every_file_on_the_page_and_reveals_focus_in_both_directions(
    cx: &mut TestAppContext,
) {
    let (host, cx) = keyboard_fixture(cx);
    cx.update(|window, cx| host.read(cx).before.clone().focus(window, cx));
    let order: Vec<_> = (0..super::PAGE_SIZE)
        .flat_map(|row| (0..3).map(move |column| (row, column)))
        .filter(|pair| *pair != (7, 1))
        .collect();
    for reverse in [false, true] {
        let sequence: Vec<_> = if reverse {
            order.iter().copied().rev().collect()
        } else {
            order.clone()
        };
        for (row, column) in sequence {
            cx.simulate_keystrokes(if reverse { "shift-tab" } else { "tab" });
            draw(cx);
            cx.update(|window, cx| {
                assert!(
                    host.read(cx).controls[row][column].is_focused(window),
                    "row {row}, control {column}, reverse {reverse}"
                )
            });
            let viewport = host.read_with(cx, |host, _| host.scroll.handle.bounds());
            let bounds = cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .find(|node| node.label() == Some(format!("{row}/{column}").as_str()))
                    .unwrap()
                    .bounds()
            });
            assert!(
                bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom(),
                "row {row} must be revealed: {bounds:?} in {viewport:?}"
            );
        }
        cx.simulate_keystrokes(if reverse { "shift-tab" } else { "tab" });
        draw(cx);
        cx.update(|window, cx| {
            let host = host.read(cx);
            assert!(if reverse { &host.before } else { &host.after }.is_focused(window));
            assert_eq!(
                host.activated, None,
                "navigation never activates a remote operation"
            );
        });
    }
    // Reaching an offscreen row also preserves its actual activation target.
    cx.update(|window, cx| host.read(cx).controls[32][0].clone().focus(window, cx));
    draw(cx);
    let keystroke = gpui::Keystroke::parse("enter").unwrap();
    cx.simulate_event(gpui::KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke,
    });
    assert_eq!(host.read_with(cx, |host, _| host.activated), Some(32));
    // Pointer scrolling is still free after the one-time focus reveal.
    host.read_with(cx, |host, _| {
        host.scroll.handle.set_offset(gpui::point(px(0.), px(0.)))
    });
    draw(cx);
    assert_eq!(
        host.read_with(cx, |host, _| host.scroll.handle.offset().y),
        px(0.)
    );
}

#[gpui::test]
fn cached_rows_reuse_layout_but_refresh_labels_and_actions_with_new_props(cx: &mut TestAppContext) {
    let (host, cx) = keyboard_fixture(cx);
    let before = host.read_with(cx, |host, _| host.renders.get());
    host.update(cx, |_, cx| cx.notify());
    draw(cx);
    assert_eq!(
        host.read_with(cx, |host, _| host.renders.get()),
        before,
        "unrelated parent redraw must reuse row subtrees"
    );

    host.update(cx, |host, cx| {
        host.offset = 1000;
        cx.notify();
    });
    draw(cx);
    assert_eq!(
        host.read_with(cx, |host, _| host.renders.get()),
        before + super::PAGE_SIZE,
        "changed row props must invalidate every affected row"
    );
    cx.update(|window, cx| host.read(cx).controls[32][0].clone().focus(window, cx));
    draw(cx);
    let viewport = host.read_with(cx, |host, _| host.scroll.handle.bounds());
    cx.update(|window, _| {
        let nodes = gpui::base::test_support::snapshots(window);
        assert!(!nodes.iter().any(|node| node.label() == Some("32/0")));
        let node = nodes
            .iter()
            .find(|node| node.label() == Some("1032/0"))
            .unwrap();
        assert!(
            node.bounds().top() >= viewport.top() && node.bounds().bottom() <= viewport.bottom()
        );
    });
    let keystroke = gpui::Keystroke::parse("enter").unwrap();
    cx.simulate_event(gpui::KeyDownEvent {
        physical_key: None,
        key_metadata: None,
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui::KeyUpEvent {
        physical_key: None,
        key_metadata: None,
        keystroke,
    });
    assert_eq!(
        host.read_with(cx, |host, _| host.activated),
        Some(1032),
        "cached callback must target the new row identity"
    );

    // Changing the viewport must remeasure cached rows, not retain old bounds.
    cx.simulate_resize(gpui::size(px(380.), px(400.)));
    draw(cx);
    assert_eq!(
        cx.debug_bounds("file-page-rows").unwrap().size.width,
        px(380.)
    );
}

#[gpui::test]
#[ignore = "manual paginated file-list CPU benchmark; excludes GPU and SSH"]
fn benchmark_file_page_frames(cx: &mut TestAppContext) {
    for count in [100, 40] {
        let (host, visual) = keyboard_fixture_count(cx, count);
        for cached in [false, true] {
            host.update(visual, |host, cx| {
                host.cached = cached;
                cx.notify();
            });
            draw(visual);
            for scrolling in [false, true] {
                let started = std::time::Instant::now();
                for step in 0..30 {
                    host.update(visual, |host, cx| {
                        if scrolling {
                            host.scroll
                                .handle
                                .set_offset(gpui::point(px(0.), px(-36. * step as f32)));
                        }
                        cx.notify();
                    });
                    draw(visual);
                }
                eprintln!(
                    "{count} file rows / {} controls (cached={cached}, scrolling={scrolling}): {:.3} ms per CPU redraw",
                    count * 3,
                    started.elapsed().as_secs_f64() * 1000. / 60.
                );
            }
        }
    }
}
