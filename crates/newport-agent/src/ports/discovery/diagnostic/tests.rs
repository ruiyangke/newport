use super::*;
use netlink_packet_core::{DefaultNla, DoneMessage, ErrorMessage};
use netlink_packet_sock_diag::inet::InetResponseHeader;
use newport_protocol::monitor::ports::discovery::{IssueKind, Outcome};
use std::{net::TcpListener, num::NonZeroI32, time::Instant};

fn response(family: Family, number: u32) -> InetResponse {
    let mut socket_id = match family {
        Family::Ipv4 => SocketId::new_v4(),
        Family::Ipv6 => SocketId::new_v6(),
    };
    socket_id.source_port = 8080;
    socket_id.cookie[..4].copy_from_slice(&number.to_ne_bytes());
    InetResponse {
        header: InetResponseHeader {
            family: family.number(),
            state: TCP_LISTEN,
            timer: None,
            socket_id,
            recv_queue: 0,
            send_queue: 128,
            uid: 0,
            inode: number,
        },
        nlas: Default::default(),
    }
}

fn frame(payload: NetlinkPayload<SockDiagMessage>, flags: u16, sequence: u32) -> Vec<u8> {
    let mut header = NetlinkHeader::default();
    header.flags = flags;
    header.sequence_number = sequence;
    // A response header may contain the receiver's port ID. Sender validation uses recvfrom.
    header.port_number = 782;
    let mut packet = NetlinkMessage::new(header, payload);
    packet.finalize();
    let mut bytes = vec![0; packet.buffer_len()];
    packet.serialize(&mut bytes);
    bytes
}

fn record(number: u32) -> Vec<u8> {
    frame(
        SockDiagMessage::InetResponse(Box::new(response(Family::Ipv4, number))).into(),
        NLM_F_MULTIPART,
        SEQUENCE,
    )
}

fn done(code: i32, flags: u16) -> Vec<u8> {
    let mut done = DoneMessage::default();
    done.code = code;
    frame(NetlinkPayload::Done(done), flags, SEQUENCE)
}

#[test]
fn multipart_inventory_preserves_shared_endpoints_and_root_uid() {
    let mut dump = Dump::new(Family::Ipv4);
    dump.consume(&[record(1), record(2), done(0, NLM_F_MULTIPART)].concat())
        .unwrap();
    assert!(dump.done);
    assert_eq!(dump.sockets.len(), 2);
    assert_eq!(dump.sockets[0].endpoint, dump.sockets[1].endpoint);
    assert_ne!(dump.sockets[0].cookie, dump.sockets[1].cookie);
    assert_eq!(dump.sockets[0].uid, 0);
    assert!(dump.sockets[0].holders.is_empty());
}

#[test]
fn cgroup_and_ipv6_scope_survive_library_unknown_attributes() {
    let mut value = response(Family::Ipv6, 7);
    value.header.socket_id.interface_id = 5;
    value.nlas.push(Nla::Other(DefaultNla::new(
        INET_DIAG_CGROUP_ID,
        0x123456789u64.to_ne_bytes().to_vec(),
    )));
    value
        .nlas
        .push(Nla::Other(DefaultNla::new(150, vec![9; 4])));
    value.nlas.push(Nla::SkV6Only(true));
    let bytes = frame(
        SockDiagMessage::InetResponse(Box::new(value)).into(),
        NLM_F_MULTIPART,
        SEQUENCE,
    );
    let mut dump = Dump::new(Family::Ipv6);
    dump.consume(&bytes).unwrap();
    let socket = &dump.sockets[0];
    assert_eq!(socket.cgroup.as_ref().unwrap().id, 0x123456789);
    assert_eq!(socket.ipv6_only, Some(true));
    assert_eq!(socket.endpoint.interface, Some(5));
}

#[test]
fn cookie_uses_native_words_and_preserves_zero() {
    assert_eq!(cookie([0; 8]), Some(0));
    assert_eq!(cookie([0xff; 8]), None);
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&0x10203040u32.to_ne_bytes());
    bytes[4..].copy_from_slice(&0x50607080u32.to_ne_bytes());
    assert_eq!(cookie(bytes), Some(0x5060708010203040));
}

#[test]
fn interruption_and_completion_errors_cannot_be_empty_success() {
    let mut dump = Dump::new(Family::Ipv4);
    dump.consume(&record(1)).unwrap();
    let error = dump.consume(&done(0, NLM_F_DUMP_INTR)).unwrap_err();
    assert_eq!(error.kind(), IssueKind::Interrupted);
    assert_eq!(error.report(true).outcome, Outcome::Partial);
    assert!(!dump.done);
    assert_eq!(dump.sockets.len(), 1);
    for code in [-libc::ENOBUFS, -libc::EACCES, i32::MIN, 1] {
        assert!(
            Dump::new(Family::Ipv4)
                .consume(&done(code, NLM_F_MULTIPART))
                .is_err()
        );
    }
}

#[test]
fn malformed_error_codes_are_rejected_without_integer_overflow() {
    for code in [i32::MIN, 1] {
        let mut error = ErrorMessage::default();
        error.code = NonZeroI32::new(code);
        let bytes = frame(NetlinkPayload::Error(error), 0, SEQUENCE);
        assert!(Dump::new(Family::Ipv4).consume(&bytes).is_err());
    }
}

#[test]
fn malformed_multipart_sequences_and_duplicate_identities_are_rejected() {
    let mut duplicate = Dump::new(Family::Ipv4);
    duplicate.consume(&record(1)).unwrap();
    assert!(duplicate.consume(&record(1)).is_err());
    let mut after_done = Dump::new(Family::Ipv4);
    after_done.consume(&done(0, NLM_F_MULTIPART)).unwrap();
    assert!(after_done.consume(&record(2)).is_err());
    for (flags, sequence) in [(0, SEQUENCE), (NLM_F_MULTIPART, SEQUENCE + 1)] {
        let bytes = frame(
            SockDiagMessage::InetResponse(Box::new(response(Family::Ipv4, 1))).into(),
            flags,
            sequence,
        );
        assert!(Dump::new(Family::Ipv4).consume(&bytes).is_err());
    }
    let mut malformed = record(1);
    malformed[..4].copy_from_slice(&0u32.to_ne_bytes());
    assert!(Dump::new(Family::Ipv4).consume(&malformed).is_err());
    assert!(Dump::new(Family::Ipv4).consume(&[0; 15]).is_err());
}

#[test]
fn invalid_known_attributes_are_rejected() {
    for bytes in [vec![1; 7], vec![1; 9], vec![0; 8]] {
        let mut response = response(Family::Ipv4, 1);
        response
            .nlas
            .push(Nla::Other(DefaultNla::new(INET_DIAG_CGROUP_ID, bytes)));
        assert!(observation(response, Family::Ipv4).is_err());
    }
    let mut ipv4 = response(Family::Ipv4, 1);
    ipv4.nlas.push(Nla::SkV6Only(false));
    assert!(observation(ipv4, Family::Ipv4).is_err());
    assert!(observation(response(Family::Ipv6, 1), Family::Ipv4).is_err());
}

#[test]
fn inventory_limits_preserve_observations_with_incomplete_coverage() {
    let mut dump = Dump::new(Family::Ipv4);
    for number in 1..=MAX_LISTENERS {
        dump.consume(&record(u32::try_from(number).unwrap()))
            .unwrap();
    }
    let error = dump
        .consume(&record(u32::try_from(MAX_LISTENERS + 1).unwrap()))
        .unwrap_err();
    assert_eq!(dump.sockets.len(), MAX_LISTENERS);
    assert_eq!(error.kind(), IssueKind::BudgetExceeded);
    assert_eq!(error.report(true).outcome, Outcome::Partial);
    let mut oversized = Dump::new(Family::Ipv4);
    oversized.bytes = DUMP_LIMIT;
    assert_eq!(
        oversized.consume(&record(1)).unwrap_err().kind(),
        IssueKind::BudgetExceeded
    );
}

#[test]
fn expired_or_cancelled_queries_stop_before_opening_a_socket() {
    let expired = collect(Family::Ipv4, &Budget::new(Instant::now(), &|| false));
    assert_eq!(expired.report.issues[0].kind, IssueKind::TimedOut);
    let cancelled = collect(
        Family::Ipv4,
        &Budget::new(Instant::now() + FAMILY_TIME, &|| true),
    );
    assert_eq!(cancelled.report.issues[0].kind, IssueKind::Interrupted);
    assert!(cancelled.sockets.is_empty());
}

#[test]
fn kernel_inventory_finds_a_live_listener_without_a_shell_tool() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let inventory = collect(
        Family::Ipv4,
        &Budget::new(Instant::now() + FAMILY_TIME, &|| false),
    );
    assert_eq!(
        inventory.report.outcome,
        Outcome::Complete,
        "{:?}",
        inventory.report
    );
    let socket = inventory
        .sockets
        .iter()
        .find(|socket| {
            socket.endpoint.address == endpoint.ip() && socket.endpoint.port == endpoint.port()
        })
        .unwrap();
    assert!(socket.cookie.is_some());
    assert_ne!(socket.inode, 0);
}
