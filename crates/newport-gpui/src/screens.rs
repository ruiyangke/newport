use super::*;
impl Newport {
    pub(super) fn screen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // App preferences remain usable while unrelated server reads are unavailable.
        if self.page == Page::Settings {
            return self.settings(cx);
        }
        // A refresh after a mutation keeps showing the accepted snapshot; only a
        // first read with nothing to show replaces the page with a skeleton.
        if (self.initial_loading("snapshot") && self.snapshot.is_null())
            || (!matches!(
                self.page,
                Page::Overview
                    | Page::Ports
                    | Page::Projects
                    | Page::Services
                    | Page::Containers
                    | Page::Files
                    | Page::RemoteApps
            ) && self.initial_loading("page"))
        {
            return loading::page(self.page, self.content_width);
        }
        if self.snapshot.is_null() {
            return empty::pane(
                IconName::ServerOff,
                "Connection unavailable",
                "Newport couldn’t load your servers. Try reconnecting.",
                cx,
            )
            .child(
                crate::controls::button("empty-reconnect")
                    .label("Reconnect")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.request("snapshot", "snapshot", json!({}));
                        cx.notify();
                    })),
            )
            .into_any_element();
        }
        if self.welcome_visible() {
            return self.welcome(cx);
        }
        if self.server.is_empty() && self.page != Page::Settings {
            return empty::pane(
                IconName::Server,
                "Add your first server",
                "Connect over SSH to manage files, terminals, and services in one place.",
                cx,
            )
            .child(
                crate::controls::button("empty-add-server")
                    .label("Add server")
                    .icon(IconName::Plus)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.edit_server(None, window, cx)),
                    ),
            )
            .into_any_element();
        }
        match self.page {
            Page::Overview => self.overview(cx),
            Page::Ports => self.ports(cx),
            Page::Projects => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(self.git_sync_feedback(cx))
                .child(self.projects(window, cx))
                .into_any_element(),
            Page::Services => self.services_screen(),
            Page::Containers => self.containers_screen(),
            Page::Terminal => self
                .terminals
                .get(&self.server)
                .map(|view| view.clone().into_any_element())
                .unwrap_or_else(|| {
                    empty::pane(
                        IconName::Terminal,
                        "Terminal unavailable",
                        "Select a server to open its terminal.",
                        cx,
                    )
                    .into_any_element()
                }),
            Page::Files => self.files(window, cx),
            Page::Integration => self.integration(cx),
            Page::RemoteApps => self.remote_apps(window, cx),
            Page::Settings => self.settings(cx),
        }
    }
}
