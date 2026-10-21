//! Read-only Compose files. The client owns reads; this view retains only file
//! choice, decoded presentation data, and the current editor's local viewport.
use super::*;
use gpui_kit::component::Selectable;
use gpui_kit::component::{
    input::{Editor, EditorState},
    tab::Tab,
};
use newport_domain::compose::ComposeProject;
use serde::Deserialize;
use std::time::Instant;

#[cfg(test)]
#[path = "compose_files_tests.rs"]
mod tests;

pub(super) struct Files {
    project: Option<String>,
    stamp: Option<(u64, Option<Instant>)>,
    data: Option<ComposeProject>,
    decode_error: Option<String>,
    error: Option<String>,
    pending: bool,
    pub(super) selected_path: Option<String>,
    tabs: Vec<(String, FocusHandle)>,
    tab_scroll: ScrollHandle,
    pub(super) editor: Entity<EditorState>,
    identity: Option<(String, String)>,
    editor_value: String,
    wrap: bool,
}

impl Files {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ContainersPanel>) -> Self {
        Self {
            project: None,
            stamp: None,
            data: None,
            decode_error: None,
            error: None,
            pending: false,
            selected_path: None,
            tabs: Vec::new(),
            tab_scroll: ScrollHandle::new(),
            editor: new_editor("", false, window, cx),
            identity: None,
            editor_value: String::new(),
            wrap: false,
        }
    }

    /// Decode only a new successful reading. A failed refresh retains this
    /// project's last readable response; another project's data is never kept.
    pub(super) fn sync(
        &mut self,
        client: &Containers,
        window: &mut Window,
        cx: &mut Context<ContainersPanel>,
    ) {
        let project = client.selected_project();
        let changed_project = self.project.as_deref() != project;
        if changed_project {
            self.project = project.map(str::to_owned);
            self.stamp = None;
            self.data = None;
            self.decode_error = None;
            self.selected_path = None;
            self.tabs.clear();
            self.tab_scroll.set_offset(point(px(0.), px(0.)));
        }
        let resource = client.resource("compose-detail");
        self.pending = resource.is_some_and(|resource| resource.pending);
        self.error = resource.and_then(|resource| resource.error.clone());
        let stamp = resource.map(|resource| (resource.revision, resource.updated_at));
        let mut changed_data = false;
        if project.is_some() && stamp != self.stamp {
            self.stamp = stamp;
            if let Some(value) = client.compose_project() {
                match ComposeProject::deserialize(value) {
                    Ok(data) => {
                        self.data = Some(data);
                        self.decode_error = None;
                        changed_data = true;
                    }
                    Err(error) => {
                        self.decode_error =
                            Some(format!("Couldn’t read the Compose files response: {error}"))
                    }
                }
            }
        }
        if changed_project || changed_data {
            self.sync_choice(window, cx);
            self.sync_editor(window, cx);
        }
    }

    fn sync_choice(&mut self, window: &mut Window, cx: &mut Context<ContainersPanel>) {
        let files = self
            .data
            .as_ref()
            .map(|data| data.files.as_slice())
            .unwrap_or_default();
        let was_focused = self.tabs.iter().any(|(_, focus)| focus.is_focused(window));
        let previous = self.selected_path.clone();
        if !files
            .iter()
            .any(|file| Some(file.path.as_str()) == self.selected_path.as_deref())
        {
            self.selected_path = files.first().map(|file| file.path.clone());
        }
        self.tabs = files
            .iter()
            .map(|file| {
                let focus = self
                    .tabs
                    .iter()
                    .find(|(path, _)| *path == file.path)
                    .map(|(_, focus)| focus.clone())
                    .unwrap_or_else(|| cx.focus_handle());
                (file.path.clone(), focus)
            })
            .collect();
        if was_focused && previous != self.selected_path {
            self.focus_selected(window, cx);
        }
    }

    fn sync_editor(&mut self, window: &mut Window, cx: &mut Context<ContainersPanel>) {
        let file = self.data.as_ref().and_then(|data| {
            data.files
                .iter()
                .find(|file| Some(file.path.as_str()) == self.selected_path.as_deref())
        });
        let readable = file.and_then(|file| file.text.as_deref());
        let identity = self
            .project
            .as_ref()
            .zip(file)
            .filter(|_| readable.is_some())
            .map(|(project, file)| (project.clone(), file.path.clone()));
        let text = readable.unwrap_or_default();
        if self.identity != identity {
            self.identity = identity;
            self.editor_value = text.to_owned();
            self.editor = new_editor(text, self.wrap, window, cx);
        } else if self.editor_value != text {
            self.editor_value = text.to_owned();
            self.editor.update(cx, |editor, cx| {
                let selection = editor.selected_range();
                let scroll = editor.scroll_offset();
                editor.set_value(text.to_owned(), window, cx);
                // set_value resets both; restore this same file's viewport and
                // selection, clipped by Kit to new bounds and UTF-8 boundaries.
                editor.set_selected_range(selection, cx);
                editor.set_scroll_offset(scroll, cx);
            });
        }
    }

    fn choose(
        &mut self,
        project: &str,
        path: &str,
        window: &mut Window,
        cx: &mut Context<ContainersPanel>,
    ) {
        if self.project.as_deref() != Some(project)
            || !self.tabs.iter().any(|(current, _)| current == path)
        {
            return;
        }
        if self.selected_path.as_deref() != Some(path) {
            self.selected_path = Some(path.to_owned());
            self.sync_editor(window, cx);
        }
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn focus_selected(&self, window: &mut Window, cx: &mut App) {
        if let Some((index, (_, focus))) = self
            .tabs
            .iter()
            .enumerate()
            .find(|(_, (path, _))| Some(path.as_str()) == self.selected_path.as_deref())
        {
            self.tab_scroll.scroll_to_item(index);
            focus.focus(window, cx);
        }
    }

    fn tab_key(
        &mut self,
        project: &str,
        path: &str,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<ContainersPanel>,
    ) {
        if event.keystroke.modifiers.modified() || window.has_active_dialog(cx) {
            return;
        }
        let Some(index) = self
            .tabs
            .iter()
            .position(|(current, focus)| current == path && focus.is_focused(window))
        else {
            return;
        };
        let count = self.tabs.len();
        let next = match event.keystroke.key.as_str() {
            "left" => (index + count - 1) % count,
            "right" => (index + 1) % count,
            "home" => 0,
            "end" => count - 1,
            "enter" | "space" => index,
            _ => return,
        };
        let path = self.tabs[next].0.clone();
        self.choose(project, &path, window, cx);
        window.prevent_default();
        cx.stop_propagation();
    }

    pub(super) fn view(&self, cx: &mut Context<ContainersPanel>) -> AnyElement {
        let Some(project) = self.project.as_ref() else {
            return div().into_any_element();
        };
        let error = self.error.as_ref().or(self.decode_error.as_ref());
        let mut view = div()
            .id("compose-files")
            .debug_selector(|| "compose-files".into())
            .role(accesskit::Role::Region)
            .aria_label("Compose file contents")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .overflow_hidden();
        if let Some(error) = error {
            let project = project.clone();
            view = view.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .flex_shrink_0()
                    .child(alert("compose-files-read-error", error.clone(), cx).test_support())
                    .child(
                        crate::controls::button("compose-files-retry")
                            .label("Try again")
                            .accessibility_label("Try reading files again")
                            .disabled(self.pending)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.compose_project_selected(&project) && this.tab == 2 {
                                    this.client.retry_compose();
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        let Some(data) = self.data.as_ref() else {
            if error.is_none() {
                view = view.child(
                    div()
                        .id("compose-files-reading")
                        .role(accesskit::Role::Status)
                        .aria_label("Reading compose files")
                        .test_support()
                        .child(loading::lines("compose-files-reading", 6)),
                );
            }
            return view.into_any_element();
        };
        if let Some(error) = &data.config_error {
            view = view.child(
                div()
                    .flex_shrink_0()
                    .rounded(px(crate::ui::RADIUS))
                    .border_1()
                    .border_color(cx.theme().danger.opacity(0.3))
                    .bg(cx.theme().danger.opacity(0.05))
                    .px(px(12.))
                    .py(px(8.))
                    .child(
                        alert(
                            "compose-files-config-error",
                            "Compose can’t read this project’s configuration.",
                            cx,
                        )
                        .font_weight(FontWeight::SEMIBOLD)
                        .test_support(),
                    )
                    .child(
                        quiet(error.clone(), cx)
                            .id("compose-files-config-details")
                            .role(accesskit::Role::Label)
                            .aria_label(error.clone())
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(crate::ui::FONT_SIZE))
                            .test_support(),
                    ),
            );
        }
        if self.tabs.len() > 1 {
            let project = project.clone();
            let paths: Vec<_> = self.tabs.iter().map(|(path, _)| path.clone()).collect();
            view = view.child(
                crate::controls::tabs("compose-file-tabs")
                    .underline()
                    .menu(true)
                    .track_scroll(&self.tab_scroll)
                    .max_width(px(220.))
                    .selected_index(
                        self.tabs
                            .iter()
                            .position(|(path, _)| {
                                Some(path.as_str()) == self.selected_path.as_deref()
                            })
                            .unwrap_or(0),
                    )
                    .children(self.tabs.iter().map(|(path, focus)| {
                        let (project, path) = (project.clone(), path.clone());
                        Tab::new()
                            .label(path.rsplit('/').next().unwrap_or(&path).to_owned())
                            .aria_label(path.clone())
                            .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
                            .track_focus(
                                &focus
                                    .clone()
                                    .tab_stop(self.selected_path.as_deref() == Some(path.as_str())),
                            )
                            .on_key_down(cx.listener(move |this, event, window, cx| {
                                if this.compose_project_selected(&project) && this.tab == 2 {
                                    this.files.tab_key(&project, &path, event, window, cx);
                                }
                            }))
                    }))
                    .on_click(cx.listener(move |this, index: &usize, window, cx| {
                        if this.compose_project_selected(&project)
                            && this.tab == 2
                            && let Some(path) = paths.get(*index)
                        {
                            this.files.choose(&project, path, window, cx);
                        }
                    })),
            );
        }
        let Some(file) = data
            .files
            .iter()
            .find(|file| Some(file.path.as_str()) == self.selected_path.as_deref())
        else {
            return view
                .when(error.is_none() && data.config_error.is_some(), |view| {
                    view.child(self.retry_button(project, cx))
                })
                .when(error.is_none() && data.config_error.is_none(), |view| {
                    view.child(
                        crate::empty::compact("No compose file is recorded for this project.", cx)
                            .id("compose-files-empty")
                            .role(accesskit::Role::Status)
                            .aria_label("No compose file is recorded for this project.")
                            .test_support(),
                    )
                })
                .into_any_element();
        };
        view = view.child(
            div()
                .id("compose-file-path")
                .role(accesskit::Role::Label)
                .aria_label(file.path.clone())
                .text_size(px(crate::ui::FONT_SIZE))
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().muted_foreground)
                .flex_shrink_0()
                .child(file.path.clone())
                .test_support(),
        );
        if let Some(error) = &file.error {
            view = view.child(
                alert("compose-file-error", error.clone(), cx)
                    .flex_shrink_0()
                    .test_support(),
            );
        }
        if error.is_none() && (data.config_error.is_some() || file.error.is_some()) {
            view = view.child(self.retry_button(project, cx));
        }
        if file.text.is_some() {
            view = view.child(self.tools(project, &file.path, cx)).child(
                div()
                    .id("compose-file-editor")
                    .debug_selector(|| "compose-file-editor".into())
                    .flex_1()
                    .min_h(px(280.))
                    .min_w_0()
                    .overflow_hidden()
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(
                        Editor::new(&self.editor)
                            .readonly(true)
                            .aria_label(file.path.clone())
                            .h_full()
                            .text_size(px(crate::ui::FONT_SIZE)),
                    ),
            );
        } else if file.error.is_none() {
            view = view.child(quiet("File contents are unavailable.", cx));
        }
        view.into_any_element()
    }

    fn retry_button(&self, project: &str, cx: &mut Context<ContainersPanel>) -> AnyElement {
        let project = project.to_owned();
        crate::controls::button("compose-files-retry")
            .label("Try again")
            .accessibility_label("Try reading files again")
            .disabled(self.pending)
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.compose_project_selected(&project) && this.tab == 2 {
                    this.client.retry_compose();
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn tools(&self, project: &str, path: &str, cx: &mut Context<ContainersPanel>) -> AnyElement {
        let mut bar = div().flex().items_center().gap(px(6.)).flex_shrink_0();
        for (id, label, icon) in [
            ("compose-file-find", "Find", IconName::Search),
            ("compose-file-copy", "Copy", IconName::Copy),
        ] {
            let (project, path) = (project.to_owned(), path.to_owned());
            bar = bar.child(
                crate::controls::compact_button(id)
                    .ghost()
                    .label(label)
                    .icon(icon)
                    .accessibility_label(format!("{label} file contents"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.compose_project_selected(&project)
                            || this.tab != 2
                            || this.files.selected_path.as_deref() != Some(path.as_str())
                        {
                            return;
                        }
                        if id == "compose-file-copy" {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                this.files.editor_value.clone(),
                            ));
                        } else {
                            this.files.editor.update(cx, |editor, cx| {
                                editor.focus(window, cx);
                                editor.open_search(false, cx);
                            });
                        }
                    })),
            );
        }
        let (project, path) = (project.to_owned(), path.to_owned());
        bar.child(
            crate::controls::compact_button("compose-file-wrap")
                .ghost()
                .label("Wrap lines")
                .selected(self.wrap)
                .toggled(self.wrap)
                .accessibility_label("Wrap file lines")
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.compose_project_selected(&project)
                        && this.tab == 2
                        && this.files.selected_path.as_deref() == Some(path.as_str())
                    {
                        this.files.wrap = !this.files.wrap;
                        let wrap = this.files.wrap;
                        this.files
                            .editor
                            .update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
                        cx.notify();
                    }
                })),
        )
        .into_any_element()
    }
}

fn new_editor(
    text: &str,
    wrap: bool,
    window: &mut Window,
    cx: &mut Context<ContainersPanel>,
) -> Entity<EditorState> {
    cx.new(|cx| {
        EditorState::new(window, cx)
            .line_number(true)
            .searchable(true)
            .replaceable(false)
            .soft_wrap(wrap)
            .default_value(text.to_owned())
    })
}
