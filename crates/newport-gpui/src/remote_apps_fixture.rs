//! Remote apps replies for tests, built from the protocol's own types.
use newport_protocol::{
    common::Present,
    desktop::{
        self,
        applications::{
            Availability, Display, Elevation, Finding, Gap, Host, Inactive, Requirement,
            SelectedRuntime, Shown, ShownOn,
        },
        configuration::{BuildRelation, UpdateOutcome},
        preferences::{Codec, Installed, Settings},
    },
    product::Variant,
    remote_apps::apps::{Application, Catalogue, DesktopAction},
};

/// A status that knows nothing yet: no agent, nothing reported about the
/// server, and a Newport build without the full agent.
pub(crate) fn status() -> Availability {
    Availability {
        agent: None,
        legacy_agent: None,
        legacy_revision: None,
        legacy_agents: Vec::new(),
        system: None,
        architecture: None,
        gaps: Vec::new(),
        host: Host {
            elevation: Elevation::Sudo,
            updater: None,
            user_services: false,
        },
        os_release: String::new(),
        expected_server: crate::snapshot_fixture::server(uuid::Uuid::nil(), "Server"),
        bundled_variant: Variant::Core,
        bundled_revision: None,
        total: None,
        installed_variant: None,
        revision: None,
        build_relation: BuildRelation::Unknown,
        installation_error: None,
        selected_runtime: None,
        agent_error: None,
        automatic_update: None,
    }
}

/// A Linux server on `architecture` without the agent, with `os_release`.
pub(crate) fn linux(architecture: &str, os_release: &str, gaps: Vec<Gap>) -> Availability {
    Availability {
        system: Some("Linux".into()),
        architecture: Some(architecture.into()),
        gaps,
        os_release: os_release.into(),
        ..status()
    }
}

/// The full runtime `build` selected for launches.
pub(crate) fn selected(build: &str) -> Option<SelectedRuntime> {
    Some(SelectedRuntime {
        build_sha256: build.into(),
        product: Variant::Full,
    })
}

/// A server running `agent`, installed as the full product of `build`.
pub(crate) fn installed(agent: &str, build: &str) -> Availability {
    Availability {
        installed_variant: Some(Variant::Full),
        agent: Some(agent.into()),
        selected_runtime: selected(build),
        ..status()
    }
}

/// An automatic update from `from` to `to` that failed with `error`.
pub(crate) fn update(from: &str, to: &str, error: Option<&str>) -> Option<UpdateOutcome> {
    Some(UpdateOutcome {
        at: 1,
        from: Some(from.into()),
        to: Some(to.into()),
        error: error.map(Into::into),
    })
}

pub(crate) fn gap(requirement: Requirement, finding: Finding) -> Gap {
    Gap {
        requirement,
        finding,
    }
}

pub(crate) fn missing(requirement: Requirement) -> Gap {
    gap(requirement, Finding::Missing(Box::new(Present {})))
}

pub(crate) fn unverified(requirement: Requirement) -> Gap {
    gap(requirement, Finding::Unverified(Box::new(Present {})))
}

pub(crate) fn outdated(requirement: Requirement, version: &str) -> Gap {
    gap(requirement, Finding::Outdated(Box::new(version.into())))
}

pub(crate) fn inactive(requirement: Requirement, reason: Inactive) -> Gap {
    gap(requirement, Finding::Inactive(Box::new(reason)))
}

/// An application in the catalogue, without icon, description or actions.
pub(crate) fn app(id: &str, name: &str) -> Application {
    Application {
        id: id.into(),
        name: name.into(),
        description: String::new(),
        keywords: Vec::new(),
        icon: None,
        actions: Vec::new(),
    }
}

pub(crate) fn action(id: &str, name: &str) -> DesktopAction {
    DesktopAction {
        id: id.into(),
        name: name.into(),
    }
}

pub(crate) fn catalogue(apps: Vec<Application>) -> Catalogue {
    Catalogue { apps }
}

/// The display shown on `computer` (here when `None`) with `apps` and their
/// window counts.
pub(crate) fn display(shown_on: Option<ShownOn>, apps: &[(&str, u32)]) -> Display {
    Display {
        shown_on,
        applications: apps
            .iter()
            .map(|(app, windows)| Shown {
                app_id: (*app).into(),
                windows: *windows,
            })
            .collect(),
    }
}

/// Another computer, by name, and its viewer session.
pub(crate) fn elsewhere(computer: &str, session: Option<&str>) -> Option<ShownOn> {
    Some(ShownOn {
        computer: Some(computer.into()),
        session: session.map(Into::into),
    })
}

pub(crate) fn settings() -> Settings {
    Settings {
        audio: true,
        codec: Codec::Auto,
        fps_goal: 60,
        hud: false,
        separate_apps: cfg!(any(target_os = "macos", windows)),
    }
}

pub(crate) fn status_reply(status: Availability) -> desktop::Reply {
    applications_reply(desktop::applications::Reply::Status(Box::new(status)))
}

pub(crate) fn catalogue_reply(catalogue: Catalogue) -> desktop::Reply {
    applications_reply(desktop::applications::Reply::Catalogue(Box::new(catalogue)))
}

pub(crate) fn display_reply(display: Display) -> desktop::Reply {
    applications_reply(desktop::applications::Reply::Display(Box::new(display)))
}

pub(crate) fn installed_reply(servers: Vec<uuid::Uuid>) -> desktop::Reply {
    desktop::Reply::Preferences(Box::new(desktop::preferences::Reply::Installed(Box::new(
        Installed { servers },
    ))))
}

pub(crate) fn settings_reply(settings: Settings) -> desktop::Reply {
    desktop::Reply::Preferences(Box::new(desktop::preferences::Reply::Settings(Box::new(
        settings,
    ))))
}

fn applications_reply(reply: desktop::applications::Reply) -> desktop::Reply {
    desktop::Reply::Applications(Box::new(reply))
}

impl crate::Newport {
    /// Completes `key`'s read in flight with `reply`, as the backend would.
    pub(crate) fn answer_remote_apps(&mut self, key: &str, generation: u64, reply: desktop::Reply) {
        self.remote_apps_readings
            .complete(&mut self.resources, key, generation, Ok(reply));
    }

    /// Reads `key` afresh and completes it with `reply`.
    pub(crate) fn feed_remote_apps(&mut self, key: &str, reply: desktop::Reply) {
        let generation = self.resources.begin_refresh(key, false).unwrap();
        self.answer_remote_apps(key, generation, reply);
    }
}
