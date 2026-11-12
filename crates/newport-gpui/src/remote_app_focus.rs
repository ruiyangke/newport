//! Which remote app comes to the front. The presenters showing the apps'
//! windows decide; the app installs the hooks that reach them.
use gpui_kit::{App, Global};
use std::rc::Rc;

/// Bring the open app to the front. Whether this computer shows it.
pub type BringForward = Rc<dyn Fn(&str, &str, &mut App) -> bool>;
/// The user opened the app just now: its next window comes to the front,
/// even one a copy already running shows.
pub type Launching = Rc<dyn Fn(&str, &str, &mut App)>;

/// What the presenters do for a remote app, by server id and catalogue id.
pub struct Hooks {
    pub bring_forward: BringForward,
    pub launching: Launching,
}

struct Installed(Rc<Hooks>);

impl Global for Installed {}

/// Set once the presenters run.
pub fn install(hooks: Hooks, cx: &mut App) {
    cx.set_global(Installed(Rc::new(hooks)));
}

fn hooks(cx: &App) -> Option<Rc<Hooks>> {
    cx.try_global::<Installed>()
        .map(|installed| installed.0.clone())
}

pub(crate) fn bring_forward(server: &str, app: &str, cx: &mut App) -> bool {
    hooks(cx).is_some_and(|hooks| (hooks.bring_forward)(server, app, cx))
}

pub(crate) fn launching(server: &str, app: &str, cx: &mut App) {
    if let Some(hooks) = hooks(cx) {
        (hooks.launching)(server, app, cx);
    }
}
