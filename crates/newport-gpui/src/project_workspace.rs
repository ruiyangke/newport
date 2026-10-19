//! One Projects shell: navigation changes independently from retained editor tabs.
use super::*;
use gpui_kit::component::tab::Tab;

pub(super) fn document_tab_surface(
    id: impl Into<ElementId>,
    active: bool,
    cx: &App,
) -> gpui::Stateful<Div> {
    let background = if active {
        cx.theme().tab_active
    } else {
        cx.theme().muted
    };
    let hover = if active {
        background
    } else {
        theme::row_hover(cx)
    };
    div()
        .id(id)
        .h_full()
        .flex()
        .items_center()
        .flex_shrink_0()
        .px_1()
        .bg(background)
        .border_r_1()
        .border_color(cx.theme().border)
        .when(!active, |view| view.border_b_1())
        .hover(move |view| view.bg(hover))
}

fn status_view_button(
    id: &'static str,
    icon: IconName,
    title: &'static str,
    active: bool,
    cx: &App,
) -> Button {
    crate::controls::compact_button(id)
        .ghost()
        .icon(icon)
        .label(title)
        .selected(active)
        .toggled(active)
        .h(px(crate::ui::STATUS_HEIGHT - 1.))
        .min_h(px(crate::ui::STATUS_HEIGHT - 1.))
        .max_h(px(crate::ui::STATUS_HEIGHT - 1.))
        .px(px(8.))
        .rounded(px(crate::ui::RADIUS))
        .border_0()
        .text_color(if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        .when(active, |button| {
            button.bg(cx.theme().foreground.opacity(0.08))
        })
}

impl Newport {
    pub(super) fn project_restore_document(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Projects || !self.git.restore_document {
            return;
        }
        let Some(editor) = self.code_workspace() else {
            return;
        };
        self.git.restore_document = false;
        let state = editor.read(cx);
        if !state.reviewing {
            return;
        }
        let document = state
            .navigator
            .documents
            .reviews
            .iter()
            .find(|item| Some(&item.key) == state.navigator.documents.active.as_ref())
            .cloned();
        if let Some(document) = document {
            // Page navigation releases the repository and retires its reads,
            // while document tabs belong to the retained project. Reopen the
            // active identity against the new handle, never the old resources.
            self.project_open_document(document, cx);
        }
    }

    pub(super) fn project_review_compact(&self, cx: &App) -> bool {
        self.code_workspace()
            .map(|e| e.read(cx).narrow)
            .unwrap_or_else(|| git_layout::compact(self.content_width, self.git.tab))
    }

    pub(super) fn project_history_stacked(&self, fallback: bool, cx: &App) -> bool {
        let Some(width) = self
            .code_workspace()
            .map(|e| e.read(cx).center_width)
            .filter(|width| *width > 0.)
        else {
            return fallback;
        };
        width < 700.
    }

    pub(super) fn project_editor_active(&self, cx: &App) -> bool {
        self.code_workspace().is_some_and(|e| !e.read(cx).reviewing)
    }
    pub(super) fn project_show_review(&self, cx: &mut Context<Self>) {
        if let Some(editor) = self.code_workspace() {
            editor.update(cx, |e, cx| {
                if self.git.tab > 1 {
                    e.navigator.documents.active = None;
                }
                e.show_review(cx);
            });
        }
    }
    fn project_choose_panel(&mut self, panel: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.code_workspace() {
            editor.update(cx, |e, cx| e.navigate(panel, window, cx));
        }
        cx.notify();
    }
    fn project_panel_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.code_workspace() else {
            return div().into_any_element();
        };
        let files = editor.read(cx).files;
        let mut panels = vec!["Changes", "History"];
        if self.git.tab > 1 {
            panels.push(match self.git.tab {
                3 => "Stashes",
                4 => "Tags",
                6 => "Remotes",
                _ => "Repository",
            });
        }
        crate::ui::header()
            .when(files, |header| {
                header.gap_1().px_1().child(crate::ui::header_rule(cx))
            })
            .when(files, |header| {
                header.child(
                    div()
                        .flex_1()
                        .px_2()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Explorer"),
                )
            })
            .when(!files, |header| {
                header.child(
                    crate::controls::tabs("project-navigation")
                        .accessibility_label("Project navigation")
                        .with_size(px(crate::ui::HEADER_HEIGHT))
                        .rounded_none()
                        .bg(cx.theme().muted)
                        .flex_1()
                        .min_w_0()
                        .selected_index(editor.read(cx).navigator.panel.saturating_sub(1))
                        .children(panels.into_iter().map(|name| {
                            Tab::new()
                                .label(name)
                                .font_weight(FontWeight::MEDIUM)
                                .rounded_none()
                                .flex_1()
                        }))
                        .on_click(cx.listener(|this, index: &usize, window, cx| {
                            this.project_choose_panel(*index + 1, window, cx)
                        })),
                )
            })
            .when(files, |header| {
                let editor = editor.clone();
                header.child(
                    crate::controls::compact_icon_button("project-quick-open")
                        .ghost()
                        .icon(IconName::Search)
                        .accessibility_label("Open file")
                        .tooltip(if cfg!(target_os = "macos") {
                            "Open file (⌘P)"
                        } else {
                            "Open file (Ctrl+P)"
                        })
                        .disabled(!editor.read(cx).can_quick_open())
                        .on_click(move |_, window, cx| {
                            editor.update(cx, |e, cx| e.open_quick(window, cx))
                        }),
                )
            })
            .child(
                crate::controls::compact_icon_button("project-refresh")
                    .ghost()
                    .when(!files, |button| {
                        button
                            .w(px(34.))
                            .min_w(px(34.))
                            .max_w(px(34.))
                            .h(px(34.))
                            .min_h(px(34.))
                            .max_h(px(34.))
                            .rounded_none()
                            .border_b_1()
                            .border_color(cx.theme().border)
                    })
                    .icon(IconName::RefreshCw)
                    .loading(if files {
                        editor.read(cx).loading_files()
                    } else {
                        let e = editor.read(cx);
                        matches!(e.navigator.panel, 1 | 2)
                            && e.navigator.collections[e.navigator.panel - 1].loading
                    })
                    .accessibility_label(if files {
                        "Refresh files"
                    } else {
                        "Refresh Git"
                    })
                    .tooltip(if files {
                        "Refresh files"
                    } else {
                        "Refresh Git"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if files {
                            editor.update(cx, |e, cx| e.refresh_files(window, cx));
                        } else {
                            editor.update(cx, |e, cx| {
                                if matches!(e.navigator.panel, 1 | 2) {
                                    e.load_collection(e.navigator.panel - 1, false, window, cx);
                                }
                            });
                            if this.git.tab > 1 {
                                this.git_refresh();
                            }
                            cx.notify();
                        }
                    })),
            )
            .when(self.content_width < 600., |v| {
                v.child(
                    crate::controls::compact_icon_button("project-return-content")
                        .ghost()
                        .icon(IconName::ArrowRight)
                        .accessibility_label("Return to open tab")
                        .tooltip("Return to open tab")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(e) = this.code_workspace() {
                                e.update(cx, |e, cx| {
                                    e.show_tree = false;
                                    cx.notify();
                                });
                            }
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn project_close_repository_tab(&mut self, cx: &mut Context<Self>) {
        if self.git.tab <= 1 {
            return;
        }
        let Some(editor) = self.code_workspace() else {
            return;
        };
        let reviewing = editor.read(cx).reviewing;
        let next = editor.read(cx).navigator.documents.reviews.last().cloned();
        self.git_navigate(0, cx);
        editor.update(cx, |e, cx| {
            if e.navigator.panel == 3 {
                e.navigator.panel = 1;
            }
            if reviewing && let Some(document) = next {
                e.review_document(document, cx);
            } else {
                e.reviewing = false;
                cx.notify();
            }
        });
        cx.notify();
    }
    fn project_review_tabs(&self, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<usize>) {
        let Some(editor) = self.code_workspace() else {
            return (Vec::new(), None);
        };
        let state = editor.read(cx);
        let active = state.navigator.documents.active.clone();
        let reviewing = state.reviewing;
        let documents = state.navigator.documents.reviews.clone();
        let mut tabs = Vec::new();
        let mut selected_index = None;
        if self.git.tab > 1 {
            if reviewing {
                selected_index = Some(0);
            }
            let title = match self.git.tab {
                3 => "Stashes",
                4 => "Tags",
                6 => "Remotes",
                _ => "Repository",
            };
            tabs.push(
                document_tab_surface("project-repository-tab-surface", reviewing, cx)
                    .child(
                        crate::controls::compact_button("project-repository-tab")
                            .text()
                            .label(title)
                            .text_color(if reviewing {
                                cx.theme().foreground
                            } else {
                                cx.theme().muted_foreground
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.project_show_review(cx))),
                    )
                    .child(
                        crate::controls::compact_icon_button("project-close-repository")
                            .ghost()
                            .icon(IconName::X)
                            .accessibility_label(format!("Close {title}"))
                            .tooltip("Close tab")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if !window.has_active_dialog(cx) {
                                    this.project_close_repository_tab(cx);
                                }
                            })),
                    )
                    .into_any_element(),
            );
        }
        for (index, document) in documents.into_iter().enumerate() {
            let title = document.title();
            let selected = reviewing && self.git.tab <= 1 && active.as_ref() == Some(&document.key);
            if selected {
                selected_index = Some(tabs.len());
            }
            let owner = editor.clone();
            let close_owner = editor.clone();
            let keep_owner = editor.clone();
            let key = document.key.clone();
            let keep_key = key.clone();
            let preview = document.preview;
            tabs.push(
                document_tab_surface(("project-review-tab-surface", index), selected, cx)
                    .child(
                        crate::controls::compact_button(("project-review-tab", index))
                            .text()
                            .cursor_default()
                            .text_color(if selected {
                                cx.theme().foreground
                            } else {
                                cx.theme().muted_foreground
                            })
                            .rounded_none()
                            .label(title.clone())
                            .accessibility_label(format!("Review {title}"))
                            .when(preview, |button| button.italic())
                            .on_click(move |event, _, cx| {
                                owner.update(cx, |e, cx| {
                                    let mut item = document.clone();
                                    if event.click_count() > 1 {
                                        item.preview = false;
                                    }
                                    e.review_document(item, cx);
                                })
                            }),
                    )
                    .when(preview && selected, |view| {
                        view.child(
                            crate::controls::compact_icon_button(("project-keep-review", index))
                                .ghost()
                                .icon(IconName::Pin)
                                .accessibility_label(format!("Keep {title} open"))
                                .tooltip("Keep open")
                                .on_click(move |_, _, cx| {
                                    keep_owner.update(cx, |e, cx| {
                                        e.navigator.documents.keep(&keep_key);
                                        cx.notify();
                                    })
                                }),
                        )
                    })
                    .child(
                        crate::controls::compact_icon_button(("project-close-review", index))
                            .ghost()
                            .icon(IconName::X)
                            .accessibility_label(format!("Close {title}"))
                            .tooltip("Close tab")
                            .on_click(move |_, _, cx| {
                                close_owner.update(cx, |e, cx| e.close_review(&key, cx))
                            }),
                    )
                    .into_any_element(),
            );
        }
        (tabs, selected_index)
    }
    pub(super) fn project_open_document(
        &mut self,
        document: project_documents::ReviewDocument,
        cx: &mut Context<Self>,
    ) {
        use project_documents::ReviewKey;
        let tab = match &document.key {
            ReviewKey::File { .. } => 0,
            ReviewKey::Commit(_) => 1,
        };
        // Only opening a document changes Git's active comparison. Browsing a
        // collection never changes this model or cancels its in-flight reads.
        let unchanged = match &document.key {
            ReviewKey::File { path, staged } => {
                self.git.tab == 0
                    && text(&self.git.selected["path"], "display") == *path
                    && self.git.diff_staged == *staged
                    && !self.git.diff.is_null()
                    && self.git.page["snapshot"] == document.status["snapshot"]
                    && self.git.error.is_none()
                    && ["git-list", "git-diff", "git-diff-more"].iter().all(|key| {
                        self.resources
                            .get(key)
                            .is_none_or(|read| !read.pending && read.error.is_none())
                    })
            }
            ReviewKey::Commit(oid) => self.git.tab == 1 && self.git.history.oid == *oid,
        };
        if unchanged {
            self.project_show_review(cx);
            return;
        }
        self.git_navigate(tab, cx);
        if let Some(editor) = self.code_workspace() {
            editor.update(cx, |e, _| {
                if e.navigator.panel == 3 {
                    e.navigator.panel = tab + 1;
                }
            });
        }
        self.git.selected = document.entry.clone();
        self.git.detail_open = true;
        match document.key {
            ReviewKey::File { path, staged } => {
                // A filtered navigator is not evidence that a retained file vanished.
                // Retain its snapshot; the backend rejects stale comparisons safely.
                self.git.page = (*document.status).clone();
                if let Some(editor) = self.code_workspace() {
                    let page = editor.read(cx).navigator.collections[0].page().clone();
                    if let Some(entry) = page["entries"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|entry| text(&entry["path"], "display") == path)
                    {
                        self.git.page = (*page).clone();
                        self.git.selected = entry.clone();
                    }
                }
                // This navigator has already obtained the exact status page
                // for the selected entry. Retire a suspended/failed legacy list
                // read so it cannot disable a fresh diff or overwrite it later.
                self.resources.invalidate("git-list");
                self.resources.invalidate("git-more");
                if let Some(generation) = self.resources.begin("git-list") {
                    self.resources
                        .complete("git-list", generation, Ok(self.git.page.clone()));
                }
                self.git_select_side(staged);
            }
            ReviewKey::Commit(_) => self.git_history_select(),
        }
        cx.notify();
    }
    fn project_review_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.git.tab == 0 {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .min_w_0()
                .child(self.git_integration_view(cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(self.git_diff_view(window, cx)),
                )
                .into_any_element();
        }
        let detail = if self.git.tab == 6 {
            self.git_remote_view(cx)
        } else {
            self.git_history_view(window, cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .child(detail)
            .into_any_element()
    }

    pub(super) fn project_workspace_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = self.code_workspace().expect("workspace exists");
        if self.git.tab == 0 && editor.read(cx).reviewing {
            editor.update(cx, |e, cx| {
                if e.navigator.documents.reconcile_file(
                    self.git.selected.clone(),
                    self.git.page.clone(),
                    self.git.diff_staged,
                ) {
                    cx.notify();
                }
            });
        }
        let header = self.project_panel_header(cx);
        let tabs = self.project_review_tabs(cx);
        let panel = editor.read(cx).navigator.panel;
        let changes = if matches!(panel, 1 | 2) {
            let collection = editor.update(cx, |e, cx| e.collection_view(cx));
            div()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .child(div().flex_1().min_h_0().child(collection))
                .when(panel == 1, |v| v.child(self.git_commit_composer(cx)))
                .into_any_element()
        } else {
            self.git_workspace_sidebar(cx)
        };
        let review = self.project_review_content(window, cx);
        let content = editor.update(cx, |e, cx| e.workspace(header, tabs, changes, review, cx));
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(git_layout::toolbar(
                [
                    self.git_project_picker(cx),
                    self.checkout_picker_trigger(cx),
                    self.git_branch_picker_trigger(cx),
                ],
                [self.git_sync_control(cx), self.git_tools_menu(cx)],
                self.content_width,
                cx,
            ))
            .child(self.status("repository", cx))
            .when_some(
                self.git.error.clone().filter(|_| editor.read(cx).reviewing),
                |v, e| v.child(div().px_3().text_color(cx.theme().danger).child(e)),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(content))
            .child(self.git_log_panel(window, cx))
            .child(
                crate::controls::status_bar(cx)
                    // View buttons carry their own 8px inset; their icons
                    // align with the text of every other page's status bar.
                    .px(px(crate::controls::PAGE_PADDING - 8.))
                    .left(
                        status_view_button(
                            "project-status-files",
                            IconName::Folder,
                            "Files",
                            editor.read(cx).files && editor.read(cx).show_tree,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.project_choose_panel(0, window, cx)
                        })),
                    )
                    .left(
                        status_view_button(
                            "project-status-git",
                            IconName::GitBranch,
                            "Git",
                            !editor.read(cx).files && editor.read(cx).show_tree,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.project_choose_panel(1, window, cx)
                        })),
                    )
                    .left(
                        status_view_button(
                            "project-status-terminal",
                            IconName::Terminal,
                            "Terminal",
                            editor.read(cx).terminal_visible,
                            cx,
                        )
                        .disabled(!editor.read(cx).available && !editor.read(cx).terminal_visible)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(editor) = this.code_workspace() {
                                editor.update(cx, |e, cx| e.toggle_terminal(window, cx));
                            }
                        })),
                    )
                    .left(div().truncate().child(if editor.read(cx).reviewing {
                        self.git_read_feedback()
                    } else {
                        "".into()
                    }))
                    .right(
                        crate::controls::compact_button("project-log")
                            .ghost()
                            .icon(IconName::ScrollText)
                            .label("Log")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.git_log.open = !this.git_log.open;
                                if this.git_log.open {
                                    this.git_log_follow_tail();
                                } else {
                                    this.git_log_close(window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}
