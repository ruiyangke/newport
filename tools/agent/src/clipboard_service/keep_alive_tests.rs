use super::*;
use crate::runtime_protocol::clipboard::FILES_FORMAT;

fn host_read() -> (State, Grant, clipboard_wire::Request) {
    host_read_of(FILES_FORMAT)
}
fn host_read_of(format: &'static str) -> (State, Grant, clipboard_wire::Request) {
    let mut state = State::new("runtime", PathBuf::from("/unused/clipboard.sock"));
    let access = state.grant("host", "integration").unwrap();
    let grant = state.authenticate(&access).unwrap();
    state.claim(&grant).unwrap();
    state.enabled(&grant, true).unwrap();
    state.publish(&grant, 7, vec![format.into()]).unwrap();
    let store = state.store.clone();
    let revision = state.revision;
    std::thread::spawn(move || store.get_shared(revision, format, Some(&|| false)));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some((id, revision, format)) = state.poll(&grant).unwrap().pop() {
            let request = clipboard_wire::Request {
                id: 1,
                revision,
                format,
            };
            assert_eq!(id, 1);
            return (state, grant, request);
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn frame(request: &clipboard_wire::Request, done: bool) -> Vec<u8> {
    clipboard_wire::reply(request, 0, done, b"")
}

#[test]
fn a_not_done_frame_renews_the_pending_read() {
    let (mut state, grant, request) = host_read();
    let base = Instant::now();
    state.requests.get_mut(&1).unwrap().expires = base + Duration::from_secs(1);
    state.reply(&grant, &frame(&request, false)).unwrap();
    let pending = &state.requests[&1];
    assert!(pending.expires >= base + Duration::from_secs(14));
    assert!(pending.expires <= pending.ceiling);
    state.reply(&grant, &frame(&request, true)).unwrap();
    assert!(state.requests.is_empty());
}

#[test]
fn renewal_stops_at_the_ceiling() {
    let (mut state, grant, request) = host_read();
    let ceiling = Instant::now() + Duration::from_secs(2);
    state.requests.get_mut(&1).unwrap().ceiling = ceiling;
    state.reply(&grant, &frame(&request, false)).unwrap();
    assert_eq!(state.requests[&1].expires, ceiling);
}

#[test]
fn an_expired_pending_read_stays_dropped() {
    let (mut state, grant, request) = host_read();
    state.requests.get_mut(&1).unwrap().expires = Instant::now() - Duration::from_secs(1);
    state.reply(&grant, &frame(&request, false)).unwrap();
    assert!(state.requests.is_empty());
}

#[test]
fn a_not_done_frame_does_not_renew_other_formats() {
    let (mut state, grant, request) = host_read_of("text/plain");
    let soon = Instant::now() + Duration::from_secs(1);
    state.requests.get_mut(&1).unwrap().expires = soon;
    state.reply(&grant, &frame(&request, false)).unwrap();
    assert_eq!(state.requests[&1].expires, soon);
}
