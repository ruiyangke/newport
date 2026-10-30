//! Key-file, agent and password authentication. Explicit key selection never falls through.
use crate::model::{AuthMethod, Server};
use anyhow::{Context, Result, bail};
use russh::{
    client,
    keys::{PrivateKeyWithHashAlg, agent::AgentIdentity, ssh_key::PrivateKey},
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;

// A selected agent can be locked or temporarily decline signing. Keep this
// typed so connection recovery does not depend on a user-facing error message.
#[derive(Debug)]
pub(super) struct SelectedAgentUnavailable;
impl std::fmt::Display for SelectedAgentUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The selected SSH agent key is unavailable. Unlock the agent and check this server’s key selection.")
    }
}
impl std::error::Error for SelectedAgentUnavailable {}

/// A credential on this computer couldn't be used: an identity file that
/// can't be read, or a saved password that can't be found. Fixing it is up to
/// the person, so it isn't retried by itself.
#[derive(Debug)]
pub(crate) struct CredentialsUnavailable;
impl std::fmt::Display for CredentialsUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This server's SSH credentials couldn't be used on this computer.")
    }
}
impl std::error::Error for CredentialsUnavailable {}

#[derive(Debug)]
pub(super) struct AgentApprovalRequired;
impl std::fmt::Display for AgentApprovalRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "SSH key approval was not completed. Retry and approve the request in your SSH agent.",
        )
    }
}
impl std::error::Error for AgentApprovalRequired {}

fn agent_error(server: &Server, error: anyhow::Error) -> anyhow::Error {
    if server.agent_key_fingerprint.is_some() {
        error.context(SelectedAgentUnavailable)
    } else {
        error
    }
}

// Only the local sign request can require user approval. Network exchanges,
// identity enumeration and agent connection must still have a deadline.
struct ApprovalSigner<S> {
    inner: S,
    approval: watch::Sender<bool>,
}
impl<S: russh::Signer + Send> russh::Signer for ApprovalSigner<S>
where
    S::Error: Into<anyhow::Error>,
{
    type Error = anyhow::Error;

    async fn auth_sign(
        &mut self,
        key: &AgentIdentity,
        hash: Option<russh::keys::ssh_key::HashAlg>,
        data: Vec<u8>,
    ) -> Result<Vec<u8>> {
        self.approval.send_replace(true);
        let result = self.inner.auth_sign(key, hash, data).await;
        self.approval.send_replace(false);
        // Retrying a declined/expired sign request in the background can hide
        // the next prompt and trigger the agent's repeated-denial throttling.
        // Keep the cause for diagnostics, but require an explicit user retry.
        result.map_err(|error| error.into().context(AgentApprovalRequired))
    }
}

fn identity_path(path: &str) -> Result<PathBuf> {
    Ok(if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .context("Cannot find home directory")
            .context(CredentialsUnavailable)?
            .join(rest)
    } else {
        path.into()
    })
}
async fn read_identity(path: PathBuf) -> Result<PrivateKey> {
    tokio::task::spawn_blocking(move || {
        russh::keys::load_secret_key(&path, None)
            .map_err(anyhow::Error::from)
            .or_else(|_| PrivateKey::read_openssh_file(&path).map_err(anyhow::Error::from))
            .with_context(|| format!("Cannot read identity {}", path.display()))
    })
    .await
    .context("Identity-file reader task failed")?
}

pub(super) async fn authenticate<H: client::Handler>(
    handle: &mut client::Handle<H>,
    server: &Server,
    approval: watch::Sender<bool>,
) -> Result<()> {
    if server.auth_method == AuthMethod::Password {
        let id = server.id;
        let password = tokio::task::spawn_blocking(move || crate::credentials::get(id))
            .await?
            .map_err(|error| anyhow::Error::msg(error).context(CredentialsUnavailable))?;
        return password_auth(handle, &server.ssh_user, &password).await;
    }
    let explicit = server
        .identity_file
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(identity_path)
        .transpose()?;
    let selected = match explicit.as_ref() {
        Some(path) => Some(
            read_identity(path.clone())
                .await
                .map_err(|error| error.context(CredentialsUnavailable))?,
        ),
        None => None,
    };
    let hash = handle.best_supported_rsa_hash().await?.flatten();
    if let Some(key) = selected.as_ref().filter(|k| !k.is_encrypted())
        && handle
            .authenticate_publickey(
                &server.ssh_user,
                PrivateKeyWithHashAlg::new(Arc::new(key.clone()), hash),
            )
            .await?
            .success()
    {
        return Ok(());
    }
    // Agent protocol over SSH_AUTH_SOCK; no ssh-agent/ssh-add child process.
    let agent_connection = crate::agent_keys::connect(server.agent_source.as_deref()).await;
    if let Err(error) = &agent_connection
        && server.agent_key_fingerprint.is_some()
    {
        return Err(anyhow::anyhow!(error.clone()).context(SelectedAgentUnavailable));
    }
    if let Ok(mut agent) = agent_connection {
        let identities = agent
            .request_identities()
            .await
            .map_err(|error| agent_error(server, error.into()))?;
        let mut agent = ApprovalSigner {
            inner: agent,
            approval,
        };
        for identity in identities {
            if server
                .agent_key_fingerprint
                .as_ref()
                .is_some_and(|selected| {
                    identity
                        .public_key()
                        .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
                        .to_string()
                        != *selected
                })
            {
                continue;
            }
            if selected
                .as_ref()
                .is_some_and(|key| key.public_key().key_data() != identity.public_key().key_data())
            {
                continue;
            }
            if authenticate_identity(
                handle,
                &server.ssh_user,
                identity,
                hash,
                &mut agent,
                server.agent_key_fingerprint.is_some(),
            )
            .await?
            {
                return Ok(());
            }
        }
    }
    if server.agent_key_fingerprint.is_some() {
        return Err(SelectedAgentUnavailable.into());
    }
    if explicit.is_none() {
        let home = dirs::home_dir().context("Cannot find home directory")?;
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            if let Ok(key) = read_identity(home.join(".ssh").join(name)).await
                && !key.is_encrypted()
                && handle
                    .authenticate_publickey(
                        &server.ssh_user,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await?
                    .success()
            {
                return Ok(());
            }
        }
    }
    bail!(
        "SSH key authentication failed. Check the username and identity, or load the matching encrypted key into your SSH agent."
    )
}
async fn authenticate_identity<H: client::Handler, S: russh::Signer<Error = anyhow::Error>>(
    handle: &mut client::Handle<H>,
    user: &str,
    identity: AgentIdentity,
    hash: Option<russh::keys::ssh_key::HashAlg>,
    signer: &mut S,
    selected: bool,
) -> Result<bool> {
    let result = match identity {
        AgentIdentity::PublicKey { key, .. } => {
            handle
                .authenticate_publickey_with(user, key, hash, signer)
                .await
        }
        AgentIdentity::Certificate { certificate, .. } => {
            handle
                .authenticate_certificate_with(user, certificate, hash, signer)
                .await
        }
    }?;
    if result.success() {
        return Ok(true);
    }
    if handle.is_closed() {
        return Err(russh::Error::Disconnect.into());
    }
    if selected {
        bail!("The server rejected the selected SSH key. Check the username and authorized keys.");
    }
    Ok(false)
}

async fn password_auth<H: client::Handler>(
    handle: &mut client::Handle<H>,
    user: &str,
    password: &str,
) -> Result<()> {
    if handle
        .authenticate_password(user, password)
        .await?
        .success()
    {
        return Ok(());
    }
    let mut reply = handle
        .authenticate_keyboard_interactive_start(user, None::<String>)
        .await?;
    // Only a conventional one-password challenge is supported, never echoable/MFA prompts.
    for _ in 0..2 {
        match reply {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(()),
            client::KeyboardInteractiveAuthResponse::InfoRequest { ref prompts, .. }
                if prompts.len() == 1
                    && !prompts[0].echo
                    && prompts[0].prompt.to_ascii_lowercase().contains("password") =>
            {
                reply = handle
                    .authenticate_keyboard_interactive_respond(vec![password.to_owned()])
                    .await?;
            }
            _ => bail!(
                "Password authentication failed or the server requires an interactive challenge."
            ),
        }
    }
    if matches!(reply, client::KeyboardInteractiveAuthResponse::Success) {
        Ok(())
    } else {
        bail!("Password authentication failed.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct TestClient;
    impl client::Handler for TestClient {
        type Error = anyhow::Error;
        async fn check_server_key(
            &mut self,
            _: &russh::keys::PublicKeyOrCertificate,
        ) -> Result<bool> {
            Ok(true)
        }
    }
    #[derive(Clone, Copy)]
    enum Response {
        Reject,
        RequestSignature,
    }
    struct TestServer(Response);
    impl russh::server::Handler for TestServer {
        type Error = anyhow::Error;
        async fn auth_publickey_offered(
            &mut self,
            _: &str,
            _: russh::server::PublicKeyOffer<'_>,
        ) -> Result<russh::server::Auth> {
            match self.0 {
                Response::Reject => Ok(russh::server::Auth::reject()),
                Response::RequestSignature => Ok(russh::server::Auth::Accept),
            }
        }
    }
    struct SlowSigner;
    impl russh::Signer for SlowSigner {
        type Error = anyhow::Error;
        async fn auth_sign(
            &mut self,
            _: &AgentIdentity,
            _: Option<russh::keys::ssh_key::HashAlg>,
            _: Vec<u8>,
        ) -> Result<Vec<u8>> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            bail!("Test agent declined signing")
        }
    }

    // A real SSH exchange over an in-memory transport: no external server,
    // user's keys, or known_hosts changes. Virtual time keeps slow approval fast.
    async fn fixture(
        response: Response,
    ) -> (
        client::Handle<TestClient>,
        AgentIdentity,
        tokio::task::JoinHandle<()>,
    ) {
        let key = PrivateKey::from(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(
            &[7; 32],
        ));
        let identity = AgentIdentity::PublicKey {
            key: key.public_key().clone(),
            comment: String::new(),
        };
        let config = russh::server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        };
        let (local, remote) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(async move {
            let session = russh::server::run_stream(Arc::new(config), remote, TestServer(response))
                .await
                .unwrap();
            let _ = session.await;
        });
        let client = client::connect_stream(Arc::new(client::Config::default()), local, TestClient)
            .await
            .unwrap();
        (client, identity, task)
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejected_key_is_a_refused_login() {
        let (mut handle, identity, task) = fixture(Response::Reject).await;
        let (approval, _waiting) = watch::channel(false);
        let mut signer = ApprovalSigner {
            inner: SlowSigner,
            approval,
        };
        let error = authenticate_identity(&mut handle, "user", identity, None, &mut signer, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("server rejected"), "{error:#}");
        assert_eq!(
            super::super::link_parts::login_error(&error),
            newport_ssh::Error::AuthRefused
        );
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_sign_request_reports_its_wait_and_a_decline_waits_for_retry() {
        let (mut handle, identity, task) = fixture(Response::RequestSignature).await;
        let (approval, mut waiting) = watch::channel(false);
        let mut signer = ApprovalSigner {
            inner: SlowSigner,
            approval,
        };
        let started = tokio::time::Instant::now();
        let sign = authenticate_identity(&mut handle, "user", identity, None, &mut signer, true);
        tokio::pin!(sign);
        tokio::select! {
            _ = &mut sign => panic!("the signer answered at once"),
            changed = waiting.changed() => changed.unwrap(),
        }
        assert!(*waiting.borrow_and_update(), "the wait for approval shows");
        let error = sign.await.unwrap_err();
        assert!(error.is::<AgentApprovalRequired>(), "{error:#}");
        assert!(started.elapsed() >= Duration::from_secs(60));
        assert!(!*waiting.borrow());
        assert_eq!(
            super::super::link_parts::login_error(&error),
            newport_ssh::Error::ApprovalRequired
        );
        task.abort();
    }
}
