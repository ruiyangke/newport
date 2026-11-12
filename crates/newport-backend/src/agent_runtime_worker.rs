//! A feature channel pins its negotiated runtime until the channel is released.
use super::{Control, protocol};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::OwnedSemaphorePermit,
};

pub(super) fn endpoint_belongs_to_runtime(endpoint: &str, worker_socket: &str) -> bool {
    // Both paths were validated as Linux socket paths; backslashes are names.
    endpoint != worker_socket
        && endpoint
            .rsplit_once('/')
            .zip(worker_socket.rsplit_once('/'))
            .is_some_and(|((directory, _), (runtime_directory, _))| directory == runtime_directory)
}

#[derive(Clone)]
pub(crate) struct Handle(pub(super) Arc<Control>);
impl Handle {
    pub(crate) fn is_draining(&self) -> bool {
        *self.0.drain.borrow()
    }
    pub(crate) async fn draining(&self) {
        let mut drain = self.0.drain.subscribe();
        while !*drain.borrow_and_update() {
            if drain.changed().await.is_err() {
                return;
            }
        }
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
    pub(crate) fn hello(&self) -> &protocol::Hello {
        &self.0.hello
    }
    /// Verify the selected build at its immutable path before executing a
    /// helper outside worker admission. Keeping this handle pins its runtime.
    pub(crate) async fn verified_executable(
        &self,
        link: &newport_ssh::Link,
    ) -> Result<String, super::Error> {
        if self.is_closed() {
            return Err(super::Error::Interrupted(
                "Selected runtime disconnected".into(),
            ));
        }
        let hello = self.hello();
        if !newport_protocol::product::valid_build(&hello.build_sha256) {
            return Err(super::Error::Unavailable(
                "Invalid selected runtime build identity.".into(),
            ));
        }
        let product = super::verify_candidate(link, &hello.build_sha256)
            .await
            .map_err(super::Error::Unavailable)?;
        let executable = selected_executable(hello, &product)?;
        if self.is_closed() {
            return Err(super::Error::Interrupted(
                "Selected runtime disconnected".into(),
            ));
        }
        Ok(executable)
    }
}
pub(crate) struct Worker {
    pub(crate) runtime: Handle,
    pub(crate) id: uuid::Uuid,
    pub(crate) endpoint: Option<newport_protocol::worker::admission::Endpoint>,
    pub(super) _permit: OwnedSemaphorePermit,
}
impl Worker {
    pub(crate) fn stream(self, stream: crate::service_channel::Stream) -> WorkerStream {
        WorkerStream {
            stream,
            worker: self,
        }
    }
}
pub struct WorkerStream {
    stream: crate::service_channel::Stream,
    worker: Worker,
}
impl WorkerStream {
    pub(crate) fn uses_session_slot(&self) -> bool {
        self.stream.uses_session_slot()
    }
    pub(crate) fn is_draining(&self) -> bool {
        self.worker.runtime.is_draining()
    }
    /// The fate of the link connection under the worker's channel.
    pub(crate) fn fate(&self) -> Option<&newport_ssh::Fate> {
        self.stream.fate()
    }
    fn check(&self) -> io::Result<()> {
        if self.worker.runtime.is_closed() {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Needle runtime disconnected",
            ))
        } else {
            Ok(())
        }
    }
}
impl AsyncRead for WorkerStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Err(error) = self.check() {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for WorkerStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Err(error) = self.check() {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// Both the announced identity and its staged executable must agree. Neither
/// untrusted hash text nor a mutable default alias becomes a shell command.
fn selected_executable(
    hello: &protocol::Hello,
    product: &newport_protocol::product::Product,
) -> Result<String, super::Error> {
    if !newport_protocol::product::valid_build(&hello.build_sha256)
        || product.variant != hello.product
        || product.control_versions != hello.versions
        || product.services
            != hello
                .services
                .iter()
                .map(|(name, capability)| (name.clone(), capability.versions.clone()))
                .collect()
    {
        return Err(super::Error::Unavailable(
            "Selected runtime does not match its staged executable. Update Needle.".into(),
        ));
    }
    Ok(format!(
        "\"$HOME/.local/share/newport/needle/builds/{}/newport-agent\"",
        hello.build_sha256
    ))
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    #[test]
    fn worker_endpoints_keep_linux_directory_identity_on_every_platform() {
        assert!(endpoint_belongs_to_runtime(
            "/run/a/b/channel.sock",
            "/run/a/b/worker.sock"
        ));
        assert!(endpoint_belongs_to_runtime(
            r"/run/a\b/channel.sock",
            r"/run/a\b/worker.sock"
        ));
        for endpoint in [
            "/run/a/b/worker.sock",
            "/run/other/channel.sock",
            r"/run/a\b/channel.sock",
        ] {
            assert!(!endpoint_belongs_to_runtime(
                endpoint,
                "/run/a/b/worker.sock"
            ));
        }
    }

    #[test]
    fn immutable_helper_never_uses_the_default_and_checks_selected_product() {
        let capability = protocol::Capability {
            contract_id: None,
            versions: vec![1],
            unavailable: None,
        };
        let mut hello = protocol::Hello {
            versions: vec![protocol::VERSION],
            instance_id: uuid::Uuid::new_v4().to_string(),
            build_sha256: "a".repeat(64),
            agent_version: None,
            worker_socket: "/tmp/runtime.sock".into(),
            product: newport_protocol::product::Variant::Full,
            services: [
                ("terminal".into(), capability.clone()),
                ("terminal-holder".into(), capability),
            ]
            .into(),
            dependencies: Default::default(),
            limits: protocol::Limits {
                max_control_bytes: protocol::MAX_FRAME,
                max_inflight: 1,
                max_workers: 1,
                max_registrations: 1,
            },
        };
        let product = newport_protocol::product::Product {
            variant: hello.product,
            control_versions: hello.versions.clone(),
            services: hello
                .services
                .iter()
                .map(|(name, capability)| (name.clone(), capability.versions.clone()))
                .collect(),
        };
        let executable = selected_executable(&hello, &product).unwrap();
        assert_eq!(
            executable,
            format!(
                "\"$HOME/.local/share/newport/needle/builds/{}/newport-agent\"",
                "a".repeat(64)
            )
        );
        assert!(!executable.contains(".local/bin"));
        hello.product = newport_protocol::product::Variant::Core;
        assert!(selected_executable(&hello, &product).is_err());
        hello.product = product.variant;
        hello.services.remove("terminal-holder");
        assert!(selected_executable(&hello, &product).is_err());
        hello.build_sha256 = "$(false)".into();
        assert!(selected_executable(&hello, &product).is_err());
    }
}
