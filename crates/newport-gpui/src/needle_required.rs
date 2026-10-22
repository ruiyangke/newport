//! Everything that runs on Needle shares one state: when a server's Needle is
//! missing or its setup unfinished, each of those pages says so the same way,
//! offers the same action, and asks the server for nothing.
use crate::*;

#[cfg(test)]
#[path = "needle_required_tests.rs"]
mod tests;

const TITLE: &str = "Needle isn’t set up";

impl Newport {
    /// Whether the selected server can't run Needle's features: its setup is
    /// unfinished or Needle was found missing. From the last snapshot only.
    pub(super) fn needle_missing(&self) -> bool {
        self.selected_server()
            .is_some_and(|server| self.needs_setup(server))
    }

    /// In place of a page that runs on Needle.
    pub(super) fn needle_required(&self, feature: &str, cx: &mut Context<Self>) -> AnyElement {
        empty::pane(IconName::ServerOff, TITLE, description(feature), cx)
            .child(self.set_up_needle_button(true, cx))
            .into_any_element()
    }

    /// In place of a Needle feature on a page that otherwise works.
    pub(super) fn needle_required_notice(
        &self,
        feature: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::controls::warning_banner("needle-required", TITLE, description(feature), cx)
            .child(self.set_up_needle_button(false, cx))
            .into_any_element()
    }

    fn set_up_needle_button(&self, primary: bool, cx: &mut Context<Self>) -> Button {
        crate::controls::button("set-up-needle")
            .when(primary, |button| button.primary())
            .label("Set up Needle")
            .disabled(self.server_form.is_some() || !self.configuration_available())
            .on_click(cx.listener(|this, _, _, cx| this.set_up_needle(cx)))
    }

    /// Server setup on Overview, from the start: it installs Needle.
    pub(super) fn set_up_needle(&mut self, cx: &mut Context<Self>) {
        self.setup_flow = None;
        self.resources.invalidate_prefix("setup-");
        self.navigate(Page::Overview, cx);
        self.poll_setup();
        cx.notify();
    }
}

fn description(feature: &str) -> String {
    format!(
        "{feature} runs on Needle, Newport’s agent on this server. Terminals, files and port forwarding work without it."
    )
}
