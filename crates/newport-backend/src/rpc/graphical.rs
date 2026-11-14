//! Typed application intent and receipts, through the backend-owned display.
use super::failure::{self, invalid_result, operation};
use crate::{
    manager,
    server_queries::{Cached, Read},
    server_session::ServerSession,
};
use newport_protocol::{
    common::{Present, Retry},
    desktop::{Failure, applications as dto},
};
use std::sync::Arc;
use uuid::Uuid;

pub struct Services<'a> {
    pub manager: &'a manager::Shared,
    pub account: &'a crate::account::Account,
    pub preferences: &'a crate::preferences::Preferences,
    pub desktop: crate::desktop_integration::Desktop,
}
fn not_submitted(error: Failure) -> Failure {
    Failure {
        error: newport_protocol::common::Error {
            code: dto::APPLICATION_NOT_SUBMITTED.into(),
            message: error.error.message,
            retry: Retry::No,
        },
        detail: error.detail,
    }
}
fn owner(
    manager: &manager::Shared,
    id: Uuid,
    expected: &newport_protocol::desktop::configuration::ConnectionScope,
) -> Result<Arc<ServerSession>, Failure> {
    if !manager.configuration_available() {
        return Err(operation("Server configuration is unavailable."));
    }
    let changed =
        |message| failure::failure(dto::APPLICATION_CONNECTION_CHANGED, message, Retry::No);
    let (profile, owner) = manager.current(id).map_err(changed)?;
    if profile.connection_scope(owner.revision(), manager.instance_id) != *expected {
        return Err(changed(
            "The server connection changed. Reopen Remote Apps.".into(),
        ));
    }
    Ok(owner)
}
async fn factory(
    services: &Services<'_>,
) -> Result<Arc<dyn crate::graphical_session::Factory>, Failure> {
    let license = services
        .account
        .remote_license()
        .await
        .map_err(failure::license)?;
    let settings = crate::preferences::remote_apps::remote_apps_settings(services.preferences)
        .await
        .map_err(|error| not_submitted(operation(error)))?;
    services
        .desktop
        .graphical_factory(license, settings)
        .await
        .map_err(|error| not_submitted(operation(error)))
}

pub async fn dispatch(
    command: &dto::Operation,
    services: &Services<'_>,
) -> Result<dto::Reply, Failure> {
    use dto::{Operation as O, Reply as R};
    Ok(match command {
        O::Status(args) => {
            let owner = crate::server_session::resolve(services.manager, args.server)
                .await
                .map_err(operation)?;
            match crate::server_queries::read(&owner, Read::RemoteAppsStatus, args.refresh)
                .await
                .map_err(operation)?
            {
                Cached::RemoteAppsStatus(status) => R::Status(status),
                _ => return Err(invalid_result("Remote Apps status cache kind")),
            }
        }
        O::Catalogue(args) => {
            let owner = crate::server_session::resolve(services.manager, args.server)
                .await
                .map_err(operation)?;
            match crate::server_queries::read(&owner, Read::RemoteAppsCatalogue, args.refresh)
                .await
                .map_err(operation)?
            {
                Cached::Catalogue(catalogue) => R::Catalogue(Box::new(catalogue)),
                _ => return Err(invalid_result("Remote Apps catalogue cache kind")),
            }
        }
        O::Display(args) => {
            let owner = crate::server_session::resolve(services.manager, args.server)
                .await
                .map_err(operation)?;
            R::Display(Box::new(owner.graphical_display()))
        }
        O::Launch(args) => {
            args.target
                .validate()
                .map_err(|error| not_submitted(operation(error)))?;
            let owner =
                owner(services.manager, args.server, &args.expected).map_err(not_submitted)?;
            let factory = factory(services).await?;
            let started = owner
                .launch_application(args.target.clone(), factory)
                .await
                .map_err(|error| not_submitted(operation(error)))?;
            let outcome = started.pending.outcome().await;
            R::Launch(Box::new(dto::Launched {
                application: started.application,
                outcome,
            }))
        }
        O::Show(args) => {
            let owner =
                owner(services.manager, args.server, &args.expected).map_err(not_submitted)?;
            owner
                .show_graphical(factory(services).await?)
                .await
                .map_err(|error| not_submitted(operation(error)))?;
            R::Show(Box::new(Present {}))
        }
        O::States(args) => {
            let owner = owner(services.manager, args.server, &args.expected)?;
            R::States(Box::new(dto::Observations {
                observations: args
                    .applications
                    .iter()
                    .map(|application| match owner.application_state(application) {
                        Ok(state) => dto::Observation {
                            application: application.clone(),
                            state: Some(state),
                            error: None,
                            ending: if state == dto::State::Ended {
                                owner.application_ending(application).ok().flatten()
                            } else {
                                None
                            },
                        },
                        Err(message) if message == crate::graphical_session::ENDED => {
                            dto::Observation {
                                application: application.clone(),
                                state: Some(dto::State::Ended),
                                error: None,
                                ending: Some(dto::Ending::DisplayStopped(Box::new(Present {}))),
                            }
                        }
                        Err(message) => dto::Observation {
                            application: application.clone(),
                            state: None,
                            error: Some(message),
                            ending: None,
                        },
                    })
                    .collect(),
            }))
        }
        O::Stop(args) => {
            let owner = owner(services.manager, args.server, &args.expected)?;
            R::Stop(Box::new(dto::Stopped {
                stopped: owner
                    .stop_application(&args.application)
                    .map_err(operation)?,
            }))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_integration::DesktopIntegration;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct DesktopProbe(AtomicUsize);
    impl DesktopIntegration for DesktopProbe {
        fn graphical_factory(
            &self,
            _: crate::account::RemoteLicense,
            _: crate::preferences::remote_apps::Settings,
        ) -> crate::graphical_session::FactoryFuture {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err("Unexpected graphical acquisition".into()) })
        }
        fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
            panic!("No native callbacks in launch validation")
        }
        fn clipboard_revision(&self) -> Option<isize> {
            None
        }
        fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
            None
        }
        fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
            None
        }
        fn open_url(&self, _: &str) -> Result<(), String> {
            panic!("No browser in launch validation")
        }
    }

    #[tokio::test]
    async fn a_refused_account_stops_a_launch_or_show_before_any_display_or_ssh_demand() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        let server: crate::model::Server = serde_json::from_value(json!({
            "id":Uuid::new_v4(), "name":"Mode validation", "sshHost":"127.0.0.1",
            "sshPort":9, "sshUser":"fixture"
        }))
        .unwrap();
        manager.config.servers.push(server.clone());
        let _ = manager.integration_access();
        let desktop = Arc::new(DesktopProbe::default());
        let account =
            crate::account::Account::new(manager.store.clone(), desktop.clone(), "fixture".into());
        let manager = crate::manager::share(manager);
        let preferences = crate::preferences::Preferences::open(directory.path()).unwrap();
        let mut scope = crate::rpc::tests::scope(server.id);
        scope.settings.ssh_port = server.ssh_port;
        scope.settings.ssh_host = server.ssh_host.clone();
        scope.settings.ssh_user = server.ssh_user.clone();
        scope.backend_instance = manager.instance_id;
        scope.connection_revision = manager.current(server.id).unwrap().1.revision();
        let services = Services {
            manager: &manager,
            account: &account,
            preferences: &preferences,
            desktop: desktop.clone(),
        };
        let launch = dto::Operation::Launch(Box::new(dto::LaunchArgs {
            server: server.id,
            expected: scope.clone(),
            target: dto::Target::Command(Box::new(dto::Command {
                argv: vec![
                    "env".into(),
                    "-u".into(),
                    "WAYLAND_DISPLAY".into(),
                    "--".into(),
                    "GDK_BACKEND=x11".into(),
                    "viewer".into(),
                ],
            })),
        }));
        let show = dto::Operation::Show(Box::new(dto::ShowArgs {
            server: server.id,
            expected: scope,
        }));
        for command in [launch, show] {
            let refused = dispatch(&command, &services).await.unwrap_err();
            assert_eq!(refused.error.code, "license_refused");
            assert!(
                matches!(refused.detail, Some(newport_protocol::desktop::FailureDetail::LicenseRefusal(reason)) if *reason == newport_protocol::desktop::account::RefusalReason::SignedOut)
            );
        }
        let (_, owner) = manager.current(server.id).unwrap();
        assert!(!owner.has_link());
        assert!(owner.graphical_sessions().is_empty());
        assert_eq!(desktop.0.load(Ordering::SeqCst), 0);
        // Unknown/duplicate protobuf operations are rejected by the shared
        // envelope decoder before this typed boundary can acquire resources.
    }
}
