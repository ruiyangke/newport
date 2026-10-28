//! Newport account sign-in routes. Outcomes reach the UI as status notices.
use super::arguments::{encode, error};
use crate::account::Account;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    AccountStatus,
    AccountSignIn,
    AccountCancelSignIn,
    AccountSignOut,
    AccountOpenPortal,
    AccountOpenBilling,
    AccountRefresh,
    AccountStartTrial,
    /// The Remote apps license, for Newport Remote over the owner-only socket.
    AccountRemoteLicense,
}
impl Command {
    pub fn parse(method: &str) -> Option<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(method))
            .ok()
    }
    pub async fn dispatch(self, account: &Account) -> Result<Value, Value> {
        match self {
            Self::AccountStatus => encode(account.status()),
            Self::AccountSignIn => {
                account.sign_in().await.map_err(error)?;
                encode(account.status())
            }
            Self::AccountCancelSignIn => {
                account.cancel_sign_in();
                encode(account.status())
            }
            Self::AccountSignOut => {
                account.sign_out().await.map_err(error)?;
                encode(account.status())
            }
            Self::AccountOpenPortal => encode(account.open_portal().await.map_err(error)?),
            Self::AccountOpenBilling => encode(account.open_billing().await.map_err(error)?),
            Self::AccountRefresh => {
                account.refresh_now().await;
                encode(account.status())
            }
            Self::AccountStartTrial => {
                account.start_trial().await.map_err(error)?;
                encode(account.status())
            }
            Self::AccountRemoteLicense => encode(account.remote_license().await.map_err(error)?),
        }
    }
}
