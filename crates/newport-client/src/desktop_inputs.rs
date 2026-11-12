//! Convert captured presentation identities before constructing typed requests.
use newport_protocol::desktop::configuration::{
    self, AuthMethod, ConnectionScope, ConnectionSettings, Proxy,
};
use serde_json::Value;
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid {}. Refresh the server connection before continuing.",
            self.0
        )
    }
}
impl Error for Invalid {}

pub fn identity(value: &str, field: &'static str) -> Result<uuid::Uuid, Invalid> {
    uuid::Uuid::parse_str(value).map_err(|_| Invalid(field))
}
fn text(value: &Value, field: &'static str) -> Result<String, Invalid> {
    value.as_str().map(str::to_owned).ok_or(Invalid(field))
}
fn optional_text(value: &Value, field: &'static str) -> Result<Option<String>, Invalid> {
    if value.is_null() {
        Ok(None)
    } else {
        text(value, field).map(Some)
    }
}
fn port(value: &Value) -> Result<u16, Invalid> {
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or(Invalid("SSH port"))
}
fn authentication(value: &Value) -> Result<AuthMethod, Invalid> {
    match value.as_str() {
        Some("publicKey") => Ok(AuthMethod::PublicKey),
        Some("password") => Ok(AuthMethod::Password),
        _ => Err(Invalid("authentication method")),
    }
}
fn proxy(value: &Value) -> Result<Option<Proxy>, Invalid> {
    if value.is_null() {
        return Ok(None);
    }
    let fields = value.as_object().ok_or(Invalid("proxy"))?;
    if fields.len() != 1 {
        return Err(Invalid("proxy"));
    }
    if let Some(command) = fields.get("command") {
        Ok(Some(Proxy::Command(Box::new(text(
            command,
            "proxy command",
        )?))))
    } else if let Some(jump) = fields.get("jump") {
        Ok(Some(Proxy::Jump(Box::new(identity(
            jump.as_str().ok_or(Invalid("jump server"))?,
            "jump server",
        )?))))
    } else {
        Err(Invalid("proxy"))
    }
}

pub fn connection_scope(value: &Value) -> Result<ConnectionScope, Invalid> {
    let value = value
        .as_array()
        .filter(|value| value.len() == 11)
        .ok_or(Invalid("connection identity"))?;
    let scope = ConnectionScope {
        settings: ConnectionSettings {
            server_id: identity(
                value[0].as_str().ok_or(Invalid("server identity"))?,
                "server identity",
            )?,
            ssh_host: text(&value[1], "SSH host")?,
            ssh_user: text(&value[2], "SSH user")?,
            ssh_port: port(&value[3])?,
            auth_method: authentication(&value[4])?,
            identity_file: optional_text(&value[5], "SSH identity file")?,
            agent_source: optional_text(&value[6], "SSH agent source")?,
            agent_key_fingerprint: optional_text(&value[7], "SSH key fingerprint")?,
            proxy: proxy(&value[8])?,
        },
        connection_revision: value[9].as_u64().ok_or(Invalid("connection revision"))?,
        backend_instance: identity(
            value[10].as_str().ok_or(Invalid("backend instance"))?,
            "backend instance",
        )?,
    };
    scope
        .validate()
        .map_err(|_| Invalid("connection identity"))?;
    Ok(scope)
}

pub fn server(value: &newport_domain::model::Server) -> configuration::Server {
    use newport_domain::model;
    configuration::Server {
        id: value.id,
        name: value.name.clone(),
        ssh_user: value.ssh_user.clone(),
        ssh_host: value.ssh_host.clone(),
        ssh_port: value.ssh_port,
        identity_file: value.identity_file.clone(),
        agent_source: value.agent_source.clone(),
        agent_key_fingerprint: value.agent_key_fingerprint.clone(),
        auth_method: match value.auth_method {
            model::AuthMethod::PublicKey => AuthMethod::PublicKey,
            model::AuthMethod::Password => AuthMethod::Password,
        },
        clipboard_enabled: value.clipboard_enabled,
        browser_enabled: value.browser_enabled,
        agent_install_approved: value.agent_install_approved,
        setup_complete: value.setup_complete,
        proxy: value.proxy.as_ref().map(|value| match value {
            model::Proxy::Command(command) => Proxy::Command(Box::new(command.clone())),
            model::Proxy::Jump(jump) => Proxy::Jump(Box::new(*jump)),
        }),
    }
}

/// Read a captured form/snapshot server without a serialization round trip.
pub fn server_record(value: &Value) -> Result<configuration::Server, Invalid> {
    fn flag(value: &Value, field: &'static str, default: bool) -> Result<bool, Invalid> {
        if value.is_null() {
            Ok(default)
        } else {
            value.as_bool().ok_or(Invalid(field))
        }
    }
    Ok(configuration::Server {
        id: identity(
            value["id"].as_str().ok_or(Invalid("server identity"))?,
            "server identity",
        )?,
        name: text(&value["name"], "server name")?,
        ssh_user: text(&value["sshUser"], "SSH user")?,
        ssh_host: text(&value["sshHost"], "SSH host")?,
        ssh_port: port(&value["sshPort"])?,
        identity_file: optional_text(&value["identityFile"], "SSH identity file")?,
        agent_source: optional_text(&value["agentSource"], "SSH agent source")?,
        agent_key_fingerprint: optional_text(&value["agentKeyFingerprint"], "SSH key fingerprint")?,
        auth_method: authentication(&value["authMethod"])?,
        clipboard_enabled: flag(&value["clipboardEnabled"], "clipboard permission", false)?,
        browser_enabled: flag(&value["browserEnabled"], "browser permission", false)?,
        agent_install_approved: flag(
            &value["agentInstallApproved"],
            "installation permission",
            false,
        )?,
        setup_complete: flag(&value["setupComplete"], "setup state", false)?,
        proxy: proxy(&value["proxy"])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn captured_scope_preserves_identity_and_rejects_lossy_scalar_coercion() {
        let mut value = json!(["11111111-1111-4111-8111-111111111111","host","user",22,"publicKey",null,null,null,{"command":"nc %h %p"},u64::MAX,"22222222-2222-4222-8222-222222222222"]);
        let captured = connection_scope(&value).unwrap();
        assert_eq!(captured.connection_revision, u64::MAX);
        assert!(
            matches!(captured.settings.proxy, Some(Proxy::Command(value)) if *value == "nc %h %p")
        );
        value[3] = json!(65536);
        assert!(connection_scope(&value).is_err());
        value[3] = json!(22);
        value[9] = json!(-1);
        assert!(connection_scope(&value).is_err());
        value[9] = json!(0);
        value[8] = json!({"command":"nc %h %p", "jump":"11111111-1111-4111-8111-111111111111"});
        assert!(connection_scope(&value).is_err());
    }
}

/// Native paths cross IPC without Unicode conversion or normalization.
pub fn native_path(value: &std::path::Path) -> newport_protocol::desktop::files::NativePath {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        newport_protocol::desktop::files::NativePath::Unix(Box::new(
            value.as_os_str().as_bytes().to_vec().into(),
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        newport_protocol::desktop::files::NativePath::Windows(Box::new(
            value.as_os_str().encode_wide().collect(),
        ))
    }
}
pub fn local_path(
    value: newport_protocol::desktop::files::NativePath,
) -> Result<std::path::PathBuf, Invalid> {
    match value {
        #[cfg(unix)]
        newport_protocol::desktop::files::NativePath::Unix(value) => {
            use std::os::unix::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_vec(value.to_vec()).into())
        }
        #[cfg(windows)]
        newport_protocol::desktop::files::NativePath::Windows(value) => {
            use std::os::windows::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_wide(value.as_ref()).into())
        }
        _ => Err(Invalid("native path platform")),
    }
}

pub fn tunnel(value: &newport_domain::model::Tunnel) -> configuration::Tunnel {
    configuration::Tunnel {
        id: value.id,
        name: value.name.clone(),
        server_id: value.server_id,
        local_port: value.local_port,
        local_port_end: value.local_port_end,
        remote_host: value.remote_host.clone(),
        remote_port: value.remote_port,
        remote_port_end: value.remote_port_end,
        auto_connect: value.auto_connect,
        auto_reconnect: value.auto_reconnect,
        direction: match value.direction {
            newport_domain::model::Direction::Local => configuration::Direction::Local,
            newport_domain::model::Direction::Remote => configuration::Direction::Remote,
        },
        project: value.project.clone(),
    }
}

pub(crate) fn domain_tunnel(value: configuration::Tunnel) -> newport_domain::model::Tunnel {
    newport_domain::model::Tunnel {
        id: value.id,
        name: value.name,
        server_id: value.server_id,
        local_port: value.local_port,
        local_port_end: value.local_port_end,
        remote_host: value.remote_host,
        remote_port: value.remote_port,
        remote_port_end: value.remote_port_end,
        auto_connect: value.auto_connect,
        auto_reconnect: value.auto_reconnect,
        direction: match value.direction {
            configuration::Direction::Local => newport_domain::model::Direction::Local,
            configuration::Direction::Remote => newport_domain::model::Direction::Remote,
        },
        project: value.project,
    }
}

#[cfg(test)]
mod native_path_tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn native_path_roundtrip_preserves_invalid_utf8_and_components() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let bytes = b"/tmp/../raw-\xff ".to_vec();
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes.clone()));
        assert_eq!(
            local_path(native_path(&path))
                .unwrap()
                .as_os_str()
                .as_bytes(),
            bytes
        );
    }
    #[cfg(windows)]
    #[test]
    fn native_path_roundtrip_preserves_unpaired_utf16_and_components() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let words = vec![67, 58, 92, 46, 46, 92, 0xd800, 32];
        let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&words));
        assert_eq!(
            local_path(native_path(&path))
                .unwrap()
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            words
        );
    }
}
