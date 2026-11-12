use super::*;
use newport_protocol::{
    common::Present,
    desktop::{self, ClientMessage, Hello, Operation, Request, RequestId, host},
};
use std::io::Read;
use std::{sync::mpsc, thread, time::Duration};
const LIMIT: Duration = Duration::from_secs(2);
fn greeting() -> Vec<u8> {
    desktop::wire::encode_client(&ClientMessage::Hello(Box::new(Hello {
        version: desktop::VERSION,
        contract_id: Some(desktop::CONTRACT_ID.into()),
    })))
    .unwrap()
}
fn activate() -> Vec<u8> {
    desktop::wire::encode_client(&ClientMessage::Request(Box::new(Request {
        id: RequestId::new(1).unwrap(),
        operation: Operation::Host(Box::new(host::Operation::Activate(Box::new(Present {})))),
    })))
    .unwrap()
}
#[test]
fn partial_frame_timeout_preserves_prefix_and_bytes() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let mut reader = Reader::new(stream);
    let frame = activate();
    peer.write_all(&frame[..2]).unwrap();
    let error = reader
        .next_until(4096, Instant::now() + Duration::from_millis(20))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    peer.write_all(&frame[2..]).unwrap();
    let received = reader
        .next_until(4096, Instant::now() + LIMIT)
        .unwrap()
        .unwrap();
    assert_eq!(received.into_bytes().as_ref(), frame);
}
#[test]
fn codec_transition_keeps_coalesced_read_ahead() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let mut bytes = greeting();
    let next = activate();
    bytes.extend_from_slice(&next);
    peer.write_all(&bytes).unwrap();
    let mut reader = Reader::new(stream);
    assert!(
        desktop::wire::decode_hello(
            reader
                .next_until(4096, Instant::now() + LIMIT)
                .unwrap()
                .unwrap()
                .body()
        )
        .is_ok()
    );
    let (stream, buffer) = reader.into_parts();
    let mut reader = Reader::from_parts(stream, buffer);
    let frame = reader
        .next_until(desktop::MAX_REQUEST, Instant::now() + LIMIT)
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_bytes().as_ref(), next);
}
#[test]
fn a_new_budget_cannot_reinterpret_a_partial_frame() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let mut reader = Reader::new(stream);
    peer.write_all(&activate()[..1]).unwrap();
    assert!(
        reader
            .next_until(4096, Instant::now() + Duration::from_millis(20))
            .is_err()
    );
    assert_eq!(
        reader
            .next_until(8192, Instant::now() + LIMIT)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}
#[test]
fn truncated_frame_is_not_successful_closure() {
    let (stream, mut peer) = Stream::pair().unwrap();
    peer.write_all(&activate()[..2]).unwrap();
    drop(peer);
    let mut reader = Reader::new(stream);
    assert_eq!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap_err()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
}
#[test]
fn a_final_frame_is_delivered_before_transport_eof() {
    let (stream, mut peer) = Stream::pair().unwrap();
    peer.write_all(&greeting()).unwrap();
    drop(peer);
    let mut reader = Reader::new(stream);
    assert!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap()
            .is_some()
    );
    assert!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap()
            .is_none()
    );
}
#[test]
fn expired_deadline_does_not_consume_buffered_next_frame() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let frame = greeting();
    peer.write_all(&[frame.clone(), frame].concat()).unwrap();
    let mut reader = Reader::new(stream);
    assert!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        reader.next_until(4096, Instant::now()).unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    assert!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap()
            .is_some()
    );
}
#[test]
fn cloned_shutdown_cancels_a_pending_framed_read() {
    let (stream, _peer) = Stream::pair().unwrap();
    let mut reader = Reader::new(stream);
    let cancel = reader.try_clone_stream().unwrap();
    let (sent, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        sent.send(reader.next_until(4096, Instant::now() + LIMIT))
            .unwrap();
    });
    cancel.shutdown(Shutdown::Both).unwrap();
    let result = received.recv_timeout(LIMIT).unwrap();
    assert!(matches!(result, Ok(None) | Err(_)));
    worker.join().unwrap();
}
#[test]
fn a_write_timeout_retires_all_clones_of_the_connection() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let mut writer = Writer::new(stream);
    let mut clone = writer.try_clone_stream().unwrap();
    let (sent, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        let payload = vec![1; 8 * 1024 * 1024];
        let deadline = Instant::now() + Duration::from_millis(20);
        // Windows pipes can queue one write before exposing backpressure.
        let result = writer
            .send_until(&payload, deadline)
            .and_then(|()| writer.send_until(&greeting(), deadline));
        sent.send((writer, result)).unwrap();
    });
    let completed = received.recv_timeout(LIMIT);
    if completed.is_err() {
        let _ = clone.shutdown(Shutdown::Both);
        if received.recv_timeout(LIMIT).is_ok() {
            let _ = worker.join();
        }
        panic!("write must respect its deadline under backpressure");
    }
    worker.join().unwrap();
    let (mut writer, result) =
        completed.expect("write must respect its deadline under backpressure");
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert!(writer.is_retired());
    assert!(clone.write_all(&greeting()).is_err());
    assert_eq!(
        writer
            .send_until(&greeting(), Instant::now() + LIMIT)
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
    peer.set_read_timeout(Some(LIMIT)).unwrap();
    let mut discard = [0; READ_SIZE];
    while peer.read(&mut discard).unwrap() != 0 {}
}
#[test]
fn expired_connection_deadline_opens_no_socket() {
    let path = std::env::temp_dir().join(format!("newport-expired-{}.sock", uuid::Uuid::new_v4()));
    assert_eq!(
        super::super::connect_private_until(path, Instant::now())
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}
#[test]
fn idle_boundaries_wait_but_owned_shutdown_unblocks_them() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let mut reader = Reader::new(stream);
    let cancel = reader.try_clone_stream().unwrap();
    let (sent, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = sent.send(reader.next_idle(4096, Duration::from_millis(10)));
    });
    assert!(matches!(
        received.recv_timeout(Duration::from_millis(30)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    peer.write_all(&greeting()).unwrap();
    let result = received.recv_timeout(LIMIT);
    let _ = cancel.shutdown(Shutdown::Both);
    assert!(result.unwrap().unwrap().is_some());
    worker.join().unwrap();
}
#[test]
fn partial_idle_deadline_is_not_reset_when_a_wait_is_resumed() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let mut reader = Reader::new(stream);
    let frame = activate();
    peer.write_all(&frame[..1]).unwrap();
    assert_eq!(
        reader
            .next_idle(4096, Duration::from_millis(20))
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    let original = reader.buffer.frame_started;
    peer.write_all(&frame[1..]).unwrap();
    assert_eq!(
        reader
            .next_idle(4096, Duration::from_millis(20))
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(reader.buffer.frame_started, original);
}
#[test]
fn complete_read_ahead_survives_delayed_consumer_polling() {
    let (stream, _peer) = Stream::pair().unwrap();
    let frame = greeting();
    let buffer = ReadBuffer {
        bytes: frame,
        arrived: Some(Instant::now() - LIMIT),
        ..ReadBuffer::default()
    };
    let mut reader = Reader::from_parts(stream, buffer);
    assert!(
        reader
            .next_idle(4096, Duration::from_millis(20))
            .unwrap()
            .is_some()
    );
}

#[test]
fn expired_partial_idle_frame_cannot_gain_time_from_until() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let mut reader = Reader::new(stream);
    let frame = activate();
    peer.write_all(&frame[..1]).unwrap();
    assert_eq!(
        reader
            .next_idle(4096, Duration::from_millis(20))
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    peer.write_all(&frame[1..]).unwrap();
    assert_eq!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
}
#[test]
fn continuation_received_after_its_partial_deadline_is_never_delivered() {
    let (stream, _peer) = Stream::pair().unwrap();
    let frame = activate();
    let mut reader = Reader::new(stream);
    let mut decoder = FrameDecoder::new(4096);
    decoder.consume(&mut &frame[..1]).unwrap();
    reader.buffer = ReadBuffer {
        decoder: Some(decoder),
        limit: 4096,
        bytes: frame[1..].to_vec(),
        arrived: Some(Instant::now()),
        frame_started: Some(Instant::now() - LIMIT),
        frame_deadline: Some(Instant::now() - Duration::from_secs(1)),
        ..ReadBuffer::default()
    };
    assert_eq!(
        reader.next_idle(4096, LIMIT).unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}
#[test]
fn peer_closed_before_first_read_keeps_final_data_and_eof() {
    let (stream, mut peer) = Stream::pair().unwrap();
    let frame = greeting();
    peer.write_all(&frame).unwrap();
    drop(peer);
    let mut reader = Reader::new(stream);
    assert_eq!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap()
            .unwrap()
            .into_bytes()
            .as_ref(),
        frame
    );
    assert!(
        reader
            .next_until(4096, Instant::now() + LIMIT)
            .unwrap()
            .is_none()
    );
}
