//! Shared test-only file peer; socket traffic always uses the shipping codec.
use newport_platform::ipc::{self, framed};
use newport_protocol::{
    common,
    desktop::{self, configuration, files, wire},
};
use serde_json::{Value, json};
use std::{
    net::Shutdown,
    time::{Duration, Instant},
};

pub(crate) struct Request {
    pub method: String,
    pub args: Value,
}
pub(crate) struct Peer {
    _reader: framed::Reader,
    writer: framed::Writer,
    request: desktop::Request,
}
impl Peer {
    pub(crate) fn admit(stream: ipc::Stream) -> Self {
        stream.set_nonblocking(false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut writer = framed::Writer::new(stream.try_clone().unwrap());
        let mut reader = framed::Reader::new(stream);
        let frame = reader
            .next_until(desktop::HANDSHAKE_LIMITS.message, deadline)
            .unwrap()
            .unwrap();
        let hello = wire::decode_client(frame.body(), false).unwrap();
        let mut phase = desktop::Exchange::default();
        phase.client(&hello).unwrap();
        assert!(
            matches!(&hello, desktop::ClientMessage::Hello(v) if v.version == desktop::VERSION)
        );
        let ready = desktop::ServerMessage::Ready(Box::new(desktop::Ready {
            contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
            version: desktop::VERSION,
            backend_instance: uuid::Uuid::from_u128(2),
        }));
        phase.server(&ready).unwrap();
        writer
            .send_until(&wire::encode_server(&ready).unwrap(), deadline)
            .unwrap();
        let frame = reader
            .next_until(desktop::MAX_REQUEST, deadline)
            .unwrap()
            .unwrap();
        let message = wire::decode_client(frame.body(), false).unwrap();
        phase.client(&message).unwrap();
        let desktop::ClientMessage::Request(request) = message else {
            panic!("expected request")
        };
        assert!(matches!(&request.operation, desktop::Operation::Files(_)));
        Self {
            _reader: reader,
            writer,
            request: *request,
        }
    }
    pub(crate) fn observation(&self) -> Request {
        let desktop::Operation::Files(operation) = &self.request.operation else {
            unreachable!()
        };
        let (method, args) = match operation.as_ref() {
            files::Operation::List(v) => {
                let mut args = json!({"id":v.server,"operation":v.operation,"path":v.path,"expectedConnection":v.expected.as_ref().map(scope)});
                if v.refresh {
                    args["refresh"] = json!(true);
                }
                ("files_list", args)
            }
            files::Operation::Preview(v) => (
                "files_preview",
                json!({"id":v.server,"operation":v.operation,"path":v.path,"expectedConnection":v.expected.as_ref().map(scope)}),
            ),
            files::Operation::Cancel(v) => ("files_cancel", json!({"operation":v.operation})),
            files::Operation::Progress(v) => ("files_progress", json!({"operation":v.operation})),
            files::Operation::Upload(v) => (
                "files_upload",
                json!({"id":v.server,"operation":v.operation,"path":v.path,"localPath":native_path(&v.local_path),"expectedConnection":scope(&v.expected)}),
            ),
            files::Operation::Download(v) => (
                "files_download",
                json!({"id":v.server,"operation":v.operation,"path":v.path,"localPath":native_path(&v.local_path),"expectedConnection":scope(&v.expected)}),
            ),
            _ => panic!("unexpected file fixture operation"),
        };
        Request {
            method: method.into(),
            args,
        }
    }
    fn respond(&mut self, value: Result<Value, String>) {
        let desktop::Operation::Files(operation) = &self.request.operation else {
            unreachable!()
        };
        let outcome = match value.and_then(|value| lower(operation, value)) {
            Ok(reply) => {
                desktop::Outcome::Completed(Box::new(desktop::Reply::Files(Box::new(reply))))
            }
            Err(message) => failure(message),
        };
        let response = desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: self.request.id,
            outcome,
        }));
        let frame = wire::encode_server_for(&response, &self.request).unwrap_or_else(|error| {
            wire::encode_server_for(
                &desktop::ServerMessage::Response(Box::new(desktop::Response {
                    id: self.request.id,
                    outcome: failure(format!("Invalid file fixture response: {error}")),
                })),
                &self.request,
            )
            .unwrap()
        });
        self.writer
            .send_until(&frame, Instant::now() + Duration::from_secs(2))
            .unwrap();
        self.writer
            .try_clone_stream()
            .unwrap()
            .shutdown(Shutdown::Both)
            .unwrap();
    }
}
fn failure(message: String) -> desktop::Outcome {
    desktop::Outcome::Failed(Box::new(desktop::Failure {
        error: common::Error {
            code: "fixture".into(),
            message,
            retry: common::Retry::No,
        },
        detail: None,
    }))
}
fn scope(v: &configuration::ConnectionScope) -> Value {
    let proxy = v.settings.proxy.as_ref().map(|v| match v {
        configuration::Proxy::Command(v) => json!({"command":v}),
        configuration::Proxy::Jump(v) => json!({"jump":v}),
    });
    json!([
        v.settings.server_id,
        v.settings.ssh_host,
        v.settings.ssh_user,
        v.settings.ssh_port,
        match v.settings.auth_method {
            configuration::AuthMethod::PublicKey => "publicKey",
            configuration::AuthMethod::Password => "password",
        },
        v.settings.identity_file,
        v.settings.agent_source,
        v.settings.agent_key_fingerprint,
        proxy,
        v.connection_revision,
        v.backend_instance
    ])
}
fn native_path(value: &files::NativePath) -> Value {
    json!(
        newport_client::desktop_inputs::local_path(value.clone())
            .unwrap()
            .into_os_string()
    )
}
fn text(value: &Value, field: &str) -> Result<String, String> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("Invalid file fixture {field}"))
}
fn lower(operation: &files::Operation, value: Value) -> Result<files::Reply, String> {
    Ok(match operation {
        files::Operation::Cancel(_) => {
            if !value.is_null() {
                return Err("Invalid file cancellation acknowledgement.".into());
            }
            files::Reply::Cancel(Box::new(common::Present {}))
        }
        files::Operation::List(_) => {
            let entries = value["entries"]
                .as_array()
                .ok_or("Invalid folder listing from the backend.")?
                .iter()
                .map(|v| {
                    let kind = match v["kind"].as_str() {
                        Some("directory") => files::EntryKind::Directory,
                        Some("file") => files::EntryKind::File,
                        Some("symlink") => files::EntryKind::Symlink,
                        Some("other") => files::EntryKind::Other,
                        _ => return Err("Invalid entry kind".into()),
                    };
                    Ok(files::Entry {
                        name: text(v, "name")?,
                        path: text(v, "path")?,
                        kind,
                        size: v["size"].as_u64(),
                        modified: v["modified"]
                            .as_u64()
                            .map(u32::try_from)
                            .transpose()
                            .map_err(|_| "Invalid modified date")?,
                        permissions: v["permissions"].as_str().map(str::to_owned),
                    })
                })
                .collect::<Result<_, String>>()?;
            files::Reply::List(Box::new(files::Listing {
                path: text(&value, "path")?,
                entries,
            }))
        }
        files::Operation::Preview(_) => {
            let kind = match value["kind"].as_str() {
                Some("text") => files::PreviewKind::Text,
                _ => return Err("Invalid file preview from the backend.".into()),
            };
            files::Reply::Preview(Box::new(files::Preview {
                kind,
                mime: text(&value, "mime")?,
                content: files::Content::Text(Box::new(text(&value, "content")?)),
                truncated: value["truncated"].as_bool().unwrap_or(false),
            }))
        }
        files::Operation::Progress(_) => {
            let progress = if value.is_null() {
                None
            } else {
                Some(files::TransferProgress {
                    name: text(&value, "name")?,
                    completed: value["completed"].as_u64().ok_or("Invalid file progress")?,
                    total: if value["total"].is_null() {
                        None
                    } else {
                        Some(
                            value["total"]
                                .as_u64()
                                .ok_or("Invalid file progress total")?,
                        )
                    },
                })
            };
            files::Reply::Progress(Box::new(files::ProgressLookup { progress }))
        }
        files::Operation::Upload(_) => files::Reply::Upload(Box::new(files::TransferResult {
            path: value
                .as_str()
                .map(|v| files::TransferPath::Remote(Box::new(v.to_owned()))),
        })),
        files::Operation::Download(_) => files::Reply::Download(Box::new(files::TransferResult {
            path: value.as_str().map(|v| {
                files::TransferPath::Local(Box::new(newport_client::desktop_inputs::native_path(
                    std::path::Path::new(v),
                )))
            }),
        })),
        _ => return Err("Unexpected file fixture operation".into()),
    })
}

pub(crate) fn reply(mut peer: Peer, value: Result<Value, &str>) {
    peer.respond(value.map_err(str::to_owned));
}

pub(crate) struct Host {
    directory: std::path::PathBuf,
    path: std::path::PathBuf,
    listener: ipc::Listener,
    connection: Value,
}
impl Host {
    pub(crate) fn with_connection(connection: Value) -> Self {
        newport_client::desktop_inputs::connection_scope(&connection)
            .expect("file fixture needs a valid connection scope");
        let directory =
            std::env::temp_dir().join(format!("np-files-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&directory).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = ipc::profile_endpoint(&directory);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // macOS's long temporary path can place the endpoint outside the
            // profile. The client requires that fallback directory to be private.
            std::fs::set_permissions(
                path.parent().unwrap(),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        let listener = ipc::Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            directory,
            path,
            listener,
            connection,
        }
    }
    pub(crate) fn backend(&self) -> newport_client::backend::Backend {
        newport_client::backend::Backend::at_path(self.path.clone())
    }
    pub(crate) fn try_receive(&self) -> Option<(Peer, Request)> {
        let (stream, _) = match self.listener.accept() {
            Ok(pair) => pair,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return None,
            Err(error) => panic!("{error}"),
        };
        let peer = Peer::admit(stream);
        let request = peer.observation();
        assert!(matches!(
            request.method.as_str(),
            "files_list" | "files_cancel"
        ));
        if request.method == "files_list" {
            assert_eq!(request.args["expectedConnection"], self.connection);
            assert_eq!(request.args["id"], self.connection[0]);
        }
        uuid::Uuid::parse_str(request.args["operation"].as_str().unwrap()).unwrap();
        Some((peer, request))
    }
    fn receive(&self, method: &str) -> (Peer, Request) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some((peer, request)) = self.try_receive() {
                assert_eq!(request.method, method);
                return (peer, request);
            }
            assert!(Instant::now() < deadline, "missing {method} RPC");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    pub(crate) fn listing(&self, path: &str) -> Peer {
        let (peer, request) = self.receive("files_list");
        assert_eq!(request.args["path"], path);
        peer
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.path.parent().unwrap());
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
