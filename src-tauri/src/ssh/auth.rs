//! Key-file, agent and password authentication. Explicit key selection never falls through.
use super::Handler;
use crate::model::{AuthMethod, Server};
use anyhow::{bail, Context, Result};
use russh::{
    client,
    keys::{agent::AgentIdentity, ssh_key::PrivateKey, PrivateKeyWithHashAlg},
};
use std::{path::PathBuf, sync::Arc};

fn identity_path(path: &str) -> Result<PathBuf> {
    Ok(if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .context("Cannot find home directory")?
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

pub(super) async fn authenticate(
    handle: &mut client::Handle<Handler>,
    server: &Server,
) -> Result<()> {
    if server.auth_method == AuthMethod::Password {
        let id = server.id;
        let password = tokio::task::spawn_blocking(move || crate::credentials::get(id))
            .await?
            .map_err(anyhow::Error::msg)?;
        return password_auth(handle, &server.ssh_user, &password).await;
    }
    let explicit = server
        .identity_file
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(identity_path)
        .transpose()?;
    let selected = match explicit.as_ref() {
        Some(path) => Some(read_identity(path.clone()).await?),
        None => None,
    };
    let hash = handle.best_supported_rsa_hash().await?.flatten();
    if let Some(key) = selected.as_ref().filter(|k| !k.is_encrypted()) {
        if handle
            .authenticate_publickey(
                &server.ssh_user,
                PrivateKeyWithHashAlg::new(Arc::new(key.clone()), hash),
            )
            .await?
            .success()
        {
            return Ok(());
        }
    }
    // Agent protocol over SSH_AUTH_SOCK; no ssh-agent/ssh-add child process.
    let agent_connection = crate::agent_keys::connect(server.agent_source.as_deref()).await;
    if server.agent_key_fingerprint.is_some() && agent_connection.is_err() {
        bail!("Selected SSH agent is unavailable. Open the agent and enable SSH access.");
    }
    if let Ok(mut agent) = agent_connection {
        for identity in agent.request_identities().await? {
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
            let result = match identity {
                AgentIdentity::PublicKey { key, .. } => {
                    handle
                        .authenticate_publickey_with(&server.ssh_user, key, hash, &mut agent)
                        .await?
                }
                AgentIdentity::Certificate { certificate, .. } => {
                    handle
                        .authenticate_certificate_with(
                            &server.ssh_user,
                            certificate,
                            hash,
                            &mut agent,
                        )
                        .await?
                }
            };
            if result.success() {
                return Ok(());
            }
        }
    }
    if server.agent_key_fingerprint.is_some() {
        bail!("The selected agent key is unavailable or was rejected. Unlock the agent and check this server’s key selection.");
    }
    if explicit.is_none() {
        let home = dirs::home_dir().context("Cannot find home directory")?;
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            if let Ok(key) = read_identity(home.join(".ssh").join(name)).await {
                if !key.is_encrypted()
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
    }
    bail!("SSH key authentication failed. Check the username and identity, or load the matching encrypted key into your SSH agent.")
}
async fn password_auth(
    handle: &mut client::Handle<Handler>,
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
