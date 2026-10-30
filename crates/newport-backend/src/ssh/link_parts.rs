//! What a server's [`newport_ssh::Link`] is built from: the way its record
//! says to reach it, the login it describes, and OpenSSH's `known_hosts`.
use super::auth;
use crate::{
    known_hosts,
    model::{Proxy, Server},
};
use newport_ssh::{
    BoxFuture, CommandDialer, Dialed, Dialer, Error, Handler, JumpDialer, Limits, Link, Parts,
    Target, TcpDialer,
};
use russh::keys::PublicKeyOrCertificate;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;
use uuid::Uuid;

/// A link to `server` with the production parts and the design's limits.
/// `jump` gives the link of the saved server it jumps through, which every
/// server behind that one shares.
pub fn link(server: &Server, jump: impl FnOnce(Uuid) -> Result<Link, Error>) -> Link {
    link_with(server, dialer(server, jump), KnownHosts::standard())
}

/// For links that never jump, such as the fixture tests' own.
#[cfg(test)]
pub(crate) fn no_jump_hosts(_: Uuid) -> Result<Link, Error> {
    Err(Error::InvalidJump(
        "no saved servers to jump through".into(),
    ))
}

/// How the link reaches `server`. A jump host that can't be found fails
/// every dial with its reason rather than connecting another way.
pub(super) fn dialer(
    server: &Server,
    jump: impl FnOnce(Uuid) -> Result<Link, Error>,
) -> Arc<dyn Dialer> {
    match &server.proxy {
        None => Arc::new(TcpDialer),
        Some(Proxy::Command(command)) => Arc::new(proxy_command(server, command)),
        Some(Proxy::Jump(id)) => match jump(*id) {
            Ok(link) => Arc::new(JumpDialer::new(link)),
            Err(error) => Arc::new(Unusable(error)),
        },
    }
}

/// The server's proxy command. ssh's `%n` is the host as the person typed
/// it, such as an `ssh_config` alias; a server name that isn't a host name
/// leaves `%n` as the host, so free text never reaches the shell.
fn proxy_command(server: &Server, command: &str) -> CommandDialer {
    let dialer = CommandDialer::new(command);
    if crate::model::host_valid(&server.name) {
        dialer.original_host(server.name.clone())
    } else {
        dialer
    }
}

/// Fails every dial with why the server can't be reached as configured.
struct Unusable(Error);

impl Dialer for Unusable {
    fn dial<'a>(
        &'a self,
        _: &'a Target,
        _: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<Dialed, Error>> {
        Box::pin(async move { Err(self.0.clone()) })
    }
}

pub(super) fn link_with(server: &Server, dialer: Arc<dyn Dialer>, host_keys: KnownHosts) -> Link {
    Link::new(
        Target {
            host: server.ssh_host.clone(),
            port: server.ssh_port,
            user: server.ssh_user.clone(),
        },
        Parts {
            dialer,
            authenticator: Arc::new(Login(server.clone())),
            host_keys: Arc::new(host_keys),
        },
        Limits::default(),
    )
}

/// Logs in as the server record says: its key file, the SSH agent or one
/// selected agent key, or its saved password.
struct Login(Server);

impl newport_ssh::Authenticator for Login {
    fn authenticate<'a>(
        &'a self,
        handle: &'a mut russh::client::Handle<Handler>,
        _: &'a Target,
        approval: &'a watch::Sender<bool>,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            auth::authenticate(handle, &self.0, approval.clone())
                .await
                .map_err(|error| login_error(&error))
        })
    }
}

/// The link's class for a failed login. The cause is logged here because
/// the classes carry no text of their own.
pub(super) fn login_error(error: &anyhow::Error) -> Error {
    let class = if error.is::<auth::AgentApprovalRequired>() {
        Error::ApprovalRequired
    } else if error.is::<auth::SelectedAgentUnavailable>() {
        Error::AgentUnavailable
    } else if error.is::<auth::CredentialsUnavailable>() {
        // The causes say what couldn't be used; the marker itself adds nothing.
        let marker = auth::CredentialsUnavailable.to_string();
        Error::Credentials(
            error
                .chain()
                .map(ToString::to_string)
                .filter(|cause| *cause != marker)
                .collect::<Vec<_>>()
                .join(": "),
        )
    } else if interrupted(error) {
        Error::Lost(format!("{error:#}"))
    } else {
        Error::AuthRefused
    };
    log::info!("SSH login failed ({class:?}): {error:#}");
    class
}

/// Whether the login failed because the connection or the agent's socket
/// broke, rather than because a credential was refused or missing.
fn interrupted(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        // russh's IO variant hides its inner error from `source()`.
        let io =
            cause
                .downcast_ref::<std::io::Error>()
                .or(match cause.downcast_ref::<russh::Error>() {
                    Some(russh::Error::IO(error)) => Some(error),
                    _ => None,
                });
        if let Some(error) = io {
            return matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::UnexpectedEof
            );
        }
        matches!(
            cause.downcast_ref::<russh::Error>(),
            Some(
                russh::Error::Disconnect
                    | russh::Error::HUP
                    | russh::Error::ConnectionTimeout
                    | russh::Error::KeepaliveTimeout
                    | russh::Error::InactivityTimeout
                    | russh::Error::SendError
            )
        ) || cause.is::<russh::SendError>()
    })
}

/// Host keys as OpenSSH's `known_hosts` has them: a new server's key is
/// trusted on first use, a changed key is refused, and host certificates
/// aren't accepted.
pub(super) struct KnownHosts {
    /// The user's file; `None` finds it when a key is checked.
    user: Option<PathBuf>,
    system: Option<PathBuf>,
}

impl KnownHosts {
    fn standard() -> Self {
        #[cfg(any(test, feature = "test-support"))]
        let system = None;
        #[cfg(not(any(test, feature = "test-support")))]
        let system = newport_platform::ssh_agent::system_known_hosts();
        Self { user: None, system }
    }

    #[cfg(test)]
    pub(super) fn at(user: PathBuf) -> Self {
        Self {
            user: Some(user),
            system: None,
        }
    }
}

impl newport_ssh::HostKeys for KnownHosts {
    fn verify(&self, target: &Target, key: &PublicKeyOrCertificate) -> Result<(), Error> {
        let key = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            PublicKeyOrCertificate::Certificate(_) => {
                return Err(Error::HostKeyRejected(
                    known_hosts::CERTIFICATES_UNSUPPORTED.into(),
                ));
            }
        };
        let path = match &self.user {
            Some(path) => Ok(path.clone()),
            None => known_hosts::user_path(),
        };
        // The file is locked while it is read and appended to, so this
        // may wait for another process that holds it.
        let result = blocking(|| {
            known_hosts::verify(
                &target.host,
                target.port,
                key,
                &path?,
                self.system.as_deref(),
            )
        });
        result.map_err(|error| match error.downcast_ref::<known_hosts::Changed>() {
            Some(changed) => {
                log::warn!("{changed}");
                Error::HostKeyChanged {
                    fingerprint: changed.fingerprint.clone(),
                }
            }
            None => Error::HostKeyRejected(format!("{error:#}")),
        })
    }
}

/// Runs blocking file work off the async worker where the runtime allows it.
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|runtime| runtime.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_ssh::HostKeys as _;
    use russh::keys::ssh_key::{
        HashAlg, PrivateKey, PublicKey, certificate, private::Ed25519Keypair,
    };

    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
    }

    #[test]
    fn percent_n_is_the_name_only_when_it_is_a_host_name() {
        let target = newport_ssh::Target {
            host: "10.0.0.5".into(),
            port: 22,
            user: "ops".into(),
        };
        for (name, expected) in [
            ("bastion-1", "nc bastion-1 22"),
            ("Build box", "nc 10.0.0.5 22"),
            ("x; rm -rf ~", "nc 10.0.0.5 22"),
            ("$(touch pwned)", "nc 10.0.0.5 22"),
            ("", "nc 10.0.0.5 22"),
        ] {
            let mut server: crate::model::Server = serde_json::from_value(serde_json::json!({
                "id": uuid::Uuid::new_v4(), "name": name, "sshUser": "ops",
                "sshHost": "10.0.0.5", "sshPort": 22,
            }))
            .unwrap();
            server.name = name.into();
            let line = proxy_command(&server, "nc %n %p")
                .command_for(&target)
                .unwrap();
            assert_eq!(line, expected, "{name:?}");
        }
    }

    fn target(port: u16) -> Target {
        Target {
            host: "Server.Example".into(),
            port,
            user: "user".into(),
        }
    }

    fn offered(key: &PublicKey) -> PublicKeyOrCertificate {
        key.clone().into()
    }

    #[test]
    fn a_new_server_is_trusted_and_saved_under_its_host_and_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let hosts = KnownHosts::at(path.clone());
        let key = key(1).public_key().clone();
        hosts.verify(&target(2222), &offered(&key)).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.starts_with(&format!(
                "[server.example]:2222 {}",
                key.to_openssh().unwrap()
            )),
            "{saved}"
        );
        hosts.verify(&target(2222), &offered(&key)).unwrap();
        assert_eq!(saved, std::fs::read_to_string(&path).unwrap());
    }

    #[test]
    fn a_changed_key_is_refused_with_the_new_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let hosts = KnownHosts::at(dir.path().join("known_hosts"));
        hosts
            .verify(&target(22), &offered(key(1).public_key()))
            .unwrap();
        let changed = key(2).public_key().clone();
        assert_eq!(
            hosts.verify(&target(22), &offered(&changed)),
            Err(Error::HostKeyChanged {
                fingerprint: changed.fingerprint(HashAlg::Sha256).to_string()
            })
        );
        // Another port is another host to known_hosts.
        hosts.verify(&target(2200), &offered(&changed)).unwrap();
    }

    #[test]
    fn a_revoked_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let key = key(1).public_key().clone();
        std::fs::write(
            &path,
            format!("@revoked server.example {}\n", key.to_openssh().unwrap()),
        )
        .unwrap();
        let result = KnownHosts::at(path).verify(&target(22), &offered(&key));
        assert!(
            matches!(&result, Err(Error::HostKeyRejected(reason)) if reason.contains("revoked")),
            "{result:?}"
        );
    }

    #[test]
    fn host_certificates_are_rejected_without_touching_known_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let authority = key(9);
        let mut builder =
            certificate::Builder::new([0; 16], key(1).public_key().key_data().clone(), 0, u64::MAX)
                .unwrap();
        builder
            .cert_type(certificate::CertType::Host)
            .unwrap()
            .valid_principal("server.example")
            .unwrap();
        let certificate = builder.sign(&authority).unwrap();
        let result = KnownHosts::at(path.clone()).verify(&target(22), &certificate.into());
        assert_eq!(
            result,
            Err(Error::HostKeyRejected(
                known_hosts::CERTIFICATES_UNSUPPORTED.into()
            ))
        );
        assert!(!path.exists());
    }

    fn server(proxy: Option<Proxy>) -> Server {
        let mut server: Server = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name": "Inner", "sshUser": "ops",
            "sshHost": "127.0.0.1", "sshPort": 22
        }))
        .unwrap();
        server.proxy = proxy;
        server
    }

    fn dial_target(server: &Server) -> Target {
        Target {
            host: server.ssh_host.clone(),
            port: server.ssh_port,
            user: server.ssh_user.clone(),
        }
    }

    fn no_jump(_: Uuid) -> Result<Link, Error> {
        panic!("only a jump host's link is looked up")
    }

    #[tokio::test]
    async fn a_server_without_a_proxy_dials_its_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut server = server(None);
        server.ssh_port = listener.local_addr().unwrap().port();
        let dialer = dialer(&server, no_jump);
        let approval = watch::channel(false).0;
        let target = dial_target(&server);
        let (dialed, accepted) = tokio::join!(dialer.dial(&target, &approval), listener.accept());
        dialed.unwrap();
        accepted.unwrap();
    }

    #[tokio::test]
    async fn a_proxy_command_runs_with_the_servers_tokens_and_name() {
        use tokio::io::AsyncReadExt;
        let server = server(Some(Proxy::Command("printf %n-%h-%p-%r".into())));
        let dialer = dialer(&server, no_jump);
        let approval = watch::channel(false).0;
        let target = dial_target(&server);
        let mut dialed = dialer.dial(&target, &approval).await.unwrap();
        let mut output = String::new();
        dialed.stream.read_to_string(&mut output).await.unwrap();
        assert_eq!(output, "Inner-127.0.0.1-22-ops");
    }

    #[tokio::test]
    async fn a_server_behind_a_jump_host_uses_the_link_it_is_given() {
        let jump = server(None);
        let behind = server(Some(Proxy::Jump(jump.id)));
        let mut asked = None;
        let dialer = dialer(&behind, |id| {
            asked = Some(id);
            Ok(link_with(
                &jump,
                Arc::new(TcpDialer),
                KnownHosts::at("/nonexistent".into()),
            ))
        });
        assert_eq!(asked, Some(jump.id));
        // The jump host's link keeps the deadlines of its own login.
        assert_eq!(dialer.deadline(std::time::Duration::from_secs(1)), None);
    }

    #[tokio::test]
    async fn a_jump_host_that_cant_be_found_fails_every_dial() {
        let behind = server(Some(Proxy::Jump(Uuid::new_v4())));
        let missing = Error::InvalidJump("it is no longer saved".into());
        let dialer = dialer(&behind, |_| Err(missing.clone()));
        let approval = watch::channel(false).0;
        let target = dial_target(&behind);
        for _ in 0..2 {
            let result = dialer.dial(&target, &approval).await;
            assert_eq!(result.err(), Some(missing.clone()));
        }
        assert_eq!(missing.retry(), newport_ssh::Retry::Never);
    }

    #[test]
    fn login_failures_keep_their_class() {
        assert_eq!(
            login_error(
                &anyhow::Error::new(russh::Error::Disconnect).context(auth::AgentApprovalRequired)
            ),
            Error::ApprovalRequired
        );
        assert_eq!(
            login_error(&anyhow::anyhow!("agent locked").context(auth::SelectedAgentUnavailable)),
            Error::AgentUnavailable
        );
        assert!(matches!(
            login_error(&anyhow::Error::new(std::io::Error::from(
                std::io::ErrorKind::ConnectionReset
            ))),
            Error::Lost(_)
        ));
        assert!(matches!(
            login_error(&anyhow::Error::new(russh::Error::IO(std::io::Error::from(
                std::io::ErrorKind::UnexpectedEof
            )))),
            Error::Lost(_)
        ));
        assert_eq!(
            login_error(&anyhow::anyhow!("SSH key authentication failed.")),
            Error::AuthRefused
        );
        assert_eq!(
            login_error(
                &anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::NotFound))
                    .context("Cannot read identity ~/.ssh/missing")
                    .context(auth::CredentialsUnavailable)
            ),
            Error::Credentials("Cannot read identity ~/.ssh/missing: entity not found".into())
        );
    }
}
