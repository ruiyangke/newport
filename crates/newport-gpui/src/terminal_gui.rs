//! Read-only setup readiness. The backend owns display creation and lifetime.
use gpui_kit::App;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

pub(crate) struct Readiness {
    result: RefCell<Option<Result<bool, String>>>,
    started: Instant,
}
#[cfg(test)]
pub(crate) struct TestProbe(pub Result<bool, String>);
#[cfg(test)]
impl gpui_kit::Global for TestProbe {}
impl Readiness {
    pub fn start(server: String, cx: &mut App) -> Rc<Self> {
        let check = Rc::new(Self {
            result: RefCell::new(None),
            started: Instant::now(),
        });
        #[cfg(test)]
        if let Some(probe) = cx.try_global::<TestProbe>() {
            *check.result.borrow_mut() = Some(probe.0.clone());
            return check;
        }
        let weak = Rc::downgrade(&check);
        cx.spawn(async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let status = newport_client::backend::Backend::discover().call_with_timeout(
                        "remote_apps_status",
                        serde_json::json!({"id":server,"refresh":true}),
                        Duration::from_secs(15),
                    )?;
                    Ok::<_, String>(crate::terminal_setup::setup_ready(&status))
                })
                .await;
            if let Some(check) = weak.upgrade() {
                *check.result.borrow_mut() = Some(result);
            }
        })
        .detach();
        check
    }
    pub fn poll(&self) -> Option<Result<bool, String>> {
        self.result.borrow().clone().or_else(|| {
            (self.started.elapsed() > Duration::from_secs(20)).then(|| {
                Err("GUI setup check timed out. Check the server connection and retry.".into())
            })
        })
    }
}
