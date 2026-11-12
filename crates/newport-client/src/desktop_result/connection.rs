//! Explicit typed reply projections into existing UI render state.
use super::configuration;
use newport_protocol::desktop::connection::*;
use serde_json::{Value, json};
pub(super) fn setup_inspection(value: SetupInspection) -> Value {
    json!({"os": value.os, "arch": value.architecture, "cores": value.cores, "memoryKiB": value.memory_kib, "freeKiB": value.free_kib, "installed": value.installed, "total": value.total})
}
pub(super) fn agent_key(value: AgentKey) -> Value {
    json!({"source": value.source, "comment": value.comment, "fingerprint": value.fingerprint, "algorithm": value.algorithm})
}
pub(super) fn agent_key_list(value: AgentKeyList) -> Value {
    json!({"keys": value.keys.into_iter().map(agent_key).collect::<Vec<_>>(), "warnings": value.warnings.into_iter().collect::<Vec<_>>()})
}
pub(super) fn config_host(value: ConfigHost) -> Value {
    json!({"id": value.id, "alias": value.alias, "host": value.host, "user": value.user, "port": value.port, "identityFile": value.identity_file, "proxy": value.proxy.map(configuration::proxy), "through": value.through.into_iter().collect::<Vec<_>>(), "importable": value.importable, "suggested": value.suggested, "note": value.note})
}
pub(super) fn config_hosts(value: ConfigHosts) -> Value {
    json!({"path": value.path, "hosts": value.hosts.into_iter().map(config_host).collect::<Vec<_>>()})
}
pub(super) fn ssh_resolved(value: SshResolved) -> Value {
    json!({"hostname": value.hostname, "user": value.user, "port": value.port, "identityFile": value.identity_file, "proxyCommand": value.proxy_command, "proxyJump": value.proxy_jump})
}
pub(super) fn project(reply: Reply) -> Result<Value, crate::request_error::RequestError> {
    Ok(match reply {
        Reply::SetupInspect(value) => setup_inspection(*value),
        Reply::SetupInstall(_)
        | Reply::SetupSkip(_)
        | Reply::TestConnection(_)
        | Reply::ReinstallAgent(_) => Value::Null,
        Reply::SetupFinish(value) => json!(value.notice),
        Reply::SshAgentKeys(value) => agent_key_list(*value),
        Reply::SshConfigHosts(value) => config_hosts(*value),
        Reply::ConnectionsQuery(_) => {
            return Err("Discovery results require the typed discovery client.".into());
        }
        Reply::UninstallAgent(value) => json!(value.text),
        Reply::InspectAgent(value) => configuration::installation_status(*value),
        Reply::SshResolve(value) => ssh_resolved(*value),
    })
}
