//! Account statuses for UI tests, built from the protocol's types.
use newport_protocol::desktop::account::{License, Phase, Status};

/// An account in `state`, with no license.
pub(crate) fn status(state: Phase) -> Status {
    Status {
        state,
        ..Default::default()
    }
}

/// A signed-in account holding `license`.
pub(crate) fn signed_in(license: Option<License>) -> Status {
    Status {
        state: Phase::SignedIn,
        license,
        ..Default::default()
    }
}

/// A license in `state`, on `seat` when given.
pub(crate) fn license(state: &str, seat: Option<&str>) -> License {
    License {
        state: Some(state.into()),
        seat: seat.map(Into::into),
        ..Default::default()
    }
}
