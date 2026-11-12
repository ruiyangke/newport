//! Whether the account's display is running and which viewer owns it.
use super::Invalid;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub running: bool,
    pub viewer: Option<Viewer>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    pub session: String,
    pub computer: Option<String>,
}
impl State {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(viewer) = &self.viewer
            && (!self.running
                || viewer.session.is_empty()
                || viewer.session.len() > 128
                || viewer.session.contains('\0')
                || viewer
                    .computer
                    .as_ref()
                    .is_some_and(|value| value.len() > 1024 || value.contains('\0')))
        {
            return Err(Invalid("stable display viewer"));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_stopped_display_cannot_have_a_viewer() {
        State::default().validate().unwrap();
        let mut state = State {
            running: true,
            viewer: Some(Viewer {
                session: "session".into(),
                computer: None,
            }),
        };
        state.validate().unwrap();
        state.running = false;
        assert!(state.validate().is_err());
    }
}
