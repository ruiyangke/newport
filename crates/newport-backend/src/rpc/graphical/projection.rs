//! Explicit lowering of existing availability/catalogue read models.
use super::{dto, invalid_result, p};
use newport_protocol::{
    common::Present,
    desktop::{Failure, configuration as config},
    product::Variant,
};
use serde_json::Value;
fn enum_text<T>(value: &Value, read: impl FnOnce(&str) -> Option<T>) -> Result<T, Failure> {
    value
        .as_str()
        .and_then(read)
        .ok_or_else(|| invalid_result("enum"))
}
fn product(value: &Value) -> Result<Variant, Failure> {
    enum_text(value, |v| match v {
        "core" => Some(Variant::Core),
        "full" => Some(Variant::Full),
        _ => None,
    })
}
fn relation(value: &Value) -> Result<config::BuildRelation, Failure> {
    enum_text(value, |v| {
        Some(match v {
            "unknown" => config::BuildRelation::Unknown,
            "missing" => config::BuildRelation::Missing,
            "bundled" => config::BuildRelation::Bundled,
            "different" => config::BuildRelation::Different,
            "older" => config::BuildRelation::Older,
            "newer" => config::BuildRelation::Newer,
            "sameRelease" => config::BuildRelation::SameRelease,
            _ => return None,
        })
    })
}
fn update(value: &Value) -> Result<config::UpdateOutcome, Failure> {
    Ok(config::UpdateOutcome {
        at: p::unsigned(p::field(value, "at")?)?,
        from: p::optional(value, "from", p::text)?,
        to: p::optional(value, "to", p::text)?,
        error: p::optional(value, "error", p::text)?,
    })
}
fn server(value: &Value) -> Result<config::Server, Failure> {
    Ok(config::Server {
        id: p::text(p::field(value, "id")?)?
            .parse()
            .map_err(|_| invalid_result("server id"))?,
        name: p::text(p::field(value, "name")?)?,
        ssh_user: p::text(p::field(value, "sshUser")?)?,
        ssh_host: p::text(p::field(value, "sshHost")?)?,
        ssh_port: u16::try_from(p::unsigned(p::field(value, "sshPort")?)?)
            .map_err(|_| invalid_result("sshPort"))?,
        identity_file: p::optional(value, "identityFile", p::text)?,
        agent_source: p::optional(value, "agentSource", p::text)?,
        agent_key_fingerprint: p::optional(value, "agentKeyFingerprint", p::text)?,
        auth_method: enum_text(p::field(value, "authMethod")?, |v| match v {
            "publicKey" => Some(config::AuthMethod::PublicKey),
            "password" => Some(config::AuthMethod::Password),
            _ => None,
        })?,
        clipboard_enabled: p::bool_value(p::field(value, "clipboardEnabled")?)?,
        browser_enabled: p::bool_value(p::field(value, "browserEnabled")?)?,
        agent_install_approved: p::bool_value(p::field(value, "agentInstallApproved")?)?,
        setup_complete: p::bool_value(p::field(value, "setupComplete")?)?,
        proxy: p::optional(value, "proxy", |proxy| {
            if let Some(command) = proxy.get("command") {
                Ok(config::Proxy::Command(Box::new(p::text(command)?)))
            } else if let Some(jump) = proxy.get("jump") {
                Ok(config::Proxy::Jump(Box::new(
                    p::text(jump)?
                        .parse()
                        .map_err(|_| invalid_result("proxy jump"))?,
                )))
            } else {
                Err(invalid_result("proxy"))
            }
        })?,
    })
}
fn gap(value: &Value) -> Result<dto::Gap, Failure> {
    Ok(dto::Gap {
        requirement: enum_text(p::field(value, "requirement")?, |v| {
            Some(match v {
                "dbus" => dto::Requirement::Dbus,
                "xwayland" => dto::Requirement::Xwayland,
                "fonts" => dto::Requirement::Fonts,
                "gles" => dto::Requirement::Gles,
                "portal" => dto::Requirement::Portal,
                "audio" => dto::Requirement::Audio,
                _ => return None,
            })
        })?,
        finding: match p::text(p::field(value, "state")?)?.as_str() {
            "missing" => dto::Finding::Missing(Box::new(Present {})),
            "outdated" => dto::Finding::Outdated(Box::new(p::text(p::field(value, "version")?)?)),
            "unverified" => dto::Finding::Unverified(Box::new(Present {})),
            "inactive" => dto::Finding::Inactive(Box::new(enum_text(
                p::field(value, "reason")?,
                |v| match v {
                    "root" => Some(dto::Inactive::Root),
                    "stopped" => Some(dto::Inactive::Stopped),
                    "failed" => Some(dto::Inactive::Failed),
                    _ => None,
                },
            )?)),
            _ => return Err(invalid_result("requirement finding")),
        },
    })
}
pub(super) fn availability(value: &Value) -> Result<dto::Availability, Failure> {
    let host = p::field(value, "host")?;
    Ok(dto::Availability {
        agent: p::optional(value, "agent", p::text)?,
        legacy_agent: p::optional(value, "legacyAgent", p::text)?,
        legacy_revision: p::optional(value, "legacyRevision", p::text)?,
        system: p::optional(value, "system", p::text)?,
        architecture: p::optional(value, "architecture", p::text)?,
        bundled_revision: p::optional(value, "bundledRevision", p::text)?,
        revision: p::optional(value, "revision", p::text)?,
        installation_error: p::optional(value, "installationError", p::text)?,
        agent_error: p::optional(value, "agentError", p::text)?,
        legacy_agents: p::list(p::field(value, "legacyAgents")?, |entry| {
            Ok(dto::LegacyAgent {
                revision: p::text(p::field(entry, "revision")?)?,
                agent: p::text(p::field(entry, "agent")?)?,
                built_at: p::optional(entry, "builtAt", p::unsigned)?,
            })
        })?,
        gaps: p::list(p::field(value, "gaps")?, gap)?,
        host: dto::Host {
            elevation: enum_text(p::field(host, "elevation")?, |v| match v {
                "root" => Some(dto::Elevation::Root),
                "sudo" => Some(dto::Elevation::Sudo),
                "doas" => Some(dto::Elevation::Doas),
                "unavailable" => Some(dto::Elevation::Unavailable),
                _ => None,
            })?,
            updater: p::optional(host, "updater", |value| {
                enum_text(value, |v| match v {
                    "rpmOstree" => Some(dto::Updater::RpmOstree),
                    "transactionalUpdate" => Some(dto::Updater::TransactionalUpdate),
                    _ => None,
                })
            })?,
            user_services: p::bool_value(p::field(host, "userServices")?)?,
        },
        os_release: p::text(p::field(value, "osRelease")?)?,
        expected_server: server(p::field(value, "expectedConnection")?)?,
        bundled_variant: product(p::field(value, "bundledVariant")?)?,
        total: p::optional(value, "total", p::unsigned)?,
        installed_variant: p::optional(value, "installedVariant", product)?,
        build_relation: p::optional(value, "buildRelation", relation)?
            .unwrap_or(config::BuildRelation::Unknown),
        selected_runtime: p::optional(value, "selectedRuntime", |runtime| {
            Ok(dto::SelectedRuntime {
                build_sha256: p::text(p::field(runtime, "buildSha256")?)?,
                product: product(p::field(runtime, "product")?)?,
            })
        })?,
        automatic_update: p::optional(value, "automaticUpdate", update)?,
    })
}
pub(super) fn catalogue(
    value: &Value,
) -> Result<newport_protocol::remote_apps::apps::Catalogue, Failure> {
    use newport_protocol::remote_apps::apps::{Application, Catalogue, DesktopAction};
    let catalogue = Catalogue {
        apps: p::list(p::field(value, "apps")?, |value| {
            Ok(Application {
                id: p::text(p::field(value, "id")?)?,
                name: p::text(p::field(value, "name")?)?,
                description: p::text(p::field(value, "description")?)?,
                keywords: p::list(p::field(value, "keywords")?, p::text)?,
                icon: p::optional(value, "icon", p::text)?,
                actions: p::list(p::field(value, "actions")?, |action| {
                    Ok(DesktopAction {
                        id: p::text(p::field(action, "id")?)?,
                        name: p::text(p::field(action, "name")?)?,
                    })
                })?,
            })
        })?,
    };
    catalogue
        .validate()
        .map_err(|_| invalid_result("catalogue"))?;
    Ok(catalogue)
}
