//! Remote configuration and bounded library-based transfers. Credentials are
//! requested from the server's SSH agent or its configured HTTPS helpers.
use super::{
    branches, journal,
    protocol::{Action, Error},
    repository,
};
use git2::{Cred, CredentialType, Remote, RemoteCallbacks, Repository};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};

fn engine(error: git2::Error) -> Error {
    // Some libgit2 file-lock failures use Generic/Os instead of Locked.
    // Classify the fixed diagnostic prefix without exposing paths or URL secrets.
    let code = if error.class() == git2::ErrorClass::Os
        && error.message().starts_with("failed to lock file '")
    {
        git2::ErrorCode::Locked
    } else {
        error.code()
    };
    let (code, message) = match code {
        git2::ErrorCode::Locked => ("REPOSITORY_BUSY", "Git could not acquire a repository lock. If a previous Git process crashed, inspect its leftover lock files before retrying."),
        git2::ErrorCode::Auth => ("AUTH_REQUIRED", "Remote authentication failed. Configure an SSH agent or an HTTPS credential helper on the server. Interactive login is unavailable in this connection."),
        git2::ErrorCode::Certificate => ("CERTIFICATE_REJECTED", "The remote certificate or SSH host key could not be verified."),
        git2::ErrorCode::NotFound => ("REMOTE_NOT_FOUND", "The remote or repository could not be found."),
        git2::ErrorCode::NotFastForward => ("NON_FAST_FORWARD", "The remote branch has diverged. Fetch and integrate before pushing."),
        git2::ErrorCode::Exists => ("REMOTE_EXISTS", "A remote with that name already exists."),
        _ => ("REMOTE_ERROR", "The remote operation failed. Check connectivity and remote configuration."),
    };
    Error::new(code, message)
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN", "The remote operation may have updated references. Inspect the operation and remote before retrying.")
}
// libgit2 can update tracking refs even when writing FETCH_HEAD fails. Check
// existing locks before any transfer; never remove locks owned by another process.
fn fetch_locks(repo: &Repository) -> Result<(), Error> {
    for (path, label) in [
        (repo.path().join("FETCH_HEAD.lock"), "FETCH_HEAD.lock"),
        (
            repo.commondir().join("packed-refs.lock"),
            "packed-refs.lock",
        ),
    ] {
        match std::fs::symlink_metadata(path) {
            Ok(_) => return Err(Error::new("REPOSITORY_BUSY", format!(
                "Fetch is blocked by {label}. Another Git operation may be running, or a previous crash left this lock behind. Inspect the lock before retrying."
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(_) => return Err(Error::new("REMOTE_ERROR", "Cannot inspect the repository's fetch locks.")),
        }
    }
    Ok(())
}
pub(super) fn name(value: &str) -> Result<(), Error> {
    if value.len() > 256 || value.contains('\0') || !Remote::is_valid_name(value) {
        return Err(Error::invalid("Invalid remote name."));
    }
    Ok(())
}
pub(super) fn validate_url(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(Error::invalid("Invalid remote URL."));
    }
    if value.starts_with('/') {
        return Ok(());
    }
    if !value.contains("://")
        && value.split_once(':').is_some_and(|(host, path)| {
            !host.is_empty() && !host.contains('/') && !host.starts_with('-') && !path.is_empty()
        })
    {
        return Ok(());
    }
    if let Ok(url) = url::Url::parse(value) {
        if ["ssh", "https", "git", "file"].contains(&url.scheme())
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
        {
            return Ok(());
        }
    }
    Err(Error::invalid("Use SSH, HTTPS, git:// or a local path without embedded passwords, query strings or fragments."))
}
/// Existing repository URLs may contain HTTPS credentials. They stay on the
/// server: callers pass only the remote name and configuration hash. New URLs
/// supplied over RPC still use `validate_url`, so secrets never enter journals.
fn validate_configured_url(value: &str) -> Result<(), Error> {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(Error::invalid("Invalid configured remote URL."));
    }
    if let Ok(mut url) = url::Url::parse(value) {
        if url.scheme() == "https" {
            let _ = url.set_password(None);
            return validate_url(url.as_str());
        }
    }
    validate_url(value)
}
pub(super) fn display_url(value: Option<&str>) -> Option<String> {
    value.map(|value| {
        if let Ok(mut url) = url::Url::parse(value) {
            let _ = url.set_password(None);
            if url.scheme() == "https" || url.scheme() == "http" {
                let _ = url.set_username("");
            }
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        } else if validate_url(value).is_ok() {
            value.into()
        } else {
            "[unsupported remote URL]".into()
        }
    })
}
pub(super) fn token(remote: &Remote<'_>) -> Result<String, Error> {
    let fetch: Vec<_> = remote
        .fetch_refspecs()
        .map_err(engine)?
        .iter_bytes()
        .map(|b| b.to_vec())
        .collect();
    let push: Vec<_> = remote
        .push_refspecs()
        .map_err(engine)?
        .iter_bytes()
        .map(|b| b.to_vec())
        .collect();
    Ok(journal::hash(&serde_json::to_vec(&json!({"url":remote.url_bytes(),"pushUrl":remote.pushurl_bytes(),"fetch":fetch,"push":push})).map_err(|_| Error::invalid("Remote configuration is too large."))?))
}
/// Resolve exactly one configured remote without enumerating unrelated remotes.
pub fn selected(repo: &Repository, remote_name: &str) -> Result<Value, Error> {
    name(remote_name)?;
    let remote = repo.find_remote(remote_name).map_err(engine)?;
    describe(&remote, remote_name)
}
fn describe(remote: &Remote<'_>, name: &str) -> Result<Value, Error> {
    Ok(
        json!({"name":name,"url":display_url(remote.url().ok()),"pushUrl":display_url(remote.pushurl().ok().flatten()),"token":token(remote)?}),
    )
}
pub fn list(repo: &Repository) -> Result<Value, Error> {
    let names = repo.remotes().map_err(engine)?;
    if names.len() > 128 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "At most 128 remotes are supported.",
        ));
    }
    let mut entries = Vec::new();
    for value in names.iter() {
        let value = value
            .map_err(engine)?
            .ok_or_else(|| Error::invalid("A remote name is missing."))?;
        let remote = repo.find_remote(value).map_err(engine)?;
        entries.push(describe(&remote, value)?);
    }
    Ok(json!({"entries":entries,"authentication":{"ssh":"server_agent","https":"server_helpers"}}))
}
fn callbacks<'a>(
    repo: &'a Repository,
    deadline: Instant,
    attempted: &'a Cell<bool>,
) -> RemoteCallbacks<'a> {
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(move |url, username, allowed| {
        if Instant::now() > deadline || attempted.replace(true) {
            return Err(git2::Error::new(
                git2::ErrorCode::Auth,
                git2::ErrorClass::Net,
                "Authentication unavailable",
            ));
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) && url.starts_with("https://") {
            return super::credentials::https(repo, url, username, deadline);
        }
        let username = username.unwrap_or("git");
        if allowed.contains(CredentialType::SSH_KEY) {
            Cred::ssh_key_from_agent(username)
        } else if allowed.contains(CredentialType::USERNAME) {
            attempted.set(false);
            Cred::username(username)
        } else {
            Err(git2::Error::new(
                git2::ErrorCode::Auth,
                git2::ErrorClass::Net,
                "Credential prompt required",
            ))
        }
    });
    // The macOS TLS backend cannot load a temporary CA file. The isolated
    // HTTPS test pins its exact certificate instead; never compiled into agents.
    #[cfg(test)]
    if let Ok(path) = std::env::var("NEWPORT_GIT_HTTPS_CERT_DER") {
        let expected = std::fs::read(path).expect("HTTPS fixture certificate");
        callbacks.certificate_check(move |certificate, host| {
            if host == "127.0.0.1"
                && certificate
                    .as_x509()
                    .is_some_and(|cert| cert.data() == expected)
            {
                Ok(git2::CertificateCheckStatus::CertificateOk)
            } else {
                Err(git2::Error::new(
                    git2::ErrorCode::Certificate,
                    git2::ErrorClass::Ssl,
                    "Fixture certificate mismatch",
                ))
            }
        });
    }
    callbacks.transfer_progress(move |_| Instant::now() < deadline);
    callbacks.sideband_progress(move |_| Instant::now() < deadline);
    callbacks
}
/// Prepare a clone privately. Initialize explicitly so external templates and
/// local hardlink optimizations cannot introduce unreviewed files or sharing.
pub(super) fn clone_into(
    url: &str,
    path: &std::path::Path,
    branch: Option<&str>,
    bare: bool,
) -> Result<Value, Error> {
    validate_url(url)?;
    if let Some(branch) = branch {
        branches::name(branch)?;
    }
    let mut init = git2::RepositoryInitOptions::new();
    init.bare(bare)
        .external_template(false)
        .no_reinit(true)
        .initial_head("refs/heads/main");
    let repo = Repository::init_opts(path, &init).map_err(engine)?;
    if repo
        .config()
        .map_err(engine)?
        .get_string("core.worktree")
        .is_ok()
    {
        return Err(Error::new(
            "UNSUPPORTED_CONFIGURATION",
            "Clone has an unexpected worktree override.",
        ));
    }
    verify_tls(&repo)?;
    repo.remote("origin", url).map_err(engine)?;
    let spec = if bare {
        "+refs/heads/*:refs/heads/*"
    } else {
        "+refs/heads/*:refs/remotes/origin/*"
    };
    repo.config()
        .map_err(engine)?
        .set_str("remote.origin.fetch", spec)
        .map_err(engine)?;
    let mut remote = repo.find_remote("origin").map_err(engine)?;
    // Validate after Git's URL rewrite rules as well as before initialization.
    validate_configured_url(remote.url().map_err(engine)?)?;
    let attempted = Cell::new(false);
    let mut fetch = git2::FetchOptions::new();
    fetch
        .remote_callbacks(callbacks(
            &repo,
            Instant::now() + Duration::from_secs(240),
            &attempted,
        ))
        .follow_redirects(git2::RemoteRedirect::None)
        .download_tags(git2::AutotagOption::All);
    remote
        .fetch(&[spec], Some(&mut fetch), Some("clone: Newport"))
        .map_err(engine)?;
    let default_branch = remote
        .default_branch()
        .ok()
        .and_then(|b| b.as_str().ok().map(str::to_owned));
    if branch.is_none()
        && repo
            .references_glob(if bare {
                "refs/heads/*"
            } else {
                "refs/remotes/origin/*"
            })
            .map_err(engine)?
            .next()
            .is_none()
    {
        return Ok(
            json!({"bare":bare,"branch":"main","unborn":true,"defaultBranchSource":"fallback"}),
        );
    }
    let selected = branch.map(str::to_owned).or_else(|| {
        default_branch
            .as_deref()
            .and_then(|s| s.strip_prefix("refs/heads/"))
            .map(str::to_owned)
    });
    let selected = match selected {
        Some(branch) => branch,
        None if repo.references().map_err(engine)?.next().is_none() => {
            return Ok(
                json!({"bare":bare,"branch":"main","unborn":true,"defaultBranchSource":"fallback"}),
            );
        }
        None => {
            return Err(Error::new(
                "BRANCH_REQUIRED",
                "The remote has no usable default branch. Choose a branch explicitly.",
            ))
        }
    };
    branches::name(&selected)?;
    let local_ref = format!("refs/heads/{selected}");
    let source_ref = if bare {
        local_ref.clone()
    } else {
        format!("refs/remotes/origin/{selected}")
    };
    let source = repo.find_reference(&source_ref).map_err(|_| {
        Error::new(
            "BRANCH_NOT_FOUND",
            "The selected branch was not found on the remote.",
        )
    })?;
    let commit = source.peel_to_commit().map_err(engine)?;
    if !bare {
        repo.reference(&local_ref, commit.id(), false, "clone: Newport")
            .map_err(engine)?;
    }
    repo.set_head(&local_ref).map_err(engine)?;
    if !bare {
        repo.find_branch(&selected, git2::BranchType::Local)
            .map_err(engine)?
            .set_upstream(Some(&format!("origin/{selected}")))
            .map_err(engine)?;
        if let Some(default) = default_branch
            .as_deref()
            .and_then(|s| s.strip_prefix("refs/heads/"))
        {
            let target = format!("refs/remotes/origin/{default}");
            if repo.find_reference(&target).is_ok() {
                repo.reference_symbolic(
                    "refs/remotes/origin/HEAD",
                    &target,
                    false,
                    "clone: Newport",
                )
                .map_err(engine)?;
            }
        }
        super::checkout::supported_hook(&repo, "post-checkout")?;
        super::checkout::supported_files(&repo, &commit.tree().map_err(engine)?)?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().safe()))
            .map_err(engine)?;
    }
    Ok(
        json!({"bare":bare,"branch":selected,"oid":commit.id().to_string(),"unborn":false,"defaultBranchSource":if branch.is_some(){"selected"}else{"remote"}}),
    )
}
pub(super) fn verify_tls(repo: &Repository) -> Result<(), Error> {
    let config = repo.config().map_err(engine)?;
    let mut entries = config.entries(None).map_err(engine)?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(engine)?;
        let key = entry.name().map_err(engine)?;
        if key.starts_with("http.")
            && key
                .rsplit('.')
                .next()
                .is_some_and(|v| v.eq_ignore_ascii_case("sslverify"))
            && config.get_bool(key).is_ok_and(|v| !v)
        {
            return Err(Error::new(
                "CERTIFICATE_REJECTED",
                "TLS verification must be enabled for Git transfers.",
            ));
        }
    }
    Ok(())
}

/// Capture one advertisement without downloading objects or updating refs.
pub(super) fn visit_references(
    repo: &Repository,
    remote_name: &str,
    expected_token: &str,
    for_push: bool,
    mut visit: impl FnMut(Value) -> Result<(), Error>,
) -> Result<Value, Error> {
    name(remote_name)?;
    // An independent handle lets us overlay connection policy without writing
    // repository configuration or changing another operation's config view.
    let repo = Repository::open(repo.path()).map_err(engine)?;
    verify_tls(&repo)?;
    let policy = tempfile::NamedTempFile::new()
        .map_err(|_| Error::new("IO_ERROR", "Cannot prepare remote connection policy."))?;
    let mut overlay = git2::Config::open(policy.path()).map_err(engine)?;
    overlay
        .set_bool("http.followRedirects", false)
        .map_err(engine)?;
    let mut config = repo.config().map_err(engine)?;
    config
        .add_file(policy.path(), git2::ConfigLevel::App, true)
        .map_err(engine)?;
    repo.set_config(&config).map_err(engine)?;
    let mut remote = repo.find_remote(remote_name).map_err(engine)?;
    if token(&remote)? != expected_token {
        return Err(Error::new(
            "STALE_REMOTE",
            "Remote configuration changed. Refresh before continuing.",
        ));
    }
    let url = if for_push {
        remote
            .pushurl()
            .map_err(engine)?
            .unwrap_or(remote.url().map_err(engine)?)
    } else {
        remote.url().map_err(engine)?
    };
    validate_configured_url(url)?;
    let attempted = Cell::new(false);
    let deadline = Instant::now() + Duration::from_secs(240);
    let connection = remote
        .connect_auth(
            if for_push {
                git2::Direction::Push
            } else {
                git2::Direction::Fetch
            },
            Some(callbacks(&repo, deadline, &attempted)),
            None,
        )
        .map_err(engine)?;
    let heads = connection.list().map_err(engine)?;
    for head in heads {
        // git2 0.21 exposes these fields only as UTF-8 and panics on invalid
        // encoding. Contain that library limitation at the RPC boundary.
        let (name, symbolic) = std::panic::catch_unwind(|| (head.name(), head.symref_target()))
            .map_err(|_| {
                Error::new(
                    "UNSUPPORTED_ENCODING",
                    "A remote reference name is not UTF-8.",
                )
            })?;
        let kind = if name == "HEAD" {
            "head"
        } else if name.starts_with("refs/heads/") {
            "branch"
        } else if name.starts_with("refs/tags/") {
            if name.ends_with("^{}") {
                "peeled_tag"
            } else {
                "tag"
            }
        } else {
            "other"
        };
        let row = json!({"reference":super::protocol::Path::new(name.as_bytes()),"kind":kind,"oid":{"format":head.oid().object_format().str(),"hex":head.oid().to_string()},"symbolicTarget":symbolic.map(|s| super::protocol::Path::new(s.as_bytes()))});
        if Instant::now() > deadline {
            return Err(Error::new("TIMEOUT", "Remote reference listing timed out."));
        }
        visit(row)?;
    }
    Ok(
        json!({"remote":remote_name,"remoteToken":expected_token,"forPush":for_push,"basis":"remote_advertisement","truncated":false}),
    )
}

#[cfg(test)]
fn references(
    repo: &Repository,
    remote: &str,
    token: &str,
    for_push: bool,
) -> Result<(Vec<Value>, Value), Error> {
    let mut rows = Vec::new();
    let metadata = visit_references(repo, remote, token, for_push, |row| {
        rows.push(row);
        Ok(())
    })?;
    Ok((rows, metadata))
}

pub fn apply(repo: &Repository, action: &Action, expected: &str) -> Result<Value, Error> {
    apply_with_fetch(repo, action, expected, None)
}

fn apply_with_fetch(
    repo: &Repository,
    action: &Action,
    expected: &str,
    fetched: Option<&mut dyn FnMut(&Remote<'_>)>,
) -> Result<Value, Error> {
    if repository::fingerprint(repo)? != expected {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "Refresh the repository before changing its remotes.",
        ));
    }
    if let Action::RemoteAdd {
        name: remote_name,
        url,
    } = action
    {
        name(remote_name)?;
        validate_url(url)?;
        match repo.find_remote(remote_name) {
            Ok(_) => {
                return Err(Error::new(
                    "REMOTE_EXISTS",
                    "A remote with that name already exists.",
                ))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => {}
            Err(e) => return Err(engine(e)),
        }
        repo.remote(remote_name, url).map_err(|_| unknown())?;
        return Ok(json!({"remote":remote_name,"refreshRequired":true}));
    }
    let (remote_name, expected_token) = match action {
        Action::RemoteRename {
            name,
            expected_token,
            ..
        } => (name, expected_token),
        Action::RemoteSetUrl {
            name,
            expected_token,
            ..
        }
        | Action::RemoteRemove {
            name,
            expected_token,
        } => (name, expected_token),
        Action::Fetch {
            remote,
            expected_token,
            ..
        }
        | Action::BranchDeleteRemote {
            remote,
            expected_token,
            ..
        }
        | Action::PushWithLease {
            remote,
            expected_token,
            ..
        }
        | Action::Push {
            remote,
            expected_token,
            ..
        }
        | Action::TagDeleteRemote {
            remote,
            expected_token,
            ..
        }
        | Action::TagPush {
            remote,
            expected_token,
            ..
        } => (remote, expected_token),
        _ => return Err(Error::invalid("Not a remote operation.")),
    };
    name(remote_name)?;
    let mut remote = repo.find_remote(remote_name).map_err(engine)?;
    if token(&remote)? != *expected_token {
        return Err(Error::new(
            "STALE_REMOTE",
            "Remote configuration changed. Refresh before continuing.",
        ));
    }
    match action {
        Action::RemoteRename { new_name, .. } => {
            super::remote_rename::apply(repo, remote_name, new_name, expected_token, expected)
        }
        Action::RemoteSetUrl { url, .. } => {
            validate_url(url)?;
            repo.remote_set_url(remote_name, url)
                .map_err(|_| unknown())?;
            Ok(json!({"refreshRequired":true}))
        }
        Action::RemoteRemove { .. } => {
            repo.remote_delete(remote_name).map_err(|_| unknown())?;
            Ok(json!({"refreshRequired":true}))
        }
        Action::Fetch { .. }
        | Action::Push { .. }
        | Action::BranchDeleteRemote { .. }
        | Action::TagDeleteRemote { .. }
        | Action::PushWithLease { .. }
        | Action::TagPush { .. } => {
            let is_push = matches!(
                action,
                Action::Push { .. }
                    | Action::PushWithLease { .. }
                    | Action::BranchDeleteRemote { .. }
                    | Action::TagDeleteRemote { .. }
                    | Action::TagPush { .. }
            );
            if is_push {
                super::checkout::supported_hook(repo, "pre-push")?;
            }
            let url = if is_push {
                remote
                    .pushurl()
                    .map_err(engine)?
                    .unwrap_or(remote.url().map_err(engine)?)
            } else {
                remote.url().map_err(engine)?
            };
            validate_configured_url(url)?;
            let smart_transport = url.starts_with("ssh://")
                || url.starts_with("https://")
                || url.starts_with("git://")
                || (!url.contains("://")
                    && !url.starts_with("file:")
                    && !url.starts_with('/')
                    && url.split_once(':').is_some_and(|(host, path)| {
                        !host.is_empty() && !host.contains('/') && !path.is_empty()
                    }));
            if matches!(
                action,
                Action::PushWithLease { .. }
                    | Action::BranchDeleteRemote { .. }
                    | Action::TagDeleteRemote { .. }
            ) && !smart_transport
            {
                return Err(Error::new("UNSUPPORTED_TRANSPORT","Leased remote updates require SSH, HTTPS or git://; libgit2 local transport cannot enforce its final compare-and-swap."));
            }
            verify_tls(repo)?;
            let deadline = Instant::now() + Duration::from_secs(240);
            let attempted = Cell::new(false);
            if let Action::Fetch { prune, .. } = action {
                fetch_locks(repo)?;
                let updated = Cell::new(0usize);
                let mut callbacks = callbacks(repo, deadline, &attempted);
                callbacks.update_tips(|_, _, _| {
                    updated.set(updated.get() + 1);
                    true
                });
                let mut options = git2::FetchOptions::new();
                options
                    .update_fetchhead(true)
                    .remote_callbacks(callbacks)
                    .follow_redirects(git2::RemoteRedirect::None)
                    .prune(if *prune {
                        git2::FetchPrune::On
                    } else {
                        git2::FetchPrune::Off
                    })
                    .download_tags(git2::AutotagOption::Auto);
                remote
                    .fetch(&[] as &[&str], Some(&mut options), Some("fetch: Newport"))
                    .map_err(|e| {
                        let mut error = engine(e);
                        if updated.get() > 0 {
                            error.message = format!(
                                "Fetch stopped after updating some local tracking references; branch integration was not started. {}",
                                error.message
                            );
                        }
                        error
                    })?;
                if let Some(fetched) = fetched {
                    fetched(&remote);
                }
                Ok(
                    json!({"remote":remote_name,"updatedReferences":updated.get(),"refreshRequired":true}),
                )
            } else {
                let (oid, destination, mut result) = match action {
                    Action::Push {
                        branch,
                        expected_oid,
                        destination_branch,
                        ..
                    }
                    | Action::PushWithLease {
                        branch,
                        expected_oid,
                        destination_branch,
                        ..
                    } => {
                        branches::name(branch)?;
                        branches::name(destination_branch)?;
                        let oid = branches::oid(expected_oid)?;
                        if repo
                            .find_branch(branch, git2::BranchType::Local)
                            .map_err(engine)?
                            .get()
                            .target()
                            != Some(oid)
                        {
                            return Err(Error::new(
                                "STALE_REFERENCE",
                                "The local branch moved. Refresh before pushing.",
                            ));
                        }
                        (
                            oid,
                            format!("refs/heads/{destination_branch}"),
                            json!({"destinationBranch":destination_branch}),
                        )
                    }
                    Action::BranchDeleteRemote {
                        branch,
                        expected_oid,
                        ..
                    } => {
                        branches::name(branch)?;
                        if branches::oid(expected_oid)?.is_zero() {
                            return Err(Error::invalid(
                                "Select an existing remote branch commit before deleting it.",
                            ));
                        }
                        (
                            git2::Oid::ZERO_SHA1,
                            format!("refs/heads/{branch}"),
                            json!({"destinationBranch":branch,"deleted":true}),
                        )
                    }
                    Action::TagDeleteRemote {
                        name, expected_oid, ..
                    } => {
                        let reference = super::tags::reference(name)?;
                        if super::tags::oid(expected_oid)?.is_zero() {
                            return Err(Error::invalid(
                                "Select an existing remote tag object before deleting it.",
                            ));
                        }
                        (
                            git2::Oid::ZERO_SHA1,
                            reference,
                            json!({"destinationTag":name,"deleted":true}),
                        )
                    }
                    Action::TagPush {
                        name, expected_oid, ..
                    } => {
                        let reference = super::tags::reference(name)?;
                        let oid = super::tags::oid(expected_oid)?;
                        if repo.find_reference(&reference).map_err(engine)?.target() != Some(oid) {
                            return Err(Error::new(
                                "STALE_REFERENCE",
                                "The local tag changed. Refresh before pushing.",
                            ));
                        }
                        (oid, reference, json!({"destinationTag":name}))
                    }
                    _ => unreachable!(),
                };
                let lease = match action {
                    Action::PushWithLease {
                        expected_remote_oid,
                        ..
                    } => Some(branches::oid(expected_remote_oid)?),
                    Action::BranchDeleteRemote { expected_oid, .. }
                    | Action::TagDeleteRemote { expected_oid, .. } => {
                        Some(branches::oid(expected_oid)?)
                    }
                    _ => None,
                };
                let lease_refused = Cell::new(false);
                let tag_push = matches!(action, Action::TagPush { .. });
                let tag_exists = Cell::new(false);
                let started = Cell::new(false);
                let statuses = RefCell::new(Vec::new());
                let mut callbacks = callbacks(repo, deadline, &attempted);
                callbacks.push_negotiation(|updates| {
                    if let Some(expected_remote) = lease {
                        if updates.len() != 1
                            || updates[0].src() != expected_remote
                            || updates[0].dst() != oid
                            || updates[0].dst_refname_bytes() != destination.as_bytes()
                        {
                            lease_refused.set(true);
                            return Err(git2::Error::from_str("Remote lease changed"));
                        }
                    }
                    if tag_push
                        && updates
                            .iter()
                            .any(|update| !update.src().is_zero() && update.src() != update.dst())
                    {
                        tag_exists.set(true);
                        return Err(git2::Error::from_str("Remote tag exists"));
                    }
                    if Instant::now() > deadline {
                        return Err(git2::Error::from_str("Transfer deadline"));
                    }
                    started.set(true);
                    Ok(())
                });
                callbacks.push_update_reference(|name, status| {
                    statuses
                        .borrow_mut()
                        .push((name.to_owned(), status.is_none()));
                    Ok(())
                });
                let mut options = git2::PushOptions::new();
                options
                    .remote_callbacks(callbacks)
                    .follow_redirects(git2::RemoteRedirect::None);
                let force = if lease.is_some() { "+" } else { "" };
                let spec = if matches!(
                    action,
                    Action::BranchDeleteRemote { .. } | Action::TagDeleteRemote { .. }
                ) {
                    format!(":{destination}")
                } else {
                    format!("{force}{oid}:{destination}")
                };
                remote.push(&[spec], Some(&mut options)).map_err(|e| {
                    if lease_refused.get() {
                        Error::new("STALE_REMOTE_REFERENCE","The remote reference no longer matches the expected object. Refresh and review it before another remote update.")
                    } else if tag_exists.get() {
                        Error::new("REMOTE_TAG_EXISTS", "The remote already has a different tag with this name. It was not overwritten.")
                    } else if started.get() && e.code() != git2::ErrorCode::NotFastForward {
                        unknown()
                    } else {
                        engine(e)
                    }
                })?;
                if statuses.borrow().iter().any(|(_, ok)| !ok) {
                    return Err(Error::new(
                        "PUSH_REJECTED",
                        "The remote rejected the reference update.",
                    ));
                }
                if let Some(expected_remote) = lease {
                    result["leaseMatched"] = true.into();
                    result["expectedRemoteOid"] = expected_remote.to_string().into();
                }
                result["remote"] = json!(remote_name);
                result["oid"] = if oid.is_zero() {
                    Value::Null
                } else {
                    json!(oid.to_string())
                };
                result["refreshRequired"] = true.into();
                Ok(result)
            }
        }
        _ => Err(Error::invalid("Not a remote operation.")),
    }
}

fn unfetched_branch() -> Error {
    Error::new("REMOTE_BRANCH_NOT_FOUND", "The selected branch was not fetched. Check this remote's fetch refspecs and refresh its branches.")
}

/// Match Git's single-star negative refspec without interpreting it as a regex.
fn excluded_source(remote: &Remote<'_>, source: &str) -> bool {
    remote.refspecs().any(|spec| {
        if spec.direction() != git2::Direction::Fetch {
            return false;
        }
        let Some(pattern) = spec.bytes().strip_prefix(b"^") else {
            return false;
        };
        let source = source.as_bytes();
        match pattern.iter().position(|byte| *byte == b'*') {
            None => pattern == source,
            Some(star) => {
                source.len() >= pattern.len() - 1
                    && source.starts_with(&pattern[..star])
                    && source.ends_with(&pattern[star + 1..])
            }
        }
    })
}

// libgit2 strips credentials and default ports when recording FETCH_HEAD.
// Normalize both URLs for this comparison; local paths and scp syntax remain exact.
fn fetch_url_matches(recorded: &[u8], configured: &[u8]) -> bool {
    fn normalized(bytes: &[u8]) -> Option<url::Url> {
        let text = std::str::from_utf8(bytes).ok()?;
        if !text.contains("://") {
            return None;
        }
        let mut url = url::Url::parse(text).ok()?;
        let _ = url.set_username("");
        let _ = url.set_password(None);
        if matches!(
            (url.scheme(), url.port()),
            ("ssh", Some(22)) | ("git", Some(9418))
        ) {
            let _ = url.set_port(None);
        }
        Some(url)
    }
    recorded == configured
        || match (normalized(recorded), normalized(configured)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
}

/// The fetch's advertisement is still attached to its handle after disconnect.
/// Cross-check the freshly written FETCH_HEAD instead of guessing a local ref
/// destination; external Git writes cannot substitute an older/different OID.
fn fetched_source(
    repo: &Repository,
    remote: &Remote<'_>,
    source: &str,
) -> Result<git2::Oid, Error> {
    let mut advertised = None;
    for head in remote.list().map_err(engine)? {
        let name = std::panic::catch_unwind(|| head.name()).map_err(|_| {
            Error::new(
                "UNSUPPORTED_ENCODING",
                "A remote reference name is not UTF-8.",
            )
        })?;
        if name != source {
            continue;
        }
        if advertised.is_some_and(|oid| oid != head.oid()) {
            return Err(unfetched_branch());
        }
        advertised = Some(head.oid());
    }
    let advertised = advertised
        .filter(|oid| !oid.is_zero())
        .ok_or_else(unfetched_branch)?;
    let mut found = false;
    let mut mismatched = false;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        repo.fetchhead_foreach(|name, url, oid, _| {
            if name == source {
                if !fetch_url_matches(url, remote.url_bytes()) || *oid != advertised {
                    mismatched = true;
                } else {
                    found = true;
                }
            }
            true
        })
    }))
    .map_err(|_| unfetched_branch())?
    .map_err(|_| unfetched_branch())?;
    if !found || mismatched {
        return Err(unfetched_branch());
    }
    Ok(advertised)
}

/// Fetch and integrate as one durable invocation. A refused integration is a
/// known partial result: fetched tracking refs remain, local HEAD does not move.
pub fn pull_fast_forward(
    repo: &Repository,
    remote: &str,
    expected_token: &str,
    remote_branch: &str,
    expected: &str,
) -> Result<Value, Error> {
    name(remote)?;
    branches::name(remote_branch)?;
    let head = repo.head().map_err(engine)?;
    if !head.is_branch() {
        return Err(Error::new(
            "DETACHED_HEAD",
            "Switch to a branch before pulling.",
        ));
    }
    super::checkout::supported_hook(repo, "post-merge")?;
    let source = format!("refs/heads/{remote_branch}");
    if excluded_source(&repo.find_remote(remote).map_err(engine)?, &source) {
        return Err(unfetched_branch());
    }
    let mut fetched = None;
    apply_with_fetch(
        repo,
        &Action::Fetch {
            remote: remote.into(),
            expected_token: expected_token.into(),
            prune: true,
        },
        expected,
        Some(&mut |remote| {
            fetched = Some(fetched_source(repo, remote, &source));
        }),
    )?;
    let integrate = || -> Result<Value, Error> {
        let oid = fetched.ok_or_else(unfetched_branch)??;
        super::checkout::fast_forward(repo, &oid.to_string(), expected)
    };
    match integrate() {
        Ok(mut result) => {
            result["fetched"] = json!(true);
            result["remote"] = json!(remote);
            result["remoteBranch"] = json!(remote_branch);
            Ok(result)
        }
        Err(mut error) => {
            error.message = format!(
                "Fetch completed; branch integration did not complete. {}",
                error.message
            );
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commit(repo: &Repository, text: &str) -> git2::Oid {
        let blob = repo.blob(text.as_bytes()).unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("file", blob, 0o100644).unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            text,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn run(repo: &Repository, action: Action) -> Result<Value, Error> {
        apply(repo, &action, &repository::fingerprint(repo).unwrap())
    }
    fn remote_token(repo: &Repository) -> String {
        token(&repo.find_remote("origin").unwrap()).unwrap()
    }
    #[test]
    fn remote_listing_refuses_invalid_encoding_and_disabled_tls() {
        let temp = tempfile::tempdir().unwrap();
        let source = Repository::init_bare(temp.path().join("source")).unwrap();
        let tip = commit(&source, "source");
        let local = Repository::init(temp.path().join("local")).unwrap();
        local
            .remote("origin", source.path().to_str().unwrap())
            .unwrap();
        let mut packed = format!("{tip} refs/heads/invalid-").into_bytes();
        packed.extend_from_slice(b"\xff\n");
        std::fs::write(source.path().join("packed-refs"), packed).unwrap();
        assert_eq!(
            references(&local, "origin", &remote_token(&local), false)
                .unwrap_err()
                .code,
            "UNSUPPORTED_ENCODING"
        );
        std::fs::remove_file(source.path().join("packed-refs")).unwrap();
        local
            .config()
            .unwrap()
            .set_bool("http.sslVerify", false)
            .unwrap();
        assert_eq!(
            references(&local, "origin", &remote_token(&local), false)
                .unwrap_err()
                .code,
            "CERTIFICATE_REJECTED"
        );
        local
            .config()
            .unwrap()
            .set_bool("http.sslVerify", true)
            .unwrap();
        assert!(!references(&local, "origin", &remote_token(&local), false)
            .unwrap()
            .0
            .is_empty());
    }
    #[test]
    fn leased_push_refuses_local_transport_without_changing_remote() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
        remote.set_head("refs/heads/main").unwrap();
        let base = commit(&remote, "base");
        let local =
            Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local")).unwrap();
        let next = commit(&local, "next");
        let request = Action::PushWithLease {
            remote: "origin".into(),
            expected_token: remote_token(&local),
            branch: "main".into(),
            expected_oid: next.to_string(),
            destination_branch: "main".into(),
            expected_remote_oid: base.to_string(),
        };
        assert_eq!(
            run(&local, request).unwrap_err().code,
            "UNSUPPORTED_TRANSPORT"
        );
        assert_eq!(remote.head().unwrap().target(), Some(base));
        remote
            .tag_lightweight("release", &remote.find_object(base, None).unwrap(), false)
            .unwrap();
        let delete_tag = Action::TagDeleteRemote {
            remote: "origin".into(),
            expected_token: remote_token(&local),
            name: "release".into(),
            expected_oid: base.to_string(),
        };
        assert_eq!(
            run(&local, delete_tag).unwrap_err().code,
            "UNSUPPORTED_TRANSPORT"
        );
        assert_eq!(
            remote.find_reference("refs/tags/release").unwrap().target(),
            Some(base)
        );
        let delete = Action::BranchDeleteRemote {
            remote: "origin".into(),
            expected_token: remote_token(&local),
            branch: "main".into(),
            expected_oid: base.to_string(),
        };
        assert_eq!(
            run(&local, delete).unwrap_err().code,
            "UNSUPPORTED_TRANSPORT"
        );
        assert_eq!(remote.head().unwrap().target(), Some(base));
    }
    #[test]
    fn push_tags_preserves_annotations_and_never_replaces_remote_tags() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
        let base = commit(&remote, "base");
        let local =
            Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local")).unwrap();
        let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
        let annotation = local
            .tag(
                "v1",
                &local.find_object(base, None).unwrap(),
                &signature,
                "release notes",
                false,
            )
            .unwrap();
        let push = |name: &str, oid: git2::Oid| Action::TagPush {
            remote: "origin".into(),
            expected_token: remote_token(&local),
            name: name.into(),
            expected_oid: oid.to_string(),
        };
        run(&local, push("v1", annotation)).unwrap();
        assert_eq!(
            remote.find_reference("refs/tags/v1").unwrap().target(),
            Some(annotation)
        );
        assert_eq!(
            remote.find_tag(annotation).unwrap().message_bytes(),
            Some(b"release notes".as_slice())
        );
        run(&local, push("v1", annotation)).unwrap();
        assert_eq!(
            run(&local, push("v1", base)).unwrap_err().code,
            "STALE_REFERENCE"
        );
        let next = commit(&local, "next");
        let replacement = local
            .tag(
                "v1",
                &local.find_object(next, None).unwrap(),
                &signature,
                "different release",
                true,
            )
            .unwrap();
        assert_eq!(
            run(&local, push("v1", replacement)).unwrap_err().code,
            "REMOTE_TAG_EXISTS"
        );
        assert_eq!(
            remote.find_reference("refs/tags/v1").unwrap().target(),
            Some(annotation)
        );
        local
            .tag_lightweight("light", &local.find_object(base, None).unwrap(), false)
            .unwrap();
        run(&local, push("light", base)).unwrap();
        local
            .tag_lightweight("light", &local.find_object(next, None).unwrap(), true)
            .unwrap();
        assert_eq!(
            run(&local, push("light", next)).unwrap_err().code,
            "REMOTE_TAG_EXISTS"
        );
        assert_eq!(
            remote.find_reference("refs/tags/light").unwrap().target(),
            Some(base)
        );
    }

    #[test]
    fn fetch_respects_repository_refspec() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
        remote.set_head("refs/heads/main").unwrap();
        let oid = commit(&remote, "base");
        remote
            .reference("refs/heads/private", oid, false, "fixture")
            .unwrap();
        let local = Repository::init(temp.path().join("local")).unwrap();
        local
            .remote("origin", remote.path().to_str().unwrap())
            .unwrap();
        local
            .config()
            .unwrap()
            .set_str(
                "remote.origin.fetch",
                "+refs/heads/main:refs/remotes/origin/main",
            )
            .unwrap();
        run(
            &local,
            Action::Fetch {
                remote: "origin".into(),
                expected_token: remote_token(&local),
                prune: false,
            },
        )
        .unwrap();
        assert_eq!(
            local
                .find_reference("refs/remotes/origin/main")
                .unwrap()
                .target(),
            Some(oid)
        );
        assert!(local.find_reference("refs/remotes/origin/private").is_err());
    }

    #[test]
    fn fetch_prune_push_and_reject_non_fast_forward() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
        remote.set_head("refs/heads/main").unwrap();
        commit(&remote, "base");
        let local =
            Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local")).unwrap();
        let next = commit(&local, "next");
        let result = run(
            &local,
            Action::Push {
                remote: "origin".into(),
                expected_token: remote_token(&local),
                branch: "main".into(),
                expected_oid: next.to_string(),
                destination_branch: "main".into(),
            },
        )
        .unwrap();
        assert_eq!(result["oid"], next.to_string());
        assert_eq!(remote.head().unwrap().target(), Some(next));
        let divergent = commit(&remote, "server change");
        let ours = commit(&local, "local change");
        assert_eq!(
            run(
                &local,
                Action::Push {
                    remote: "origin".into(),
                    expected_token: remote_token(&local),
                    branch: "main".into(),
                    expected_oid: ours.to_string(),
                    destination_branch: "main".into()
                }
            )
            .unwrap_err()
            .code,
            "NON_FAST_FORWARD"
        );
        remote
            .branch("extra", &remote.find_commit(divergent).unwrap(), false)
            .unwrap();
        run(
            &local,
            Action::Fetch {
                remote: "origin".into(),
                expected_token: remote_token(&local),
                prune: true,
            },
        )
        .unwrap();
        assert_eq!(
            local
                .find_reference("refs/remotes/origin/main")
                .unwrap()
                .target(),
            Some(divergent)
        );
        assert!(local.find_reference("refs/remotes/origin/extra").is_ok());
        remote
            .find_branch("extra", git2::BranchType::Local)
            .unwrap()
            .delete()
            .unwrap();
        run(
            &local,
            Action::Fetch {
                remote: "origin".into(),
                expected_token: remote_token(&local),
                prune: true,
            },
        )
        .unwrap();
        assert!(local.find_reference("refs/remotes/origin/extra").is_err());
        assert_eq!(local.head().unwrap().target(), Some(ours));
    }
    #[test]
    fn pull_reports_fetch_locks_and_recovers_after_the_lock_is_removed() {
        for lock in [
            "FETCH_HEAD.lock",
            "packed-refs.lock",
            "refs/remotes/origin/main.lock",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
            remote.set_head("refs/heads/main").unwrap();
            let base = commit(&remote, "base");
            let local =
                Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local"))
                    .unwrap();
            let next = commit(&remote, "server update");
            remote
                .reference("refs/heads/aaa-before-locked", next, true, "fixture")
                .unwrap();
            let lock_path = local.path().join(lock);
            std::fs::write(&lock_path, "existing lock").unwrap();
            let error = pull_fast_forward(
                &local,
                "origin",
                &remote_token(&local),
                "main",
                &repository::fingerprint(&local).unwrap(),
            )
            .unwrap_err();
            assert_eq!(error.code, "REPOSITORY_BUSY", "{lock}: {error:?}");
            if lock == "refs/remotes/origin/main.lock" {
                assert!(error.message.starts_with("Fetch stopped after updating"));
            }
            assert_eq!(local.head().unwrap().target(), Some(base));
            assert_eq!(
                std::fs::read(local.workdir().unwrap().join("file")).unwrap(),
                b"base"
            );
            assert_eq!(std::fs::read(&lock_path).unwrap(), b"existing lock");
            std::fs::remove_file(lock_path).unwrap();
            let result = pull_fast_forward(
                &local,
                "origin",
                &remote_token(&local),
                "main",
                &repository::fingerprint(&local).unwrap(),
            )
            .unwrap();
            assert_eq!(result["fastForwarded"], true);
            assert_eq!(local.head().unwrap().target(), Some(next));
        }
    }

    #[test]
    fn pull_fast_forward_integrates_and_reports_fetched_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
        remote.set_head("refs/heads/main").unwrap();
        commit(&remote, "base");
        let local =
            Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local")).unwrap();
        local
            .config()
            .unwrap()
            .set_str(
                "core.hooksPath",
                local.path().join("hooks").to_str().unwrap(),
            )
            .unwrap();
        let next = commit(&remote, "server update");
        let result = pull_fast_forward(
            &local,
            "origin",
            &remote_token(&local),
            "main",
            &repository::fingerprint(&local).unwrap(),
        )
        .unwrap();
        assert_eq!(result["fetched"], true);
        assert_eq!(result["fastForwarded"], true);
        let local = Repository::open(temp.path().join("local")).unwrap();
        assert_eq!(local.head().unwrap().target(), Some(next));
        assert_eq!(local.head().unwrap().shorthand().unwrap(), "main");
        assert_eq!(
            std::fs::read(local.workdir().unwrap().join("file")).unwrap(),
            b"server update"
        );
        std::fs::write(local.workdir().unwrap().join("file"), "my local edit").unwrap();
        let another = commit(&remote, "another server update");
        let error = pull_fast_forward(
            &local,
            "origin",
            &remote_token(&local),
            "main",
            &repository::fingerprint(&local).unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.code, "CHECKOUT_CONFLICT");
        assert!(error.message.starts_with("Fetch completed"));
        assert_eq!(local.head().unwrap().target(), Some(next));
        assert_eq!(
            local
                .find_reference("refs/remotes/origin/main")
                .unwrap()
                .target(),
            Some(another)
        );
        assert_eq!(
            std::fs::read(local.workdir().unwrap().join("file")).unwrap(),
            b"my local edit"
        );
    }
    #[test]
    fn pull_uses_fetched_source_with_custom_and_source_only_refspecs() {
        for spec in [
            "+refs/heads/*:refs/newport/*",
            "+refs/heads/main:refs/heads/cache-main",
            "refs/heads/main",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
            remote.set_head("refs/heads/main").unwrap();
            let base = commit(&remote, "base");
            let local =
                Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local"))
                    .unwrap();
            local
                .config()
                .unwrap()
                .set_str(
                    "core.hooksPath",
                    local.path().join("hooks").to_str().unwrap(),
                )
                .unwrap();
            local
                .config()
                .unwrap()
                .set_str("remote.origin.fetch", spec)
                .unwrap();
            let next = commit(&remote, "new tip");
            let result = pull_fast_forward(
                &local,
                "origin",
                &remote_token(&local),
                "main",
                &repository::fingerprint(&local).unwrap(),
            )
            .unwrap();
            assert_eq!(
                local.head().unwrap().target(),
                Some(next),
                "{spec}: {result}"
            );
            assert_eq!(
                local
                    .find_reference("refs/remotes/origin/main")
                    .unwrap()
                    .target(),
                Some(base),
                "the stale default tracking ref is not the fetched source"
            );
            // An unchanged second fetch still supplies the selected source.
            pull_fast_forward(
                &local,
                "origin",
                &remote_token(&local),
                "main",
                &repository::fingerprint(&local).unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn pull_refuses_excluded_branches_even_when_stale_tracking_refs_exist() {
        for negative in [None, Some("^refs/heads/main"), Some("^refs/heads/ma*")] {
            let temp = tempfile::tempdir().unwrap();
            let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
            remote.set_head("refs/heads/main").unwrap();
            let base = commit(&remote, "base");
            remote
                .reference("refs/heads/other", base, false, "fixture")
                .unwrap();
            let local =
                Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local"))
                    .unwrap();
            local
                .config()
                .unwrap()
                .set_str(
                    "core.hooksPath",
                    local.path().join("hooks").to_str().unwrap(),
                )
                .unwrap();
            commit(&remote, "stale fetched tip");
            local
                .find_remote("origin")
                .unwrap()
                .fetch(&[] as &[&str], None, None)
                .unwrap();
            commit(&remote, "latest tip");
            let mut config = local.config().unwrap();
            if let Some(spec) = negative {
                config
                    .set_multivar("remote.origin.fetch", "^$", spec)
                    .unwrap();
            } else {
                config
                    .set_str(
                        "remote.origin.fetch",
                        "+refs/heads/other:refs/remotes/origin/other",
                    )
                    .unwrap();
            }
            let result = pull_fast_forward(
                &local,
                "origin",
                &remote_token(&local),
                "main",
                &repository::fingerprint(&local).unwrap(),
            );
            assert!(
                result.is_err(),
                "excluded branch was integrated: {negative:?} {result:?}"
            );
            assert_eq!(local.head().unwrap().target(), Some(base));
            assert_eq!(
                std::fs::read(local.workdir().unwrap().join("file")).unwrap(),
                b"base"
            );
        }
    }

    #[test]
    fn fetch_head_url_comparison_handles_sanitized_credentials() {
        for (recorded, configured) in [
            ("ssh://host/repo", "ssh://user@host:22/repo"),
            ("ssh://host:2222/repo", "ssh://user@host:2222/repo"),
            ("https://host/repo", "https://user@host:443/repo"),
            ("git://host/repo", "git://host:9418/repo"),
            ("user@host:repo", "user@host:repo"),
            ("/tmp/repo", "/tmp/repo"),
        ] {
            assert!(fetch_url_matches(
                recorded.as_bytes(),
                configured.as_bytes()
            ));
        }
        for recorded in [
            "ssh://other/repo",
            "ssh://host/other",
            "ssh://host:2222/repo",
            "https://host/repo",
        ] {
            assert!(!fetch_url_matches(
                recorded.as_bytes(),
                b"ssh://user@host/repo"
            ));
        }
    }

    #[test]
    fn fetched_source_rejects_missing_malformed_ambiguous_and_rewritten_fetch_heads() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Repository::init_bare(temp.path().join("remote.git")).unwrap();
        remote.set_head("refs/heads/main").unwrap();
        let base = commit(&remote, "base");
        let local =
            Repository::clone(remote.path().to_str().unwrap(), temp.path().join("local")).unwrap();
        let next = commit(&remote, "next");
        let mut fetch = local.find_remote("origin").unwrap();
        fetch.fetch(&[] as &[&str], None, None).unwrap();
        let path = local.path().join("FETCH_HEAD");
        let original = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            fetched_source(&local, &fetch, "refs/heads/main").unwrap(),
            next
        );
        // Overlapping positive refspecs may record the same source more than once.
        std::fs::write(&path, format!("{original}{original}")).unwrap();
        assert_eq!(
            fetched_source(&local, &fetch, "refs/heads/main").unwrap(),
            next
        );
        let stale = original.replace(&next.to_string(), &base.to_string());
        for invalid in [
            String::new(),
            "not a FETCH_HEAD record\n".into(),
            stale.clone(),
            format!("{original}{stale}"),
            original.replace(
                remote.path().to_str().unwrap().trim_end_matches('/'),
                "/different-remote.git",
            ),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert_eq!(
                fetched_source(&local, &fetch, "refs/heads/main")
                    .unwrap_err()
                    .code,
                "REMOTE_BRANCH_NOT_FOUND"
            );
        }
        std::fs::remove_file(path).unwrap();
        assert!(fetched_source(&local, &fetch, "refs/heads/main").is_err());
        assert_eq!(local.head().unwrap().target(), Some(base));
    }

    #[test]
    #[ignore = "Run scripts/test-git-https.py for a disposable authenticated TLS server"]
    fn authenticated_https_transfer() {
        let url = std::env::var("NEWPORT_GIT_HTTPS_URL").unwrap();
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path().join("local")).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        repo.remote("origin", &url).unwrap();
        let mut config = repo.config().unwrap();
        config.set_multivar("credential.helper", ".*", "").unwrap();
        assert_eq!(
            run(
                &repo,
                Action::Fetch {
                    remote: "origin".into(),
                    expected_token: remote_token(&repo),
                    prune: false
                }
            )
            .unwrap_err()
            .code,
            "AUTH_REQUIRED"
        );
        config
            .set_multivar(
                "credential.helper",
                "^$",
                "!f() { printf 'username=fixture\\npassword=fixture-token\\n'; }; f",
            )
            .unwrap();
        run(
            &repo,
            Action::Fetch {
                remote: "origin".into(),
                expected_token: remote_token(&repo),
                prune: false,
            },
        )
        .unwrap();
        let oid = commit(&repo, "authenticated transfer");
        run(
            &repo,
            Action::Push {
                remote: "origin".into(),
                expected_token: remote_token(&repo),
                branch: "main".into(),
                expected_oid: oid.to_string(),
                destination_branch: "main".into(),
            },
        )
        .unwrap();
        if let Ok(mut reference) = repo.find_reference("refs/remotes/origin/main") {
            reference.delete().unwrap();
        }
        run(
            &repo,
            Action::Fetch {
                remote: "origin".into(),
                expected_token: remote_token(&repo),
                prune: false,
            },
        )
        .unwrap();
        assert_eq!(
            repo.find_reference("refs/remotes/origin/main")
                .unwrap()
                .target(),
            Some(oid)
        );
        // Existing HTTPS push/fetch URLs can hold credentials. No helper is
        // needed, and none of these secrets may enter the returned JSON.
        config.set_multivar("credential.helper", ".*", "").unwrap();
        config
            .set_multivar(
                "credential.helper",
                "^$",
                "!touch helper-was-called; exit 1",
            )
            .unwrap();
        let mut authenticated = url::Url::parse(&url).unwrap();
        authenticated.set_username("fixture").unwrap();
        authenticated.set_password(Some("fixture%2Dtoken")).unwrap();
        repo.remote_set_pushurl("origin", Some(authenticated.as_str()))
            .unwrap();
        let started = Instant::now();
        let (advertised, _) = references(&repo, "origin", &remote_token(&repo), true).unwrap();
        assert!(advertised
            .iter()
            .any(|row| row["reference"]["display"] == "refs/heads/main"));
        let advertisement_ms = started.elapsed().as_secs_f64() * 1000.0;
        let next = commit(&repo, "embedded credential transfer");
        let started = Instant::now();
        let result = run(
            &repo,
            Action::Push {
                remote: "origin".into(),
                expected_token: remote_token(&repo),
                branch: "main".into(),
                expected_oid: next.to_string(),
                destination_branch: "main".into(),
            },
        )
        .unwrap();
        let push_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert!(!result.to_string().contains("fixture-token"));
        assert!(!list(&repo).unwrap().to_string().contains("fixture%2Dtoken"));
        repo.remote_set_url("origin", authenticated.as_str())
            .unwrap();
        run(
            &repo,
            Action::Fetch {
                remote: "origin".into(),
                expected_token: remote_token(&repo),
                prune: false,
            },
        )
        .unwrap();
        assert_eq!(
            repo.find_reference("refs/remotes/origin/main")
                .unwrap()
                .target(),
            Some(next)
        );
        assert!(!temp.path().join("local/helper-was-called").exists());
        authenticated
            .set_password(Some("incorrect-private-token"))
            .unwrap();
        repo.remote_set_pushurl("origin", Some(authenticated.as_str()))
            .unwrap();
        let error = references(&repo, "origin", &remote_token(&repo), true).unwrap_err();
        assert_eq!(error.code, "AUTH_REQUIRED");
        assert!(!error.message.contains("incorrect-private-token"));
        assert!(!temp.path().join("local/helper-was-called").exists());
        println!("HTTPS embedded credentials: advertisement_ms={advertisement_ms:.2}, push_ms={push_ms:.2}; helper calls=0 for successful URL authentication");
    }

    #[test]
    #[ignore = "Requires outbound HTTPS to the public GitHub test repository"]
    fn anonymous_https_fetch() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.remote("origin", "https://github.com/octocat/Hello-World.git")
            .unwrap();
        run(
            &repo,
            Action::Fetch {
                remote: "origin".into(),
                expected_token: remote_token(&repo),
                prune: false,
            },
        )
        .unwrap();
        assert!(repo
            .references_glob("refs/remotes/origin/*")
            .unwrap()
            .next()
            .is_some());
    }
    #[test]
    fn configured_https_credentials_stay_server_side() {
        assert!(validate_configured_url("https://user:secret@example.test/repo").is_ok());
        assert!(validate_configured_url("https://user:se%40cret@example.test/repo").is_ok());
        for address in [
            "ssh://user:secret@example.test/repo",
            "http://user:secret@example.test/repo",
            "https://user:secret@example.test/repo?token=secret",
            "https://user:secret@example.test/repo#secret",
        ] {
            let error = validate_configured_url(address).unwrap_err();
            assert!(!error.message.contains("secret"));
        }
        assert!(validate_url("https://user:secret@example.test/repo").is_err());
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.remote("origin", "https://user:secret@example.test/repo")
            .unwrap();
        repo.remote_set_pushurl(
            "origin",
            Some("https://user:other-secret@example.test/repo"),
        )
        .unwrap();
        let before = remote_token(&repo);
        let response = list(&repo).unwrap().to_string();
        assert!(!response.contains("secret"));
        assert!(!response.contains("user"));
        repo.remote_set_pushurl("origin", Some("https://user:changed@example.test/repo"))
            .unwrap();
        assert_ne!(before, remote_token(&repo));
    }

    #[test]
    fn selected_remote_matches_listing_redacts_and_ignores_list_limit() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.remote(
            "origin",
            "https://user:secret@example.test/repo?token=secret",
        )
        .unwrap();
        repo.remote_set_pushurl("origin", Some("https://user:pushsecret@example.test/repo"))
            .unwrap();
        let initial = selected(&repo, "origin").unwrap();
        assert_eq!(initial, list(&repo).unwrap()["entries"][0]);
        assert!(!initial.to_string().contains("secret"));
        assert_eq!(
            selected(&repo, "missing").unwrap_err().code,
            "REMOTE_NOT_FOUND"
        );
        assert!(selected(&repo, "bad\0name").is_err());
        // Append fixture configuration in one write, outside the measured path.
        let config = repo.path().join("config");
        let mut contents = std::fs::read_to_string(&config).unwrap();
        for i in 0..2000 {
            contents.push_str(&format!(
                "\n[remote \"fixture-{i}\"]\nurl = https://example.test/{i}\n"
            ));
        }
        std::fs::write(config, contents).unwrap();
        assert_eq!(selected(&repo, "origin").unwrap(), initial);
        assert_eq!(
            selected(&repo, "fixture-1999").unwrap()["name"],
            "fixture-1999"
        );
        assert_eq!(list(&repo).unwrap_err().code, "LIMIT_EXCEEDED");
        repo.remote_set_url("origin", "https://example.test/changed")
            .unwrap();
        assert_ne!(
            selected(&repo, "origin").unwrap()["token"],
            initial["token"]
        );
    }

    #[test]
    fn remote_configuration_preconditions_and_redaction() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        run(
            &repo,
            Action::RemoteAdd {
                name: "origin".into(),
                url: "ssh://git@example.test/repo".into(),
            },
        )
        .unwrap();
        let initial = remote_token(&repo);
        run(
            &repo,
            Action::RemoteSetUrl {
                name: "origin".into(),
                url: "https://example.test/repo".into(),
                expected_token: initial.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            run(
                &repo,
                Action::RemoteRemove {
                    name: "origin".into(),
                    expected_token: initial
                }
            )
            .unwrap_err()
            .code,
            "STALE_REMOTE"
        );
        run(
            &repo,
            Action::RemoteRemove {
                name: "origin".into(),
                expected_token: remote_token(&repo),
            },
        )
        .unwrap();
        assert!(list(&repo).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty());
        repo.remote(
            "private",
            "https://user:secret@example.test/repo?token=secret",
        )
        .unwrap();
        assert!(!list(&repo).unwrap().to_string().contains("secret"));
        assert!(validate_url("https://user:secret@example.test/repo").is_err());
        assert!(validate_url("https://user@example.test/repo").is_ok());
        assert!(name("bad\0remote").is_err());
        assert!(validate_url("git@example.test:repo.git").is_ok());
        assert!(validate_url("example.test:/repo.git").is_ok());
        repo.remote("origin", "https://example.test/repo").unwrap();
        repo.config()
            .unwrap()
            .set_bool("http.https://example.test.sslVerify", false)
            .unwrap();
        assert_eq!(
            run(
                &repo,
                Action::Fetch {
                    remote: "origin".into(),
                    expected_token: remote_token(&repo),
                    prune: false
                }
            )
            .unwrap_err()
            .code,
            "CERTIFICATE_REJECTED"
        );
    }
}
