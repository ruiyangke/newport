//! Why remote apps last opened in Newport although the setting asks for
//! separate apps, so the setting can say so.
use std::sync::Mutex;

static UNAVAILABLE: Mutex<Option<String>> = Mutex::new(None);

pub fn report(unavailable: Option<String>) {
    *UNAVAILABLE.lock().unwrap() = unavailable;
}

pub(crate) fn unavailable() -> Option<String> {
    UNAVAILABLE.lock().unwrap().clone()
}
