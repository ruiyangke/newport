//! Explicit terminal link gestures, scoped to the PTY's original server owner.
use crate::{desktop_integration::Desktop, manager::Shared, model::Status, terminal::Sessions};
use newport_domain::{forwarding, model::Tunnel};
use std::{
    net::{Ipv4Addr, Ipv6Addr, TcpListener},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

struct Destination {
    url: url::Url,
    host: String,
    port: u16,
}

fn destination(value: &str) -> Result<Option<Destination>, String> {
    let parsed = url::Url::parse(value).map_err(|_| "Invalid link URL.")?;
    let host = match parsed.host() {
        Some(url::Host::Domain(host))
            if host.trim_end_matches('.').eq_ignore_ascii_case("localhost") =>
        {
            "127.0.0.1".into()
        }
        Some(url::Host::Ipv4(ip)) if ip.is_loopback() || ip.is_unspecified() => {
            if ip.is_unspecified() {
                Ipv4Addr::LOCALHOST.to_string()
            } else {
                ip.to_string()
            }
        }
        Some(url::Host::Ipv6(ip)) if ip.is_loopback() || ip.is_unspecified() => {
            Ipv6Addr::LOCALHOST.to_string()
        }
        _ => return Ok(None),
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Server links with embedded credentials cannot be forwarded.".into());
    }
    let port = parsed
        .port_or_known_default()
        .filter(|port| *port != 0)
        .ok_or("Invalid server link port.")?;
    Ok(Some(Destination {
        url: parsed,
        host,
        port,
    }))
}

impl Destination {
    fn forwarded_url(&self, port: u16) -> Result<String, String> {
        let mut url = self.url.clone();
        url.set_host(Some("127.0.0.1"))
            .map_err(|_| "Invalid local endpoint.")?;
        url.set_port(Some(port))
            .map_err(|_| "Invalid local port.")?;
        Ok(url.into())
    }
}

// Planning is advisory: the forwarding owner performs the authoritative bind.
// Never open the browser on a port merely because another process occupies it.
fn available_port(planned: u16, tunnels: &[Tunnel]) -> Result<u16, String> {
    if TcpListener::bind((Ipv4Addr::LOCALHOST, planned)).is_ok() {
        return Ok(planned);
    }
    for _ in 0..32 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| format!("Cannot allocate a local forward: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        if !tunnels.iter().any(|t| {
            t.direction == newport_domain::model::Direction::Local
                && (t.local_port..=t.local_port_end.unwrap_or(t.local_port)).contains(&port)
        }) {
            return Ok(port);
        }
    }
    Err("No available local port for the server link.".into())
}

pub async fn open_terminal_link(
    state: &Shared,
    terminals: &Sessions,
    desktop: Desktop,
    terminal: Uuid,
    value: String,
) -> Result<(), String> {
    let value = newport_domain::external_link::http_url(&value)
        .ok_or("Only http and https links can be opened.")?;
    let Some(destination) = destination(&value)? else {
        return super::open_url(desktop, value).await;
    };
    let owner = terminals.link_owner(terminal)?;
    tokio::time::timeout(Duration::from_secs(45), async {
        let url = prepare(state, &owner, &destination).await?;
        // Closing or replacing the originating terminal while connecting cancels
        // browser dispatch, without undoing an already saved forward.
        let current = terminals.link_owner(terminal)?;
        if !Arc::ptr_eq(&current, &owner) || owner.is_revoked() {
            return Err("The terminal connection changed before its link could open.".into());
        }
        super::open_url(desktop, url).await
    })
    .await
    .map_err(|_| {
        "Timed out preparing the server link. Check the forward in Connections.".to_owned()
    })?
}

async fn prepare(
    state: &Shared,
    owner: &Arc<crate::server_session::ServerSession>,
    destination: &Destination,
) -> Result<String, String> {
    let server = owner.server();
    let scope = newport_domain::connection_scope::connection(
        &serde_json::to_value(server).map_err(|e| e.to_string())?,
        &serde_json::json!({"instanceId":state.instance_id,"runtime":{"connectionRevisions":{server.id.to_string():owner.revision()}}}),
    );
    let snapshot = state.snapshot();
    if owner.is_revoked() || snapshot.load_error.is_some() {
        return Err("The terminal's server connection is no longer available.".into());
    }
    let target = forwarding::discovered_tunnel(
        &snapshot.config.tunnels,
        server.id,
        &destination.host,
        destination.port,
    )?;
    let existing = snapshot.config.tunnels.iter().any(|t| t.id == target.id);
    if existing
        && snapshot
            .runtime
            .tunnels
            .get(&target.id)
            .is_some_and(|s| s.status == Status::Connected)
    {
        let endpoint = state.forward_open_endpoint(
            target.id,
            Some(destination.port),
            Some(&target),
            Some(&scope),
        )?;
        return destination.forwarded_url(endpoint.port());
    }
    let saved = if existing {
        super::forward_tunnel_batch(state, server.id, vec![], vec![target], Some(scope.clone()))
            .await?
    } else {
        let owner = owner.clone();
        let expected = scope.clone();
        let host = destination.host.clone();
        let port = destination.port;
        let proposed_id = target.id;
        // Keep allocation stable across the profile planner's validation replay.
        // Binding and other I/O must stay outside its short configuration lock.
        let local_port = available_port(target.local_port, &snapshot.config.tunnels)?;
        let target = state
            .edit_configuration(None, move |manager| {
                super::control::validate_forward_owner(manager, &owner)?;
                super::control::validate_forward_connection(
                    manager,
                    owner.server().id,
                    Some(&expected),
                )?;
                let mut config = manager.config.clone();
                let mut target =
                    forwarding::discovered_tunnel(&config.tunnels, owner.server().id, &host, port)?;
                if !config.tunnels.iter().any(|t| t.id == target.id) {
                    target.id = proposed_id;
                    target.local_port = local_port;
                    target.name = format!("Terminal · {host}:{port}");
                    target.validate_unique(&config.tunnels)?;
                    config.tunnels.push(target.clone());
                }
                Ok((config, target))
            })
            .await?;
        state.connect_tunnel_batch(server.id, &[target], Some(&scope))?
    };
    let [target] = saved.as_slice() else {
        return Err("Forwarding did not confirm exactly one destination.".into());
    };
    loop {
        if owner.is_revoked() {
            return Err("The terminal's server connection changed while forwarding.".into());
        }
        let snapshot = state.snapshot();
        if snapshot.config.tunnels.iter().find(|t| t.id == target.id) != Some(target) {
            return Err("The forward changed before the link could open.".into());
        }
        match snapshot.runtime.tunnels.get(&target.id) {
            Some(status) if status.status == Status::Connected => {
                let endpoint = state.forward_open_endpoint(
                    target.id,
                    Some(destination.port),
                    Some(target),
                    Some(&scope),
                )?;
                return destination.forwarded_url(endpoint.port());
            }
            Some(status) if matches!(status.status, Status::Connecting | Status::Reconnecting) => {}
            Some(status) => {
                return Err(status.error_message.clone().unwrap_or_else(|| {
                    "The forward disconnected before the link could open.".into()
                }));
            }
            None => return Err("The forward is no longer available.".into()),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        manager::{Manager, share},
        model::{ConnectionState, Server},
        profile::Store,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct Browser(Mutex<Vec<String>>);
    impl crate::desktop_integration::DesktopIntegration for Browser {
        fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
            action();
            Ok(())
        }
        fn clipboard_revision(&self) -> Option<isize> {
            None
        }
        fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
            None
        }
        fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
            None
        }
        fn open_url(&self, url: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(url.into());
            Ok(())
        }
    }

    fn server() -> Server {
        serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"name":"Terminal link fixture","sshHost":"127.0.0.1","sshPort":9,"sshUser":"fixture"})).unwrap()
    }

    #[tokio::test]
    async fn connected_range_is_reused_and_revoked_server_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let server = server();
        let mut tunnel = forwarding::discovered_tunnel(&[], server.id, "127.0.0.1", 5170).unwrap();
        tunnel.remote_port_end = Some(5180);
        tunnel.local_port = 9000;
        tunnel.local_port_end = Some(9010);
        manager.config.servers.push(server.clone());
        manager.config.tunnels.push(tunnel.clone());
        manager
            .runtime
            .lock()
            .unwrap()
            .tunnels
            .insert(tunnel.id, ConnectionState::new(Status::Connected));
        let state = share(manager);
        let owner = crate::server_session::resolve(&state, server.id)
            .await
            .unwrap();
        let target = destination("http://localhost:5178/demo?q=a#section")
            .unwrap()
            .unwrap();
        assert_eq!(
            prepare(&state, &owner, &target).await.unwrap(),
            "http://127.0.0.1:9008/demo?q=a#section"
        );
        assert_eq!(state.snapshot().config.tunnels, vec![tunnel]);
        state.lock().await.advance_connection_revision(server.id);
        assert!(prepare(&state, &owner, &target).await.is_err());
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    async fn external_links_do_not_forward_and_unknown_terminal_cannot_forward() {
        let directory = tempfile::tempdir().unwrap();
        let state = share(Manager::new(Store::for_test(directory.path().into())));
        let browser = Arc::new(Browser::default());
        let sessions = Sessions::default();
        let id = Uuid::new_v4();
        open_terminal_link(
            &state,
            &sessions,
            browser.clone(),
            id,
            "https://example.org/path?q=1#section".into(),
        )
        .await
        .unwrap();
        assert!(
            open_terminal_link(
                &state,
                &sessions,
                browser.clone(),
                id,
                "http://localhost:5178/path".into()
            )
            .await
            .is_err()
        );
        assert!(
            open_terminal_link(
                &state,
                &sessions,
                browser.clone(),
                id,
                "file:///etc/passwd".into()
            )
            .await
            .is_err()
        );
        assert_eq!(
            *browser.0.lock().unwrap(),
            vec!["https://example.org/path?q=1#section"]
        );
        assert!(state.snapshot().config.tunnels.is_empty());
        crate::manager::shutdown(&state).await;
    }

    #[tokio::test]
    #[ignore = "Requires npm run test:remote and real OpenSSH"]
    async fn remote_terminal_link_forwards_once_reuses_and_reaches_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let directory = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(Store::for_test(directory.path().into()));
        let mut server = server();
        server.ssh_port = std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap();
        server.identity_file = Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap());
        manager.config.servers.push(server.clone());
        let state = share(manager);
        let owner = crate::server_session::resolve(&state, server.id)
            .await
            .unwrap();
        let sessions = Sessions::default();
        let terminal = Uuid::new_v4();
        sessions
            .open_bound(terminal, server, 80, 24, owner)
            .unwrap();
        let browser = Arc::new(Browser::default());
        // The local SSH fixture publishes an allocated HTTP destination; the
        // Linux/OpenSSH fixture serves HTTP on 8080.
        let http_port = std::env::var("NEWPORT_TEST_HTTP_PORT")
            .map(|port| port.parse::<u16>().unwrap())
            .unwrap_or(8080);
        // Force a local-port conflict: the port is ours or already occupied.
        let _occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, http_port)).ok();
        let url = format!("http://localhost:{http_port}/?q=preserved#section");
        let (first, second) = tokio::join!(
            open_terminal_link(&state, &sessions, browser.clone(), terminal, url.clone()),
            open_terminal_link(&state, &sessions, browser.clone(), terminal, url.clone()),
        );
        first.unwrap();
        second.unwrap();
        let config = state.snapshot().config;
        assert_eq!(
            config.tunnels.len(),
            1,
            "concurrent clicks share one forward"
        );
        let port = config.tunnels[0].local_port;
        assert_ne!(port, http_port);
        assert!(!config.tunnels[0].auto_connect);
        assert_eq!(
            *browser.0.lock().unwrap(),
            vec![format!("http://127.0.0.1:{port}/?q=preserved#section"); 2]
        );
        let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        stream
            .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            Duration::from_secs(10),
            stream.read_to_string(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.contains("200 OK"), "{response}");
        if std::env::var_os("NEWPORT_TEST_HTTP_PORT").is_some() {
            assert!(response.ends_with("Newport HTTP fixture\r\n"), "{response}");
        }
        open_terminal_link(&state, &sessions, browser.clone(), terminal, url.clone())
            .await
            .unwrap();
        assert_eq!(state.snapshot().config.tunnels, config.tunnels);
        sessions.close_server(None).await;
        assert!(
            open_terminal_link(&state, &sessions, browser, terminal, url)
                .await
                .is_err()
        );
        crate::manager::shutdown(&state).await;
    }

    #[test]
    fn server_urls_preserve_scheme_path_query_and_fragment() {
        for (input, host, port) in [
            (
                "http://localhost:5178/demo?q=a%20b#panel",
                "127.0.0.1",
                5178,
            ),
            ("https://LOCALHOST./demo?q=a%20b#panel", "127.0.0.1", 443),
            (
                "http://127.0.0.2:8080/demo?q=a%20b#panel",
                "127.0.0.2",
                8080,
            ),
            ("http://[::1]:5178/demo?q=a%20b#panel", "::1", 5178),
            ("http://0.0.0.0/demo?q=a%20b#panel", "127.0.0.1", 80),
            ("http://[::]:5178/demo?q=a%20b#panel", "::1", 5178),
        ] {
            let target = destination(input).unwrap().unwrap();
            assert_eq!(target.host, host);
            assert_eq!(target.port, port);
            assert_eq!(
                target.forwarded_url(9000).unwrap(),
                format!(
                    "{}://127.0.0.1:9000/demo?q=a%20b#panel",
                    target.url.scheme()
                )
            );
        }
        for input in [
            "https://example.org/",
            "http://localhost.evil:5178/",
            "http://192.168.1.1:80/",
        ] {
            assert!(destination(input).unwrap().is_none());
        }
        assert!(destination("http://localhost:0/").is_err());
        assert!(destination("http://user:secret@localhost/").is_err());
    }

    #[test]
    fn occupied_local_port_gets_a_different_loopback_listener() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        assert_ne!(available_port(port, &[]).unwrap(), port);
    }
}
