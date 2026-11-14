//! Convert captured presentation identities before constructing typed requests.
use newport_protocol::desktop::configuration::ConnectionScope;
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
/// The connection scope a view captured, checked before a request carries
/// it. A view without one has no server connection to name.
pub fn captured(scope: Option<&ConnectionScope>) -> Result<ConnectionScope, Invalid> {
    let scope = scope.ok_or(Invalid("connection identity"))?;
    scope
        .validate()
        .map_err(|_| Invalid("connection identity"))?;
    if scope.settings.ssh_port == 0 {
        return Err(Invalid("SSH port"));
    }
    Ok(scope.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::desktop::configuration::{AuthMethod, ConnectionSettings, Proxy};
    #[test]
    fn a_captured_scope_is_checked_before_it_is_sent() {
        assert_eq!(captured(None), Err(Invalid("connection identity")));
        let mut scope = ConnectionScope {
            settings: ConnectionSettings {
                server_id: uuid::Uuid::from_u128(1),
                ssh_host: "host".into(),
                ssh_user: "user".into(),
                ssh_port: 22,
                auth_method: AuthMethod::PublicKey,
                identity_file: None,
                agent_source: None,
                agent_key_fingerprint: None,
                proxy: Some(Proxy::Command(Box::new("nc %h %p".into()))),
            },
            connection_revision: u64::MAX,
            backend_instance: uuid::Uuid::from_u128(2),
        };
        assert_eq!(captured(Some(&scope)).as_ref(), Ok(&scope));
        scope.settings.ssh_port = 0;
        assert_eq!(captured(Some(&scope)), Err(Invalid("SSH port")));
        scope.settings.ssh_port = 22;
        scope.settings.ssh_host = "host\0".into();
        assert_eq!(captured(Some(&scope)), Err(Invalid("connection identity")));
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
