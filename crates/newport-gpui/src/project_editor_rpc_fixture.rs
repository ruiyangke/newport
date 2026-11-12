//! Observe typed editor requests without reaching a backend.
use super::{TestRpc, desktop, files};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn connection() -> Value {
    json!([
        "11111111-1111-4111-8111-111111111111",
        "host",
        "user",
        22,
        "publicKey",
        null,
        null,
        null,
        null,
        1,
        "22222222-2222-4222-8222-222222222222"
    ])
}

pub(super) fn observed_rpc(
    observe: impl Fn(&str, Value) -> Result<Value, String> + Send + Sync + 'static,
) -> TestRpc {
    Arc::new(move |operation| {
        let (method, args) = observation(operation);
        observe(method, args)
    })
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
