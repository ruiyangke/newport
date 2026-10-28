//! The command router: each RPC method goes to the module that owns it.
use super::{account, control, features, git, graphical, preferences};
use serde_json::Value;

pub struct Services<'a> {
    pub integration_access: &'a crate::integration_permissions::Access,
    pub features: features::Services<'a>,
    pub git: git::Services<'a>,
    pub preferences: &'a crate::preferences::Preferences,
    pub account: &'a crate::account::Account,
    pub desktop: crate::desktop_integration::Desktop,
}

impl Services<'_> {
    /// `None` leaves desktop-only commands to the application's OS adapter.
    pub async fn dispatch(&self, method: &str, args: &Value) -> Option<Result<Value, Value>> {
        if let Some(command) = features::Command::parse(method) {
            return Some(command.dispatch(&self.features, args).await);
        }
        if let Some(command) = graphical::Command::parse(method) {
            return Some(
                command
                    .dispatch(
                        &graphical::Services {
                            manager: self.features.manager,
                            account: self.account,
                            preferences: self.preferences,
                            desktop: self.desktop.clone(),
                        },
                        args,
                    )
                    .await,
            );
        }
        if let Some(command) = git::Command::parse(method) {
            return Some(command.dispatch(&self.git, args).await);
        }
        if let Some(command) = preferences::Command::parse(method) {
            return Some(command.dispatch(self.preferences, args).await);
        }
        if let Some(command) = account::Command::parse(method) {
            return Some(command.dispatch(self.account).await);
        }
        if let Some(command) = control::Command::parse(method) {
            return Some(
                command
                    .dispatch(
                        &control::Services {
                            manager: self.features.manager,
                            integration_access: self.integration_access,
                            metrics: self.features.metrics,
                            files: self.features.files,
                            terminals: self.features.terminals,
                            preferences: self.preferences,
                            desktop: self.desktop.clone(),
                        },
                        args,
                    )
                    .await,
            );
        }
        None
    }
}
