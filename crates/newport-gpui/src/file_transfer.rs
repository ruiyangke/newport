//! File transfer presentation. The shared client owns requests and their lifetime.
use super::*;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::progress::Progress as ProgressBar;
pub use newport_client::file_transfer::Transfer;
use newport_client::file_transfer::{Direction, Progress};
use std::path::PathBuf;

#[cfg(test)]
#[path = "file_transfer_state_tests.rs"]
mod state_tests;

pub(super) struct FileChooser {
    id: uuid::Uuid,
    target: files::FileTarget,
    direction: Direction,
    backend: backend::Backend,
}

fn choose_local_file(
    direction: Direction,
    remote: &str,
    cx: &App,
) -> Task<Result<Option<PathBuf>, String>> {
    match direction {
        Direction::Upload => {
            let reply = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Upload file".into()),
            });
            cx.background_executor().spawn(async move {
                let paths = reply
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                match paths {
                    None => Ok(None),
                    Some(mut paths) if paths.len() == 1 => Ok(paths.pop()),
                    Some(_) => Err("Choose one file to upload".into()),
                }
            })
        }
        Direction::Download => {
            // Downloads go to Downloads, as in other Mac apps.
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let directory = Some(home.join("Downloads"))
                .filter(|downloads| downloads.is_dir())
                .unwrap_or(home);
            let name = remote.rsplit('/').next().filter(|name| !name.is_empty());
            let reply = cx.prompt_for_new_path(&directory, name);
            cx.background_executor().spawn(async move {
                reply
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())
            })
        }
    }
}

fn progress_label(progress: &Progress) -> String {
    let done = bytes(&json!(progress.completed));
    progress.total.map_or_else(
        || format!("{done} transferred"),
        |total| format!("{done} / {}", bytes(&json!(total))),
    )
}
fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Download => "download",
        Direction::Upload => "upload",
    }
}
fn success_message(direction: Direction) -> &'static str {
    match direction {
        Direction::Download => "File downloaded.",
        Direction::Upload => "File uploaded.",
    }
}

fn progress_percent(progress: Option<&Progress>) -> Option<f32> {
    let progress = progress?;
    let total = progress.total.filter(|total| *total > 0)?;
    Some((progress.completed.min(total) as f64 / total as f64 * 100.) as f32)
}

fn transfer_strip(
    summary: String,
    progress: Option<&Progress>,
    cancel: AnyElement,
    error: Option<&str>,
    cx: &App,
) -> AnyElement {
    let percent = progress_percent(progress);
    let detail = progress
        .map(progress_label)
        .unwrap_or_else(|| "Preparing transfer…".into());
    div()
        .id("file-transfer-status")
        .debug_selector(|| "file-transfer-status".into())
        .role(Role::Status)
        .aria_label(summary.clone())
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(16.))
        .py(px(10.))
        .flex_shrink_0()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .font_weight(FontWeight::MEDIUM)
                                .text_ellipsis()
                                .child(summary),
                        )
                        .child(
                            div()
                                .id("file-transfer-progress")
                                .role(Role::Label)
                                .aria_label(detail.clone())
                                .text_size(px(crate::ui::FONT_SIZE))
                                .text_color(cx.theme().muted_foreground)
                                .child(detail)
                                .test_support(),
                        )
                        .child(
                            div()
                                .id("file-transfer-meter")
                                .debug_selector(|| "file-transfer-meter".into())
                                .h(px(5.))
                                .w_full()
                                .child(
                                    ProgressBar::new("file-transfer-bar")
                                        .accessibility_label("File transfer")
                                        .color(cx.theme().primary)
                                        .with_size(px(5.))
                                        .value(percent.unwrap_or(0.))
                                        .loading(percent.is_none()),
                                ),
                        ),
                )
                .child(cancel),
        )
        .when_some(error, |view, error| {
            view.child(
                div()
                    .id("file-cancel-error")
                    .role(Role::Alert)
                    .aria_label(error.to_owned())
                    .text_color(cx.theme().danger)
                    .text_size(px(crate::ui::FONT_SIZE))
                    .child(error.to_owned())
                    .test_support(),
            )
        })
        .into_any_element()
}

impl Newport {
    pub(super) fn file_transfer_busy(&self) -> bool {
        self.file_chooser.is_some() || self.file_transfer.is_some()
    }

    pub(super) fn start_file_transfer(
        &mut self,
        target: &files::FileTarget,
        direction: Direction,
        cx: &mut Context<Self>,
    ) {
        if self.file_transfer_busy()
            || !self.file_target_current(target)
            || !target.supports_transfer(direction)
        {
            return;
        }
        let id = uuid::Uuid::new_v4();
        self.file_chooser = Some(FileChooser {
            id,
            target: target.clone(),
            direction,
            backend: backend::Backend::discover(),
        });
        let choice = choose_local_file(direction, &target.path, cx);
        cx.spawn(async move |owner, cx| {
            let choice = choice.await;
            let _ = owner.update(cx, |this, cx| {
                this.finish_file_choice(id, choice);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_file_choice(&mut self, id: uuid::Uuid, choice: Result<Option<PathBuf>, String>) {
        if !self
            .file_chooser
            .as_ref()
            .is_some_and(|chooser| chooser.id == id)
        {
            return;
        }
        let chooser = self.file_chooser.take().unwrap();
        let local = match choice {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => {
                self.notification = Some(Notification::error(error));
                return;
            }
        };
        let current = self.snapshot["config"]["servers"]
            .as_array()
            .and_then(|servers| {
                servers
                    .iter()
                    .find(|server| server["id"] == chooser.target.server)
            })
            .map(|server| terminal_scope::connection(server, &self.snapshot));
        if !self.configuration_available() || current.as_ref() != Some(&chooser.target.connection) {
            self.notification = Some(Notification::error(
                "The connection changed. Choose the file again.",
            ));
            return;
        }
        self.file_transfer = Some(Transfer::start(
            chooser.backend,
            chooser.target.server,
            chooser.target.path,
            chooser.direction,
            chooser.target.connection,
            local,
        ));
    }

    pub(super) fn poll_file_transfer(&mut self, _cx: &mut Context<Self>) -> bool {
        let configuration_available = self.configuration_available();
        // A profile outage is not retirement. Leave the owned worker's result
        // queued until its captured connection can be revalidated, otherwise a
        // successful upload/download receipt would be consumed and discarded.
        if !configuration_available {
            return false;
        }
        let Some(transfer) = self.file_transfer.as_mut() else {
            return false;
        };
        // Page/server navigation leaves the original transfer running. An edited,
        // deleted or restarted connection retires it before consuming its reply.
        let current = self.snapshot["config"]["servers"]
            .as_array()
            .and_then(|servers| servers.iter().find(|s| s["id"] == transfer.server()))
            .map(|server| terminal_scope::connection(server, &self.snapshot));
        let obsolete = configuration_available && current.as_ref() != Some(transfer.connection());
        let newly_retired = obsolete && !transfer.retired();
        if newly_retired {
            transfer.retire();
        }
        let update = transfer.poll();
        if !transfer.finished() {
            return self.page == Page::Files
                && self.window_visible
                && (newly_retired || update.changed);
        }
        let current_scope = configuration_available
            && !transfer.retired()
            && current.as_ref() == Some(transfer.connection());
        let uploaded = current_scope
            && transfer.direction() == Direction::Upload
            && update.completion.as_ref().is_some_and(Result::is_ok);
        let refresh = uploaded
            && self.page == Page::Files
            && self.server == transfer.server()
            && self.file_browsers.get(&self.server).is_some_and(|browser| {
                browser.directory == transfer.path()
                    && (!browser.reads.listing_pending()
                        || browser.reads.attempted_path() == Some(transfer.path()))
            });
        let destination = refresh.then(|| transfer.path().to_owned());
        if current_scope && let Some(result) = update.completion {
            self.notification = match result {
                Ok(_) => Some(Notification::success(success_message(transfer.direction()))),
                Err(error) if error == "File operation cancelled" => None,
                Err(error) => Some(Notification::error(error)),
            };
        }
        self.file_transfer = None;
        if let Some(destination) = destination {
            // Supersede and cancel the pre-upload read, retaining visible rows/filter.
            self.close_file_preview();
            if let Some(browser) = self.file_browsers.get_mut(&self.server) {
                browser.reads.navigate(destination.clone());
                browser.requested = destination;
            }
        }
        true
    }

    fn cancel_file_transfer(&mut self, operation: &str) -> bool {
        self.file_transfer
            .as_mut()
            .is_some_and(|transfer| transfer.operation() == operation && transfer.cancel())
    }

    pub(super) fn transfer_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(transfer) = &self.file_transfer else {
            return div().into_any_element();
        };
        let operation = transfer.operation().to_owned();
        let state = if transfer.cancelling() {
            "Cancelling"
        } else if transfer.progress().is_none() {
            "Preparing"
        } else {
            "Transferring"
        };
        let summary = format!(
            "{state} {}: {}",
            direction_name(transfer.direction()),
            transfer
                .progress()
                .map(|p| p.name.as_str())
                .unwrap_or(transfer.name())
        );
        transfer_strip(
            summary,
            transfer.progress(),
            crate::controls::button("cancel-file-transfer")
                .label(if transfer.cancel_error().is_some() {
                    "Try again"
                } else {
                    "Cancel"
                })
                .accessibility_label(if transfer.cancel_error().is_some() {
                    "Try cancelling the transfer again"
                } else {
                    "Cancel transfer"
                })
                .loading(transfer.cancel_pending())
                .disabled(transfer.cancelling() || transfer.retired())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.cancel_file_transfer(&operation) {
                        cx.notify();
                    }
                }))
                .into_any_element(),
            transfer.cancel_error(),
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{progress_label, progress_percent, transfer_strip};
    use gpui_kit::{Context, IntoElement, Render, Role, Window, div, prelude::*, px};
    use newport_client::file_transfer::Progress;

    #[gpui_kit::test]
    async fn native_picker_returns_paths_and_cancellation_without_a_backend(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::{Direction, choose_local_file};
        let upload = cx.update(|cx| choose_local_file(Direction::Upload, "/remote", cx));
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(options.files && !options.directories && !options.multiple);
            Some(vec!["/tmp/資料 file.txt".into()])
        });
        assert_eq!(upload.await.unwrap(), Some("/tmp/資料 file.txt".into()));
        let upload = cx.update(|cx| choose_local_file(Direction::Upload, "/remote", cx));
        cx.simulate_path_prompt_response(|_| None);
        assert_eq!(upload.await.unwrap(), None);

        let download =
            cx.update(|cx| choose_local_file(Direction::Download, "/remote/file.txt", cx));
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some("/tmp/download.txt".into()));
        assert_eq!(download.await.unwrap(), Some("/tmp/download.txt".into()));
        let download =
            cx.update(|cx| choose_local_file(Direction::Download, "/remote/file.txt", cx));
        cx.simulate_new_path_selection(|_| None);
        assert_eq!(download.await.unwrap(), None);
    }

    #[gpui_kit::test]
    fn chooser_reply_is_consumed_once_and_cannot_use_a_changed_connection(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::*;
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (owner, cx) = cx.add_window_view(crate::page_perf_tests::owner);
        owner.update(cx, |this, _| {
            let id = uuid::Uuid::new_v4();
            let target = files::FileTarget::from_listing(
                "test".into(),
                json!(["old connection"]),
                &json!({"path":"/remote"}),
                "/remote",
            )
            .unwrap();
            let chooser = || FileChooser {
                id,
                target: target.clone(),
                direction: Direction::Upload,
                backend: backend::Backend::at_path("/nonexistent-newport-test.sock".into()),
            };
            this.file_chooser = Some(chooser());
            assert!(this.file_transfer_busy());
            this.finish_file_choice(uuid::Uuid::new_v4(), Ok(None));
            assert!(
                this.file_chooser.is_some(),
                "an obsolete reply must not clear the active chooser"
            );
            this.finish_file_choice(id, Ok(None));
            assert!(!this.file_transfer_busy());
            this.finish_file_choice(id, Ok(Some("/tmp/late.txt".into())));
            assert!(
                !this.file_transfer_busy(),
                "a cancelled choice cannot be replayed"
            );
            this.file_chooser = Some(chooser());
            this.finish_file_choice(id, Ok(Some("/tmp/selected.txt".into())));
            assert!(
                !this.file_transfer_busy(),
                "stale connection must never dispatch a transfer"
            );
            assert!(this.notification.is_some());

            // Ordinary page/server navigation must not retarget the chosen file.
            let server = this.snapshot["config"]["servers"][0].clone();
            let connection = terminal_scope::connection(&server, &this.snapshot);
            let original_server = server["id"].as_str().unwrap().to_owned();
            let target = files::FileTarget::from_listing(
                original_server.clone(),
                connection,
                &json!({"path":"/original-folder"}),
                "/original-folder",
            )
            .unwrap();
            this.file_chooser = Some(FileChooser {
                target,
                ..chooser()
            });
            this.page = Page::Projects;
            this.server = "another-server".into();
            this.finish_file_choice(id, Ok(Some("/tmp/selected.txt".into())));
            let transfer = this
                .file_transfer
                .as_ref()
                .expect("valid captured scope should transfer");
            assert_eq!(transfer.server(), original_server);
            assert_eq!(transfer.path(), "/original-folder");
            let operation = transfer.operation().to_owned();
            this.finish_file_choice(id, Ok(Some("/tmp/duplicate.txt".into())));
            assert_eq!(this.file_transfer.as_ref().unwrap().operation(), operation);
        });
    }

    #[test]
    fn progress_labels_distinguish_unknown_totals_and_empty_files() {
        assert_eq!(
            progress_label(&Progress {
                name: "a".into(),
                completed: 1024,
                total: None
            }),
            "1.0 KiB transferred"
        );
        assert_eq!(
            progress_label(&Progress {
                name: "a".into(),
                completed: 0,
                total: Some(0)
            }),
            "0 B / 0 B"
        );
    }

    #[test]
    fn transfer_percentage_handles_unknown_zero_and_large_totals() {
        assert_eq!(progress_percent(None), None);
        for (completed, total, expected) in [
            (12, None, None),
            (0, Some(0), None),
            (0, Some(100), Some(0.)),
            (25, Some(100), Some(25.)),
            (120, Some(100), Some(100.)),
            (u64::MAX, Some(u64::MAX), Some(100.)),
        ] {
            assert_eq!(
                progress_percent(Some(&Progress {
                    name: "file".into(),
                    completed,
                    total
                })),
                expected
            );
        }
    }

    struct TransferHost {
        width: f32,
        progress: Option<Progress>,
        error: Option<String>,
    }
    impl Render for TransferHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(self.width))
                .h(px(420.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("toolbar")
                        .debug_selector(|| "transfer-test-toolbar".into())
                        .h(px(48.))
                        .flex_shrink_0(),
                )
                .child(div().flex_1().min_h_0())
                .child(transfer_strip(
                    "Downloading a-long-unicode-file-name-文件.txt".into(),
                    self.progress.as_ref(),
                    crate::controls::button("cancel")
                        .label("Cancel")
                        .into_any_element(),
                    self.error.as_deref(),
                    cx,
                ))
        }
    }

    #[gpui_kit::test]
    fn transfer_strip_stays_at_bottom_with_visible_cancel_at_compact_widths(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let (host, cx) = cx.add_window_view(|_, _| TransferHost {
            width: 760.,
            progress: None,
            error: None,
        });
        for width in [760., 320.] {
            for progress in [
                None,
                Some(Progress {
                    name: "file.txt".into(),
                    completed: 1024,
                    total: Some(4096),
                }),
            ] {
                host.update(cx, |host, cx| {
                    host.width = width;
                    host.progress = progress;
                    cx.notify();
                });
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear(cx));
                let strip = cx.debug_bounds("file-transfer-status").unwrap();
                let bar = cx.debug_bounds("file-transfer-meter").unwrap();
                assert_eq!(strip.bottom(), px(420.));
                assert_eq!(
                    cx.debug_bounds("transfer-test-toolbar").unwrap().top(),
                    px(0.)
                );
                assert_eq!(bar.size.height, px(5.));
                assert!(
                    bar.size.width > px(100.)
                        && bar.left() >= strip.left()
                        && bar.right() <= strip.right()
                );
                cx.update(|window, cx| {
                    let nodes = gpui_kit::base::test_support::snapshots(window);
                    let cancel = nodes
                        .iter()
                        .find(|node| node.label() == Some("Cancel"))
                        .unwrap();
                    assert!(cancel.bounds().left() > bar.right());
                    assert!(cancel.bounds().right() <= strip.right());
                    assert!(cancel.bounds().bottom() <= strip.bottom());
                    let expected = if host.read(cx).progress.is_some() {
                        "1.0 KiB / 4.0 KiB"
                    } else {
                        "Preparing transfer…"
                    };
                    assert!(nodes.iter().any(|node| node.label() == Some(expected)));
                });
            }
        }
        host.update(cx, |host, cx| {
            host.error = Some("Cancellation could not be sent. Try again.".into());
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                gpui_kit::base::test_support::snapshots(window)
                    .iter()
                    .any(|node| node.role() == Some(Role::Alert)
                        && node.label() == Some("Cancellation could not be sent. Try again."))
            );
        });
    }
}
