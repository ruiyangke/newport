//! Files copied on a server download when they are copied, so a paste on the
//! Mac finds them. Large copies, and copies reaching outside the server
//! user's home folder, ask first; slow downloads and failures show a toast.
//!
//! The question opens in the main window, which comes forward when it is
//! asked: a copy in a remote app leaves that app's windows in front.
use super::*;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// How often a question asked while another app is active checks whether
/// the person has come back to Newport.
const RETURN_CHECK: Duration = Duration::from_millis(250);

type Reply = Box<dyn FnOnce(bool) + Send>;

/// What a server copy would download.
pub struct ClipboardFilesQuestion {
    /// The server's name.
    pub server: String,
    /// Files, folders and links, counted recursively.
    pub items: u64,
    pub bytes: u64,
    pub outside_home: bool,
    /// Why the question is asked besides where the files are and their size.
    pub hidden: bool,
    pub executable: bool,
    pub installer: bool,
    pub large: bool,
    /// The first few top-level names, already cut short.
    pub names: Vec<String>,
    /// How many items were copied at the top level.
    pub top_level: u64,
}

/// How the download of a server copy's files is going.
pub enum ClipboardFilesNotice {
    Copying,
    Finished,
    Failed(String),
    /// A paste on the server got none of the files copied on this Mac.
    UploadFailed(String),
}

/// Identifies the toasts of these downloads.
struct Notice;

struct Question {
    question: ClipboardFilesQuestion,
    reply: Option<Reply>,
    cancelled: Arc<AtomicBool>,
}
impl Question {
    fn live(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire)
    }
    fn answer(mut self, yes: bool) {
        if let Some(reply) = self.reply.take() {
            reply(yes);
        }
    }
}
impl Drop for Question {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            reply(false);
        }
    }
}

#[derive(Default)]
struct Pending {
    /// Only the newest copy can still be current.
    waiting: Option<Question>,
    /// The question in the open dialog, with that dialog.
    open: Option<(Question, gpui_kit::component::DialogHandle)>,
    notices: VecDeque<(String, ClipboardFilesNotice)>,
    /// The main window is being brought forward for the question.
    revealing: bool,
}
impl Global for Pending {}
fn pending(cx: &mut App) -> &mut Pending {
    if !cx.has_global::<Pending>() {
        cx.set_global(Pending::default());
    }
    cx.global_mut::<Pending>()
}

pub(crate) fn request(
    question: ClipboardFilesQuestion,
    reply: Reply,
    cancelled: Arc<AtomicBool>,
    cx: &mut App,
) {
    let state = pending(cx);
    state.waiting = Some(Question {
        question,
        reply: Some(reply),
        cancelled,
    });
    // Questions that arrive together bring the window forward once.
    if !std::mem::replace(&mut state.revealing, true) {
        cx.spawn(async move |cx| {
            let mut attention = false;
            while cx.update(|cx| reveal(&mut attention, cx)) {
                cx.background_executor().timer(RETURN_CHECK).await;
            }
        })
        .detach();
    }
}

/// The window that shows the question: the main window.
fn host(cx: &mut App) -> Option<AnyWindowHandle> {
    cx.windows().into_iter().find(|handle| {
        handle
            .update(cx, |_, window, _| {
                window.root::<gpui_kit::base::Root>().flatten().is_some()
            })
            .unwrap_or(false)
    })
}

/// Brings the main window forward while a question waits in it. A window
/// that is already key is left as it is. While another app is active, the
/// Dock icon asks for attention once and the window comes forward when the
/// person returns to Newport. True to check again later.
fn reveal(attention: &mut bool, cx: &mut App) -> bool {
    let state = pending(cx);
    let asking = state.waiting.as_ref().is_some_and(Question::live)
        || state
            .open
            .as_ref()
            .is_some_and(|(question, _)| question.live());
    let host = if asking { host(cx) } else { None };
    let Some(host) = host else {
        pending(cx).revealing = false;
        return false;
    };
    let active = window_activation::app_is_active(cx);
    let waiting = host
        .update(cx, |_, window, _| {
            if window.is_window_active() {
                false
            } else if active {
                window_activation::bring_forward(window);
                false
            } else {
                if !std::mem::replace(attention, true) {
                    window.request_attention();
                }
                true
            }
        })
        .unwrap_or(false);
    if !waiting {
        pending(cx).revealing = false;
    }
    waiting
}

pub(crate) fn notice(server: String, notice: ClipboardFilesNotice, cx: &mut App) {
    let pending = pending(cx);
    if pending.notices.len() < 32 {
        pending.notices.push_back((server, notice));
    }
}

/// True while a question or notice waits for the window.
pub(crate) fn changed(cx: &App) -> bool {
    cx.try_global::<Pending>().is_some_and(|pending| {
        pending.waiting.is_some()
            || !pending.notices.is_empty()
            || pending
                .open
                .as_ref()
                .is_some_and(|(question, handle)| !question.live() || !handle.is_active())
    })
}

/// Answers the question in the open dialog.
fn answer(yes: bool, window: &mut Window, cx: &mut App) {
    if let Some((question, handle)) = pending(cx).open.take() {
        window.close_dialog_if(handle.focus_handle(), cx);
        question.answer(yes);
    }
}

fn summary(question: &ClipboardFilesQuestion) -> String {
    let items = match question.items {
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    let size = crate::bytes(&serde_json::json!(question.bytes));
    let mut text = format!("{items}, {size}");
    if !question.names.is_empty() {
        let more = question
            .top_level
            .saturating_sub(question.names.len() as u64);
        text.push_str(": ");
        text.push_str(&question.names.join(", "));
        if more > 0 {
            text.push_str(&format!(" and {more} more"));
        }
    }
    text.push('.');
    let reasons: Vec<&str> = [
        (question.outside_home, "files outside your home folder"),
        (question.hidden, "hidden items"),
        (question.executable, "executable files"),
        (question.installer, "an app or installer"),
        (question.large, "a size over 512 MiB"),
    ]
    .into_iter()
    .filter_map(|(applies, reason)| applies.then_some(reason))
    .collect();
    if let Some((last, rest)) = reasons.split_last() {
        text.push_str(" Newport asks because of ");
        if !rest.is_empty() {
            text.push_str(&rest.join(", "));
            text.push_str(" and ");
        }
        text.push_str(last);
        text.push('.');
    }
    text
}

impl Newport {
    pub(super) fn sync_clipboard_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.has_global::<Pending>() {
            return;
        }
        let notices: Vec<_> = pending(cx).notices.drain(..).collect();
        for (server, notice) in notices {
            let key = SharedString::from(match notice {
                ClipboardFilesNotice::UploadFailed(_) => format!("to {server}"),
                _ => server.clone(),
            });
            window.remove_notification1::<Notice>(key.clone(), cx);
            let notification = match notice {
                ClipboardFilesNotice::Finished => continue,
                ClipboardFilesNotice::Copying => {
                    Notification::info(format!("Copying files from {server}.")).autohide(false)
                }
                ClipboardFilesNotice::Failed(error) => {
                    Notification::error(format!("{}.", error.trim_end_matches('.')))
                        .title(format!("Couldn’t copy files from {server}"))
                }
                ClipboardFilesNotice::UploadFailed(error) => {
                    Notification::error(format!("{}.", error.trim_end_matches('.')))
                        .title(format!("Couldn’t copy files to {server}"))
                }
            };
            window.push_notification(notification.id1::<Notice>(key), cx);
        }
        // Expired, replaced by a newer copy, or closed with Escape.
        let state = pending(cx);
        let stale = state
            .open
            .as_ref()
            .is_some_and(|(question, handle)| !question.live() || !handle.is_active());
        if stale || state.waiting.as_ref().is_some_and(Question::live) {
            // A newer copy's question replaces an open one.
            if let Some((question, handle)) = state.open.take() {
                window.close_dialog_if(handle.focus_handle(), cx);
                drop(question);
            }
        }
        let state = pending(cx);
        if state.waiting.as_ref().is_some_and(|q| !q.live()) {
            state.waiting = None;
        }
        if state.waiting.is_none() || window.has_active_dialog(cx) || self.server_form.is_some() {
            return;
        }
        let question = pending(cx).waiting.take().unwrap();
        let title: SharedString = format!("Copy files from {}?", question.question.server).into();
        let detail: SharedString = summary(&question.question).into();
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title(crate::controls::dialog_title(title.clone()))
                .w(px(440.))
                .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                .p(px(24.))
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .pt_3()
                        .child(crate::label(detail.clone()))
                        .child(
                            crate::project_dialog::actions()
                                .child(
                                    crate::controls::form_button("clipboard-files-decline")
                                        .label("Don’t copy")
                                        .on_click(|_, window, cx| answer(false, window, cx)),
                                )
                                .child(
                                    crate::controls::form_button("clipboard-files-copy")
                                        .primary()
                                        .label("Copy")
                                        .on_click(|_, window, cx| answer(true, window, cx)),
                                ),
                        ),
                )
        });
        match window.active_dialog_handle(cx) {
            Some(handle) => pending(cx).open = Some((question, handle)),
            None => drop(question),
        }
        self.modal_open = window.has_active_dialog(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{ClipboardFilesQuestion, RETURN_CHECK, request, summary};
    use crate::page_perf_tests::{frame, owner};
    use gpui_kit::component::WindowExt;
    use gpui_kit::{self as gpui, AppContext, TestAppContext, VisualTestContext, WindowHandle};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    /// The main window, and a remote app's window that is key in front of it.
    fn windows(cx: &mut TestAppContext) -> (WindowHandle<gpui::Empty>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let remote = cx.add_window(|_, _| gpui::Empty);
        let (_, main) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| owner(window, cx));
            gpui::base::Root::new(view, window, cx)
        });
        frame(main);
        focus(remote, main);
        assert!(!main.update(|window, _| window.is_window_active()));
        (remote, main)
    }

    fn focus(remote: WindowHandle<gpui::Empty>, main: &mut VisualTestContext) {
        main.update(|_, cx| {
            remote
                .update(cx, |_, window, _| window.activate_window())
                .unwrap()
        });
        main.run_until_parked();
    }

    /// Asks about a copy; returns its answers and its cancel flag.
    fn ask(main: &mut VisualTestContext) -> (Arc<Mutex<Vec<bool>>>, Arc<AtomicBool>) {
        let answers = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::new(AtomicBool::new(false));
        let record = answers.clone();
        let flag = cancelled.clone();
        main.update(|_, cx| {
            request(
                ClipboardFilesQuestion {
                    server: "dev".into(),
                    items: 1,
                    bytes: 27,
                    outside_home: false,
                    hidden: true,
                    executable: false,
                    installer: false,
                    large: false,
                    names: vec![".env".into()],
                    top_level: 1,
                },
                Box::new(move |yes| record.lock().unwrap().push(yes)),
                flag,
                cx,
            )
        });
        main.run_until_parked();
        (answers, cancelled)
    }

    fn wait(main: &mut VisualTestContext) {
        main.executor().advance_clock(RETURN_CHECK * 8);
        main.run_until_parked();
    }

    #[gpui::test]
    fn a_question_brings_the_main_window_forward_once_per_copy(cx: &mut TestAppContext) {
        let (remote, main) = windows(cx);
        let (answers, _) = ask(main);
        assert!(main.update(|window, _| window.is_window_active()));
        frame(main);
        assert!(main.update(|window, cx| window.has_active_dialog(cx)));
        // Going back to the remote app leaves the question where it is.
        focus(remote, main);
        wait(main);
        frame(main);
        assert!(!main.update(|window, _| window.is_window_active()));
        assert!(main.update(|window, cx| window.has_active_dialog(cx)));
        // A newer copy asks again, in front.
        let (newer, _) = ask(main);
        assert!(main.update(|window, _| window.is_window_active()));
        frame(main);
        assert_eq!(*answers.lock().unwrap(), [false]);
        assert!(newer.lock().unwrap().is_empty());
        assert!(main.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn a_question_waits_for_the_person_to_return_to_newport(cx: &mut TestAppContext) {
        let (remote, main) = windows(cx);
        main.update(|window, _| window.activate_window());
        main.deactivate_window();
        let (answers, _) = ask(main);
        wait(main);
        assert!(!main.update(|window, _| window.is_window_active()));
        focus(remote, main);
        wait(main);
        assert!(main.update(|window, _| window.is_window_active()));
        frame(main);
        assert!(main.update(|window, cx| window.has_active_dialog(cx)));
        assert!(answers.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn an_expired_question_does_not_bring_the_window_forward(cx: &mut TestAppContext) {
        let (remote, main) = windows(cx);
        main.update(|window, _| window.activate_window());
        main.deactivate_window();
        let (_, cancelled) = ask(main);
        cancelled.store(true, Ordering::Release);
        wait(main);
        focus(remote, main);
        wait(main);
        assert!(!main.update(|window, _| window.is_window_active()));
    }

    #[test]
    fn the_question_names_the_size_and_where_the_files_are() {
        let question = |items, bytes, outside_home| ClipboardFilesQuestion {
            server: "dev".into(),
            items,
            bytes,
            outside_home,
            hidden: false,
            executable: false,
            installer: false,
            large: false,
            names: Vec::new(),
            top_level: 0,
        };
        assert_eq!(summary(&question(1, 512, false)), "1 item, 512 B.");
        assert_eq!(
            summary(&question(12, 700 << 20, true)),
            "12 items, 700.0 MiB. Newport asks because of files outside your home folder."
        );
    }

    #[test]
    fn the_question_names_why_it_asks_and_the_first_items() {
        let question = ClipboardFilesQuestion {
            server: "dev".into(),
            items: 40,
            bytes: 600 << 20,
            outside_home: false,
            hidden: true,
            executable: true,
            installer: false,
            large: true,
            names: vec![".env".into(), "tool".into(), "Editor.app".into()],
            top_level: 5,
        };
        assert_eq!(
            summary(&question),
            "40 items, 600.0 MiB: .env, tool, Editor.app and 2 more. Newport asks because of hidden items, executable files and a size over 512 MiB."
        );
        let one = ClipboardFilesQuestion {
            hidden: true,
            executable: false,
            large: false,
            names: vec![".ssh".into()],
            top_level: 1,
            ..question
        };
        assert_eq!(
            summary(&one),
            "40 items, 600.0 MiB: .ssh. Newport asks because of hidden items."
        );
    }
}
