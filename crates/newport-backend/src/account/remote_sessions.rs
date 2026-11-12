//! Account policy for retained graphical sessions, independent of UI lifetime.
//! This stream contains no token. The private adapter still verifies every
//! license and enforces its signed holder, machine and expiry.
use super::{State, grants_pro};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Access {
    #[default]
    Loading,
    Renew {
        device: String,
    },
    /// No new launches/renewal; existing sessions keep their signed deadline.
    Keep {
        device: String,
    },
    Stop,
}
impl Access {
    fn device(&self) -> Option<&str> {
        match self {
            Self::Renew { device } | Self::Keep { device } => Some(device),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Changes on sign-out, explicit revocation or an account/device change.
    /// Coalesced notifications cannot hide a revoke followed by a new grant.
    pub epoch: u64,
    pub revision: u64,
    pub access: Access,
}
impl Policy {
    pub(super) fn from_state(state: &State, previous: &Self) -> Self {
        let access = if !state.loaded {
            Access::Loading
        } else if state.signing_out {
            Access::Stop
        } else if let Some(record) = &state.session {
            let license = &record.license;
            let state = license["state"].as_str();
            if state == Some("revoked")
                || (matches!(state, Some("active" | "grace")) && license["seat"] == "free")
            {
                Access::Stop
            } else if grants_pro(license) {
                Access::Renew {
                    device: record.device_id.clone(),
                }
            } else {
                Access::Keep {
                    device: record.device_id.clone(),
                }
            }
        } else {
            Access::Stop
        };
        let revokes = (access == Access::Stop && previous.access != Access::Stop)
            || (previous.access.device().is_some() && previous.access.device() != access.device());
        Self {
            epoch: previous
                .epoch
                .checked_add(u64::from(revokes))
                .expect("Account epoch exhausted"),
            revision: state.license_revision,
            access,
        }
    }
}
