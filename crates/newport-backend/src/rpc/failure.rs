//! Explicit error lowering at the desktop boundary. Typed service failures keep
//! their code and retry decision; legacy business strings have unknown outcome.
use newport_protocol::{
    common::{Error, Retry},
    desktop::{Failure, FailureDetail, account::RefusalReason},
};

pub(super) fn failure(code: &str, message: impl Into<String>, retry: Retry) -> Failure {
    Failure {
        error: Error {
            code: code.into(),
            message: message.into(),
            retry,
        },
        detail: None,
    }
}
pub(crate) fn operation(message: impl std::fmt::Display) -> Failure {
    failure("operation", message.to_string(), Retry::Unknown)
}
pub(super) fn invalid_result(field: &str) -> Failure {
    failure(
        "invalid_result",
        format!("Invalid backend result field: {field}"),
        Retry::No,
    )
}
pub(crate) fn runtime(error: crate::agent_runtime::Error) -> Failure {
    use crate::agent_runtime::Error as E;
    let retry = if error.transient() {
        Retry::Yes
    } else {
        Retry::No
    };
    let code = match &error {
        E::Link(_) => "connection",
        E::Stale => "stale_connection",
        E::NotInstalled => "not_installed",
        E::OlderProtocol => "runtime_protocol",
        E::Interrupted(_) => "interrupted",
        E::Unavailable(_) => "unavailable",
        E::Admission(source) => {
            return failure("worker_admission", error.to_string(), source.retry());
        }
        E::Remote(remote) => {
            return Failure {
                error: remote.clone(),
                detail: None,
            };
        }
    };
    failure(code, error.to_string(), retry)
}
pub(super) fn license(error: crate::account::LicenseRefusal) -> Failure {
    let reason = match error.reason {
        "signed_out" => RefusalReason::SignedOut,
        "trial_elsewhere" => RefusalReason::TrialElsewhere,
        "free_seat" => RefusalReason::FreeSeat,
        "free" => RefusalReason::Free,
        "unavailable" => RefusalReason::Unavailable,
        "clock" => RefusalReason::Clock,
        "offline" => RefusalReason::Offline,
        _ => return invalid_result("license refusal reason"),
    };
    Failure {
        error: Error {
            code: "license_refused".into(),
            message: error.message,
            retry: Retry::No,
        },
        detail: Some(FailureDetail::LicenseRefusal(Box::new(reason))),
    }
}

pub(crate) fn terminal(error: &anyhow::Error) -> Error {
    for cause in error.chain() {
        if let Some(remote) = cause.downcast_ref::<crate::agent_runtime::Error>() {
            return runtime(remote.clone()).error;
        }
        if let Some(remote) = cause.downcast_ref::<Error>() {
            return remote.clone();
        }
    }
    operation(format!("{error:#}")).error
}

pub(super) fn remote(error: crate::remote::Failure) -> Failure {
    failure(
        error.code.unwrap_or("operation"),
        error.message,
        Retry::Unknown,
    )
}
