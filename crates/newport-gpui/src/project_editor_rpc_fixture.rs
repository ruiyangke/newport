//! Observe typed editor requests without reaching a backend.
use super::{TestRpc, desktop, files};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn connection() -> Option<desktop::configuration::ConnectionScope> {
    use desktop::configuration::{AuthMethod, ConnectionScope, ConnectionSettings};
    Some(ConnectionScope {
        settings: ConnectionSettings {
            server_id: uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            ssh_host: "host".into(),
            ssh_user: "user".into(),
            ssh_port: 22,
            auth_method: AuthMethod::PublicKey,
            identity_file: None,
            agent_source: None,
            agent_key_fingerprint: None,
            proxy: None,
        },
        connection_revision: 1,
        backend_instance: uuid::Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
    })
}

/// [`connection`], as observed requests carry it.
pub(super) fn observed_connection() -> Value {
    scope(&connection().unwrap())
}

pub(super) fn observed_rpc(
    observe: impl Fn(&str, Value) -> Result<files::Reply, String> + Send + Sync + 'static,
) -> TestRpc {
    Arc::new(move |operation| {
        let (method, args) = observation(operation);
        observe(method, args)
    })
}

/// A whole text file, as a read answers with it.
pub(super) fn document(content: &str, revision: &str) -> files::Document {
    files::Document {
        path: String::new(),
        content: content.into(),
        revision: revision.into(),
        page: None,
        cached: false,
    }
}

/// One section of a large text file.
pub(super) fn section(
    content: &str,
    revision: &str,
    offset: u64,
    next: Option<u64>,
    size: u64,
) -> files::Document {
    files::Document {
        page: Some(files::Page {
            offset,
            next,
            size,
            reason: files::PageReason::Size,
        }),
        ..document(content, revision)
    }
}

/// A text read's reply for `path`.
pub(super) fn read(path: &Value, content: &str, revision: &str) -> Result<files::Reply, String> {
    reply(files::Document {
        path: path.as_str().unwrap_or_default().into(),
        ..document(content, revision)
    })
}

/// A text read's reply carrying `document`.
pub(super) fn reply(document: files::Document) -> Result<files::Reply, String> {
    Ok(files::Reply::ReadText(Box::new(document)))
}

/// A folder listing's reply for `path`, naming each entry by its path.
pub(super) fn listing(
    path: &Value,
    entries: impl IntoIterator<Item = (impl AsRef<str>, files::EntryKind)>,
) -> Result<files::Reply, String> {
    Ok(files::Reply::List(Box::new(files::Listing {
        path: path.as_str().unwrap_or_default().into(),
        entries: entries
            .into_iter()
            .map(|(path, kind)| files::Entry {
                name: path.as_ref().rsplit('/').next().unwrap_or_default().into(),
                path: path.as_ref().into(),
                kind,
                size: None,
                modified: None,
                permissions: None,
            })
            .collect(),
    })))
}

/// A listing's reply for an empty folder at `path`.
pub(super) fn empty_listing(path: &Value) -> Result<files::Reply, String> {
    listing(path, Vec::<(&str, files::EntryKind)>::new())
}

/// A project index's reply.
pub(super) fn index(paths: &[&str]) -> Result<files::Reply, String> {
    Ok(files::Reply::ProjectIndex(Box::new(files::Index {
        paths: paths.iter().map(|path| (*path).into()).collect(),
        skipped: 0,
    })))
}

/// Watch events naming changed `paths`.
pub(super) fn events(paths: &[&str]) -> files::WatchReply {
    files::WatchReply {
        mode: files::WatchMode::Native,
        cursor: None,
        paths: paths.iter().map(|path| (*path).into()).collect(),
        reset: false,
    }
}

fn scope(value: &desktop::configuration::ConnectionScope) -> Value {
    use desktop::configuration::{AuthMethod, Proxy};
    let settings = &value.settings;
    let proxy = settings.proxy.as_ref().map(|proxy| match proxy {
        Proxy::Command(command) => json!({"command":command}),
        Proxy::Jump(server) => json!({"jump":server}),
    });
    json!([
        settings.server_id,
        settings.ssh_host,
        settings.ssh_user,
        settings.ssh_port,
        match settings.auth_method {
            AuthMethod::PublicKey => "publicKey",
            AuthMethod::Password => "password",
        },
        settings.identity_file,
        settings.agent_source,
        settings.agent_key_fingerprint,
        proxy,
        value.connection_revision,
        value.backend_instance
    ])
}

fn observation(operation: desktop::Operation) -> (&'static str, Value) {
    match operation {
        desktop::Operation::Files(operation) => match *operation {
            files::Operation::List(args) => (
                "files_list",
                json!({
                    "id":args.server,"operation":args.operation,"path":args.path,
                    "expectedConnection":args.expected.as_ref().map(scope),
                    "refresh":args.refresh,"projectRoot":args.project_root,
                }),
            ),
            files::Operation::ReadText(args) => {
                let mut value = json!({"id":args.server,"operation":args.operation,"path":args.path,
                    "expectedConnection":scope(&args.expected),"root":args.root,"allowCached":args.allow_cached});
                if let Some(revision) = args.revision {
                    value["revision"] = json!(revision);
                }
                if let Some(offset) = args.offset {
                    value["offset"] = json!(offset);
                }
                ("files_read_text", value)
            }
            files::Operation::WriteText(args) => (
                "files_write_text",
                json!({
                    "id":args.server,"operation":args.operation,"path":args.path,
                    "expectedConnection":scope(&args.expected),"root":args.root,
                    "content":args.content,"revision":args.revision,
                }),
            ),
            files::Operation::ProjectIndex(args) => (
                "files_project_index",
                json!({
                    "id":args.server,"operation":args.operation,"root":args.root,
                    "expectedConnection":scope(&args.expected),
                }),
            ),
            files::Operation::Watch(args) => (
                "files_watch",
                json!({
                    "id":args.server,"operation":args.operation,"root":args.root,"watch":args.watch,
                    "expectedConnection":scope(&args.expected),"directories":args.directories,"cursor":args.cursor,
                }),
            ),
            _ => panic!("unexpected file request"),
        },
        desktop::Operation::Git(operation) => {
            let desktop::git::Operation::Request(args) = *operation else {
                panic!("unexpected Git operation")
            };
            (
                "git_request",
                json!({"serverId":args.server,"request":args.request}),
            )
        }
        _ => panic!("unexpected editor request"),
    }
}

/// A fake Git read service for a code workspace's typed reads.
pub(super) fn observed_git(
    reply: impl Fn(&newport_protocol::git::Request) -> Result<newport_protocol::git::Reply, String>
    + Send
    + Sync
    + 'static,
) -> super::TestGit {
    std::sync::Arc::new(move |request| reply(&request))
}
