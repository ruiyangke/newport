//! Application intent and observation through the backend-owned display. These
//! commands never carry GUI wire bytes or create per-application worker streams.
use super::arguments::{encode, error};
use crate::{graphical_session::launch, manager, server_session::ServerSession};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub struct Services<'a> {
    pub manager: &'a manager::Shared,
    pub account: &'a crate::account::Account,
    pub preferences: &'a crate::preferences::Preferences,
    pub desktop: crate::desktop_integration::Desktop,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    RemoteAppsLaunch,
    RemoteAppsStates,
    RemoteAppsStop,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Launch {
    server_id: Uuid,
    expected_connection: Value,
    target: launch::Target,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct States {
    server_id: Uuid,
    expected_connection: Value,
    applications: Vec<launch::Application>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stop {
    server_id: Uuid,
    expected_connection: Value,
    application: launch::Application,
}
fn decode<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, Value> {
    serde_json::from_value(args.clone())
        .map_err(|e| error(format!("Invalid application request: {e}")))
}
fn not_submitted(value: Value) -> Value {
    json!({"code":"APPLICATION_NOT_SUBMITTED", "message":value.as_str().or_else(|| value["message"].as_str()).map(str::to_owned).unwrap_or_else(|| value.to_string())})
}
fn connection_changed(message: impl std::fmt::Display) -> Value {
    json!({"code":"APPLICATION_CONNECTION_CHANGED", "message":message.to_string()})
}
fn owner(
    manager: &manager::Shared,
    id: Uuid,
    expected: &Value,
) -> Result<Arc<ServerSession>, Value> {
    if !manager.configuration_available() {
        return Err(error("Server configuration is unavailable."));
    }
    let (profile, owner) = manager.current(id).map_err(connection_changed)?;
    let scope = serde_json::to_value(profile).map_err(|e| error(e.to_string()))?;
    let snapshot = json!({"instanceId":manager.instance_id,
        "runtime":{"connectionRevisions":{id.to_string():owner.revision()}}});
    if newport_domain::connection_scope::connection(&scope, &snapshot) != *expected {
        return Err(connection_changed(
            "The server connection changed. Reopen Remote Apps.",
        ));
    }
    Ok(owner)
}
impl Command {
    pub fn parse(method: &str) -> Option<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(method))
            .ok()
    }
    pub async fn dispatch(self, services: &Services<'_>, args: &Value) -> Result<Value, Value> {
        match self {
            Self::RemoteAppsLaunch => {
                let request: Launch = decode(args).map_err(not_submitted)?;
                request
                    .target
                    .validate()
                    .map_err(|message| not_submitted(error(message)))?;
                let owner = owner(
                    services.manager,
                    request.server_id,
                    &request.expected_connection,
                )
                .map_err(not_submitted)?;
                // Account authorization and settings precede any SSH demand.
                let license = services.account.remote_license().await.map_err(|refusal| {
                    json!({"code":format!("APPLICATION_REFUSED:{}", refusal.reason),"message":refusal.message})
                })?;
                let settings =
                    crate::preferences::remote_apps::remote_apps_settings(services.preferences)
                        .await
                        .map_err(|message| not_submitted(error(message)))?;
                let factory = services
                    .desktop
                    .graphical_factory(license, settings)
                    .await
                    .map_err(|message| not_submitted(error(message)))?;
                let started = owner
                    .launch_application(request.target, factory)
                    .await
                    .map_err(|message| not_submitted(error(message)))?;
                // Once submitted, loss of this local caller cannot cancel or
                // replay the launch. Unknown is returned distinctly from refusal.
                let outcome = started.pending.outcome().await;
                Ok(json!({"application":started.application,"outcome":outcome}))
            }
            Self::RemoteAppsStates => {
                let request: States = decode(args)?;
                if request.applications.len() > 128 {
                    return Err(error("Too many application observations"));
                }
                let owner = owner(
                    services.manager,
                    request.server_id,
                    &request.expected_connection,
                )?;
                encode(
                    request
                        .applications
                        .into_iter()
                        .map(|application| match owner.application_state(&application) {
                            Ok(state) => {
                                let mut row =
                                    json!({"application":application,"state":state,"error":null});
                                if state == crate::graphical_session::launch::State::Ended {
                                    if let Ok(Some(ending)) = owner.application_ending(&application)
                                    {
                                        row["ending"] = json!(ending);
                                    }
                                } else if let Ok(Some(notice)) =
                                    owner.application_notice(&application)
                                {
                                    row["notice"] = json!(notice);
                                }
                                row
                            }
                            // Its windows went with the display.
                            Err(message) if message == crate::graphical_session::ENDED => json!({
                                "application": application,
                                "state": crate::graphical_session::launch::State::Ended,
                                "error": null,
                                "ending": crate::graphical_session::launch::Ending::DisplayStopped,
                            }),
                            Err(message) => {
                                json!({"application":application,"state":null,"error":message})
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            }
            Self::RemoteAppsStop => {
                let request: Stop = decode(args)?;
                let owner = owner(
                    services.manager,
                    request.server_id,
                    &request.expected_connection,
                )?;
                encode(
                    owner
                        .stop_application(&request.application)
                        .map_err(error)?,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_integration::DesktopIntegration;
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
    async fn a_refused_account_stops_a_launch_before_any_display_or_ssh_demand() {
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
        let snapshot = serde_json::to_value(manager.snapshot()).unwrap();
        let scope = newport_domain::connection_scope::connection(
            &serde_json::to_value(&server).unwrap(),
            &snapshot,
        );
        let services = Services {
            manager: &manager,
            account: &account,
            preferences: &preferences,
            desktop: desktop.clone(),
        };
        let args = json!({
            "serverId":server.id, "expectedConnection":scope,
            "target":{"kind":"command", "argv":["env","-u","WAYLAND_DISPLAY","--","GDK_BACKEND=x11","viewer"]}
        });
        // The signed-out account refuses the launch, so this test is local.
        let refused = Command::RemoteAppsLaunch
            .dispatch(&services, &args)
            .await
            .unwrap_err();
        assert!(
            refused["code"]
                .as_str()
                .unwrap()
                .starts_with("APPLICATION_REFUSED:"),
            "{refused}"
        );
        let (_, owner) = manager.current(server.id).unwrap();
        assert!(!owner.has_link());
        assert!(owner.graphical_sessions().is_empty());
        assert_eq!(desktop.0.load(Ordering::SeqCst), 0);
        // Local RPC remains strict about unknown launch requirements.
        let mut unknown = args.clone();
        unknown["requiresUnknownDisplay"] = json!(true);
        assert!(decode::<Launch>(&unknown).is_err());
    }
}
