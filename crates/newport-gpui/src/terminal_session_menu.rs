//! Session commands capture a stable tab ID, never the active index at invocation.
use crate::terminal_workspace::Workspace;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{App, WeakEntity};

pub(super) fn menu(menu: PopupMenu, view: WeakEntity<Workspace>, cx: &App) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let Some((id, _, _, _)) = entity.read(cx).menu_state(cx) else {
        return menu;
    };
    menu_for_tab(menu, view, id, cx)
}

pub(super) fn menu_for_tab(
    menu: PopupMenu,
    view: WeakEntity<Workspace>,
    id: usize,
    cx: &App,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let Some((id, custom, active, can_close)) = entity.read(cx).tab_menu_state(id, cx) else {
        return menu;
    };
    let rename = view.clone();
    let mut menu = menu.item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
        let _ = rename.update(cx, |workspace, cx| workspace.rename(id, window, cx));
    }));
    if custom {
        let reset = view.clone();
        menu = menu.item(PopupMenuItem::new("Reset name").on_click(move |_, _, cx| {
            let _ = reset.update(cx, |workspace, cx| workspace.reset_name(id, cx));
        }));
    }
    let reconnect = view.clone();
    let disconnect = view.clone();
    menu.separator()
        .item(
            PopupMenuItem::new("Reconnect")
                .disabled(active)
                .on_click(move |_, window, cx| {
                    let _ =
                        reconnect.update(cx, |workspace, cx| workspace.reconnect(id, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("Disconnect")
                .disabled(!active)
                .on_click(move |_, _, cx| {
                    let _ = disconnect.update(cx, |workspace, cx| workspace.disconnect(id, cx));
                }),
        )
        .separator()
        .item(PopupMenuItem::new("Close").disabled(!can_close).on_click(
            move |event, window, cx| {
                let _ = view.update(cx, |workspace, cx| {
                    workspace.close(id, event.is_keyboard(), window, cx)
                });
            },
        ))
}
