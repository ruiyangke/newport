//! Client boundary fixtures use the shipping protobuf handshake and real EOF.
use super::{Backend, RepositoryCreation};
use newport_platform::ipc::{self, Listener, Stream};
use newport_protocol::{
    common,
    desktop::{self, wire},
    git,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

mod mutations;
mod reads;

const SERVER: &str = "00000000-0000-4000-8000-000000000001";
fn server() -> Uuid {
    Uuid::parse_str(SERVER).unwrap()
}
struct Host {
    directory: PathBuf,
    path: PathBuf,
    listener: Listener,
}
impl Host {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("np-client-{}", Uuid::new_v4().simple()));
        newport_platform::filesystem::create_private_directory(&directory).unwrap();
        let path = ipc::profile_endpoint(&directory);
        newport_platform::filesystem::create_private_directory(path.parent().unwrap()).unwrap();
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            directory,
            path,
            listener,
        }
    }
    fn receive(&self) -> Peer {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match self.listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(2))
                }
                other => panic!("Expected backend request: {other:?}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        assert_eq!(
            wire::read_hello(&mut socket).unwrap().version,
            desktop::VERSION
        );
        socket
            .write_all(
                &wire::encode_server(&desktop::ServerMessage::Ready(Box::new(desktop::Ready {
                    contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
                    version: desktop::VERSION,
                    backend_instance: Uuid::new_v4(),
                })))
                .unwrap(),
            )
            .unwrap();
        let desktop::ClientMessage::Request(request) =
            wire::read_client(&mut socket, false).unwrap()
        else {
            panic!("expected typed request");
        };
        Peer {
            socket,
            request: *request,
        }
    }
    fn no_request(&self) {
        assert!(
            matches!(self.listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_dir(self.path.parent().unwrap());
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}
struct Peer {
    socket: Stream,
    request: desktop::Request,
}
impl Peer {
    fn outcome(&mut self, outcome: desktop::Outcome) {
        let message = desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: self.request.id,
            outcome,
        }));
        self.socket
            .write_all(&wire::encode_server_for(&message, &self.request).unwrap())
            .unwrap();
    }
    fn reply(&mut self, reply: desktop::Reply) {
        self.outcome(desktop::Outcome::Completed(Box::new(reply)));
    }
    fn fail(&mut self, code: &str, message: &str) {
        self.outcome(desktop::Outcome::Failed(Box::new(desktop::Failure {
            error: common::Error {
                code: code.into(),
                message: message.into(),
                retry: common::Retry::No,
            },
            detail: None,
        })));
    }
    fn git_reply(&mut self, reply: git::Reply) {
        self.reply(desktop::Reply::Git(Box::new(desktop::git::Reply::Request(
            Box::new(desktop::git::GitResult::Reply(Box::new(reply))),
        ))));
    }
}
fn git_request(peer: &Peer) -> &git::Request {
    let desktop::Operation::Git(operation) = &peer.request.operation else {
        panic!("expected Git operation");
    };
    let desktop::git::Operation::Request(args) = operation.as_ref() else {
        panic!("expected Git request");
    };
    assert_eq!(args.server, server());
    assert!(!args.retry_connection);
    &args.request
}
fn pending(host: &Host) {
    let mut peer = host.receive();
    assert!(
        matches!(&peer.request.operation, desktop::Operation::Git(operation) if matches!(operation.as_ref(), desktop::git::Operation::PendingOperations(args) if args.server == server()))
    );
    peer.reply(desktop::Reply::Git(Box::new(
        desktop::git::Reply::PendingOperations(Box::new(desktop::git::Receipts {
            receipts: vec![],
        })),
    )));
}
fn success(id: Uuid) -> git::Reply {
    git::Reply::Operation(Box::new(git::Operation {
        operation_id: id.to_string(),
        repository: "repo".into(),
        payload_hash: "hash".into(),
        state: git::OperationState::Succeeded,
        seq: 1,
        result: Some(git::OperationResult::Refresh(git::Refresh {
            refresh_required: true,
        })),
        error: None,
    }))
}
fn repository(repo_id: &str, root: git::Path) -> git::Reply {
    git::Reply::Open(Box::new(git::Repository {
        repo_id: repo_id.into(),
        common_repo_id: "common".into(),
        root,
        bare: false,
        object_format: "sha1".into(),
        head: git::Head {
            oid: None,
            name: None,
            detached: false,
            unborn: true,
        },
        operation_state: "none".into(),
        integration: None,
        capabilities: git::RepositoryCapabilities {
            read_only: false,
            working_tree: true,
        },
    }))
}
