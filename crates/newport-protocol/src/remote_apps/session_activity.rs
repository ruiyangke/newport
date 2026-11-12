//! Owned application demand on the display. Welcome establishes revision zero
//! with no launchers. Later changes precede launch acceptance.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub revision: u64,
    /// Supervised application groups, including those without any window.
    /// Native windows are tracked independently by normal window messages.
    pub applications: u32,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.revision == 0
            || self.applications as usize > crate::remote_apps::session_launch::MAX_PENDING
        {
            return Err("Invalid session application activity".into());
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn activity_is_bounded_and_has_no_unnegotiated_fields() {
        for applications in [0, 1, crate::remote_apps::session_launch::MAX_PENDING as u32] {
            let snapshot = Snapshot {
                revision: 1,
                applications,
            };
            snapshot.validate().unwrap();
            assert_eq!(
                serde_json::from_str::<Snapshot>(&serde_json::to_string(&snapshot).unwrap())
                    .unwrap(),
                snapshot
            );
        }
        assert!(
            Snapshot {
                revision: 0,
                applications: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            Snapshot {
                revision: 1,
                applications: 33
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<Snapshot>(r#"{"revision":1,"applications":1,"pid":100}"#)
                .is_err()
        );
    }
}
