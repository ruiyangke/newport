//! Exhaustive routing over the shared operation families.
use super::{account, control, features, files, git, graphical, metrics, preferences, terminal};
use newport_protocol::desktop::{Failure, Operation, Reply};

pub struct Services<'a> {
    pub integration_access: &'a crate::integration_permissions::Access,
    pub features: features::Services<'a>,
    pub git: git::Services<'a>,
    pub preferences: &'a crate::preferences::Preferences,
    pub account: &'a crate::account::Account,
    pub desktop: crate::desktop_integration::Desktop,
}
impl Services<'_> {
    /// Only typed Host operations are delegated to the OS application adapter.
    pub async fn dispatch(&self, command: &Operation) -> Option<Result<Reply, Failure>> {
        let control = control::Services {
            integration_access: self.integration_access,
            manager: self.features.manager,
            metrics: self.features.metrics,
            files: self.features.files,
            terminals: self.features.terminals,
            preferences: self.preferences,
            desktop: self.desktop.clone(),
        };
        Some(match command {
            Operation::Account(command) => account::dispatch(command, self.account)
                .await
                .map(|value| Reply::Account(Box::new(value))),
            Operation::Connection(command) => control::connection(command, &control)
                .await
                .map(|value| Reply::Connection(Box::new(value))),
            Operation::Configuration(command) => control::configuration(command, &control)
                .await
                .map(|value| Reply::Configuration(Box::new(value))),
            Operation::Files(command) => files::dispatch(command, &self.features)
                .await
                .map(|value| Reply::Files(Box::new(value))),
            Operation::Metrics(command) => metrics::dispatch(command, &self.features)
                .await
                .map(|value| Reply::Metrics(Box::new(value))),
            Operation::Services(command) => features::services(command, &self.features)
                .await
                .map(|value| Reply::Services(Box::new(value))),
            Operation::Containers(command) => features::containers(command, &self.features)
                .await
                .map(|value| Reply::Containers(Box::new(value))),
            Operation::Preferences(command) => preferences::dispatch(command, self.preferences)
                .await
                .map(|value| Reply::Preferences(Box::new(value))),
            Operation::Git(command) => git::dispatch(command, &self.git)
                .await
                .map(|value| Reply::Git(Box::new(value))),
            Operation::Applications(command) => graphical::dispatch(
                command,
                &graphical::Services {
                    manager: self.features.manager,
                    account: self.account,
                    preferences: self.preferences,
                    desktop: self.desktop.clone(),
                },
            )
            .await
            .map(|value| Reply::Applications(Box::new(value))),
            Operation::Terminal(command) => terminal::dispatch(command, &self.features)
                .await
                .map(|value| Reply::Terminal(Box::new(value))),
            Operation::Host(_) => return None,
            Operation::Events(_) | Operation::Stream(_) => Err(super::failure::operation(
                "Connection-owned operation cannot be dispatched as a command",
            )),
        })
    }
}
