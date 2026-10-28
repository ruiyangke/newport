//! One lazy native presenter per backend-owned display. The idle watcher keeps
//! no client, renderer, frame timer or native window alive.
use crate::graphical::Registry;
use gpui_kit::gpui::App;
use newport_remote_provider_gpui::native_session::{
    self, NativeSession, PlacementMemory, SharedOptions,
};
use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Weak},
};

pub fn start(registry: Registry, cx: &mut App) {
    let mut changes = registry.observe_presentation();
    cx.spawn(async move |cx| {
        let mut presenters = BTreeMap::<String, NativeSession>::new();
        // Idle presentation owns no client. Retain only bounded per-window
        // positions while this exact backend session still exists.
        let mut placements =
            BTreeMap::<String, (Weak<newport_remote_client::RemoteSession>, PlacementMemory)>::new(
            );
        loop {
            // Mark before reconciling: activity or detachment during the update
            // must schedule another pass, including when window counts match.
            changes.borrow_and_update();
            if registry.is_closed() {
                break;
            }
            cx.update(|cx| {
                presenters.retain(|_, presenter| presenter.is_presenting());
                placements.retain(|_, (owner, _)| owner.strong_count() > 0);
                for candidate in registry.presentation_candidates() {
                    if presenters.contains_key(&candidate.key) {
                        continue;
                    }
                    let detached = registry.clone();
                    let clipboard = candidate.clipboard_presenter();
                    let placement = placements
                        .entry(candidate.key.clone())
                        .or_insert_with(|| {
                            (
                                Arc::downgrade(&candidate.connection),
                                PlacementMemory::default(),
                            )
                        })
                        .1
                        .clone();
                    let options = SharedOptions {
                        placement,
                        presentation:
                            newport_remote_provider_gpui::settings::PresentationSettings {
                                hud: candidate.hud,
                                trace: cfg!(debug_assertions)
                                    && std::env::var_os("NEWPORT_REMOTE_TRACE")
                                        .is_some_and(|value| value == "1"),
                            },
                        on_focus: clipboard.map(|presenter| {
                            Rc::new(move |focused| {
                                presenter.set_focused(focused);
                            }) as Rc<dyn Fn(bool)>
                        }),
                        // The provider calls this after releasing its claim.
                        // A window arriving after its idle snapshot therefore
                        // causes a fresh attachment to the same client.
                        on_detached: Rc::new(move |_, _| detached.recheck_presentation()),
                    };
                    match native_session::present_shared(candidate.connection.clone(), options, cx)
                    {
                        Ok(presenter) => {
                            presenters.insert(candidate.key, presenter);
                        }
                        Err(error) => {
                            if candidate.connection.control().state()
                                == newport_remote_client::SessionState::Running
                            {
                                eprintln!("Cannot present remote display: {error}");
                                candidate
                                    .connection
                                    .abort(format!("Cannot present remote display: {error}"));
                            }
                        }
                    }
                }
            });
            if changes.changed().await.is_err() {
                break;
            }
        }
    })
    .detach();
}
