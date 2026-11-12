//! Typed account routes. Credentials are lowered only into the redacted Secret.
use super::failure::{invalid_result, license, operation};
use crate::account::Account;
use newport_protocol::{
    common::{Present, Secret},
    desktop::{Failure, account as dto},
};
use serde_json::Value;

fn optional_text(value: &Value, key: &str) -> Result<Option<String>, Failure> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        _ => Err(invalid_result(key)),
    }
}
fn optional_bool(value: &Value, key: &str) -> Result<Option<bool>, Failure> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(invalid_result(key)),
    }
}
fn status(value: crate::account::Status) -> Result<dto::Status, Failure> {
    let state = match value.status {
        "loading" => dto::Phase::Loading,
        "signedOut" => dto::Phase::SignedOut,
        "waiting" => dto::Phase::Waiting,
        "signedIn" => dto::Phase::SignedIn,
        _ => return Err(invalid_result("account status")),
    };
    let notice = value
        .notice
        .map(|notice| {
            Ok(dto::Notice {
                id: notice.id,
                kind: match notice.kind {
                    "success" => dto::NoticeKind::Success,
                    "info" => dto::NoticeKind::Info,
                    "warning" => dto::NoticeKind::Warning,
                    "error" => dto::NoticeKind::Error,
                    _ => return Err(invalid_result("notice kind")),
                },
                message: notice.message,
                action: match notice.action {
                    None => None,
                    Some("manageComputers") => Some(dto::NoticeAction::ManageComputers),
                    Some("tryAgain") => Some(dto::NoticeAction::TryAgain),
                    _ => return Err(invalid_result("notice action")),
                },
            })
        })
        .transpose()?;
    // The account API deliberately exposes a filtered persisted-license read
    // model. Read its named fields explicitly; no generic serialized wire DTO.
    let license = value
        .license
        .map(|license| {
            if !license.is_object() {
                return Err(invalid_result("license"));
            }
            Ok(dto::License {
                state: optional_text(&license, "state")?,
                plan: optional_text(&license, "plan")?,
                seat: optional_text(&license, "seat")?,
                valid_until: optional_text(&license, "validUntil")?,
                renews_at: optional_text(&license, "renewsAt")?,
                ends_at: optional_text(&license, "endsAt")?,
                trial_used: optional_bool(&license, "trialUsed")?,
                message: optional_text(&license, "message")?,
                trial_used_here: optional_bool(&license, "trialUsedHere")?,
            })
        })
        .transpose()?;
    Ok(dto::Status {
        state,
        email: value.email,
        name: value.name,
        license,
        signing_out: value.signing_out,
        notice,
        license_revision: value.license_revision,
    })
}

pub async fn dispatch(command: &dto::Operation, account: &Account) -> Result<dto::Reply, Failure> {
    use dto::{Operation as O, Reply as R};
    Ok(match command {
        O::Status(_) => R::Status(Box::new(status(account.status())?)),
        O::SignIn(_) => {
            account.sign_in().await.map_err(operation)?;
            R::SignIn(Box::new(status(account.status())?))
        }
        O::CancelSignIn(_) => {
            account.cancel_sign_in();
            R::CancelSignIn(Box::new(status(account.status())?))
        }
        O::SignOut(_) => {
            account.sign_out().await.map_err(operation)?;
            R::SignOut(Box::new(status(account.status())?))
        }
        O::OpenPortal(_) => {
            account.open_portal().await.map_err(operation)?;
            R::OpenPortal(Box::new(Present {}))
        }
        O::OpenBilling(_) => {
            account.open_billing().await.map_err(operation)?;
            R::OpenBilling(Box::new(Present {}))
        }
        O::Refresh(_) => {
            account.refresh_now().await;
            R::Refresh(Box::new(status(account.status())?))
        }
        O::StartTrial(_) => {
            account.start_trial().await.map_err(operation)?;
            R::StartTrial(Box::new(status(account.status())?))
        }
        O::RemoteLicense(_) => {
            let value = account.remote_license().await.map_err(license)?;
            R::RemoteLicense(Box::new(dto::RemoteLicense {
                token: Secret {
                    value: value.token.as_bytes().to_vec().into(),
                },
                machine: value.machine,
                trusted_time: value.trusted_time,
                expires_at: value.expires_at,
            }))
        }
    })
}
