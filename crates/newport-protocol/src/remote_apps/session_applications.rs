//! Individually owned launch groups on the display. Request IDs belong to
//! this GUI session only; they are never process IDs or global application IDs.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum State {
    /// Owned process group, but no presentable window yet.
    Opening,
    /// A window belonging to this launch has content. The host must still apply
    /// its first frame before reporting the application as visible.
    Window {
        window: crate::remote_apps::v1::WindowId,
    },
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    /// The launch request that owns this group.
    pub request: u64,
    pub state: State,
}

/// Welcome establishes revision zero with no owned launches. Each subsequent
/// change sends a complete bounded snapshot, before the associated launch reply.
/// Disappearance means group cleanup completed, not that an activated existing
/// instance or every window of a named application has closed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub revision: u64,
    pub applications: Vec<Application>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.revision == 0
            || self.applications.len() > crate::remote_apps::session_launch::MAX_PENDING
            || self.applications.iter().any(|app| app.request == 0)
            || self
                .applications
                .windows(2)
                .any(|pair| pair[0].request >= pair[1].request)
        {
            return Err("Invalid session application inventory".into());
        }
        for app in &self.applications {
            if let State::Window { window } = app.state {
                window.validate()?;
            }
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn inventories_are_bounded_sorted_unique_and_strict() {
        let app = |request| Application {
            request,
            state: State::Opening,
        };
        for applications in [vec![], vec![app(1)], (1..=32).map(app).collect()] {
            let snapshot = Snapshot {
                revision: 1,
                applications,
            };
            snapshot.validate().unwrap();
            assert_eq!(
                serde_json::from_slice::<Snapshot>(&serde_json::to_vec(&snapshot).unwrap())
                    .unwrap(),
                snapshot
            );
        }
        for applications in [
            vec![app(0)],
            vec![app(1), app(1)],
            vec![app(2), app(1)],
            (1..=33).map(app).collect(),
        ] {
            assert!(
                Snapshot {
                    revision: 1,
                    applications
                }
                .validate()
                .is_err()
            );
        }
        assert!(Snapshot::default().validate().is_err());
        assert!(
            serde_json::from_str::<Snapshot>(r#"{"revision":1,"applications":[],"pid":1}"#)
                .is_err()
        );
    }
}
