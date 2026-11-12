use std::{
    future::Future,
    io::{self, Cursor, Read},
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

use bytes::Bytes;
use newport_protocol::encoding::protobuf::{
    FrameError, Layout, Limits, MAX_DEPTH, Present, Retry, Secret, decode, read_frame,
    read_message, tokio::FrameReader, write_frame, write_message,
};
use prost::Message;
use tokio::io::{AsyncRead, ReadBuf};

mod schema {
    include!(concat!(env!("OUT_DIR"), "/test/protos.rs"));
}

mod ext {
    include!(concat!(env!("OUT_DIR"), "/ext/protos.rs"));
}

use ext::wiretest::ext::Wrapped;
use schema::wiretest::{v1, v2};

const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/test/descriptors.bin"));
const EXT_DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ext/descriptors.bin"));

const LIMITS: Limits = Limits {
    message: 1 << 20,
    metadata: 1 << 10,
};

fn envelope(kind: v1::envelope::Kind) -> v1::Envelope {
    v1::Envelope { kind: Some(kind) }
}

fn chunk() -> v1::Chunk {
    v1::Chunk {
        id: 7,
        data: Bytes::from_static(b"hello"),
        name: "notes.txt".into(),
        parts: vec![Bytes::from_static(b"ab"), Bytes::from_static(b"cde")],
        attachments: [("icon".to_owned(), Bytes::from_static(b"png"))].into(),
        children: [(
            "first".to_owned(),
            v1::Nested {
                child: None,
                data: Bytes::from_static(b"x"),
            },
        )]
        .into(),
    }
}

fn login() -> v1::Login {
    v1::Login {
        user: "rui".into(),
        password: Some(Secret {
            value: Bytes::from_static(b"hunter2"),
        }),
    }
}

fn nested(depth: u32) -> v1::Nested {
    (0..depth).fold(
        v1::Nested {
            child: None,
            data: Bytes::from_static(b"leaf"),
        },
        |child, _| v1::Nested {
            child: Some(Box::new(child)),
            data: Bytes::new(),
        },
    )
}

fn every_message() -> Vec<v1::Envelope> {
    use v1::envelope::Kind;
    vec![
        envelope(Kind::Chunk(chunk())),
        envelope(Kind::Error(newport_protocol::encoding::protobuf::Error {
            code: "busy".into(),
            message: "Another request is running.".into(),
            retry: Retry::Yes.into(),
        })),
        envelope(Kind::Login(login())),
        envelope(Kind::Nested(nested(3))),
        envelope(Kind::Present(Present {})),
        v1::Envelope::default(),
    ]
}

fn stream(messages: &[impl Message + Layout]) -> Vec<u8> {
    let mut stream = Vec::new();
    for message in messages {
        write_message(&mut stream, message, &LIMITS).unwrap();
    }
    stream
}

#[test]
fn every_message_round_trips_and_the_stream_ends_cleanly() {
    let messages = every_message();
    let mut reader = Cursor::new(stream(&messages));
    for message in &messages {
        let read: v1::Envelope = read_message(&mut reader, &LIMITS).unwrap().unwrap();
        assert_eq!(&read, message);
    }
    assert!(
        read_message::<v1::Envelope, _>(&mut reader, &LIMITS)
            .unwrap()
            .is_none()
    );
}

#[test]
fn raw_frames_round_trip() {
    let mut stream = Vec::new();
    write_frame(&mut stream, b"").unwrap();
    write_frame(&mut stream, &[7; 300]).unwrap();
    let mut reader = Cursor::new(stream);
    assert_eq!(read_frame(&mut reader, 300).unwrap().unwrap(), Bytes::new());
    assert_eq!(
        read_frame(&mut reader, 300).unwrap().unwrap(),
        Bytes::from(vec![7; 300])
    );
    assert!(read_frame(&mut reader, 300).unwrap().is_none());
}

#[tokio::test]
async fn every_message_round_trips_async() {
    let messages = every_message();
    let mut stream = Vec::new();
    for message in &messages {
        newport_protocol::encoding::protobuf::tokio::write_message(&mut stream, message, &LIMITS)
            .await
            .unwrap();
    }
    newport_protocol::encoding::protobuf::tokio::write_frame(&mut stream, b"raw")
        .await
        .unwrap();
    let mut reader = FrameReader::new(&stream[..], LIMITS);
    for message in &messages {
        let read: v1::Envelope = reader.next_message().await.unwrap().unwrap();
        assert_eq!(&read, message);
    }
    let raw = reader.next().await.unwrap();
    assert_eq!(raw.unwrap(), Bytes::from_static(b"raw"));
    assert!(reader.next().await.unwrap().is_none());
    assert!(reader.next().await.unwrap().is_none());
}

/// Serves `data` three bytes a read, and is pending before every read.
struct Trickle {
    data: Vec<u8>,
    at: usize,
    ready: bool,
}

impl AsyncRead for Trickle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.ready {
            self.ready = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.ready = false;
        let end = self.data.len().min(self.at + 3);
        let read = end.min(self.at + buf.remaining());
        buf.put_slice(&self.data[self.at..read]);
        self.at = read;
        Poll::Ready(Ok(()))
    }
}

#[test]
fn a_read_cancelled_inside_a_frame_goes_on_where_it_stopped() {
    let messages = every_message();
    let trickle = Trickle {
        data: stream(&messages),
        at: 0,
        ready: false,
    };
    let mut reader = FrameReader::new(trickle, LIMITS);
    let mut cx = Context::from_waker(Waker::noop());
    let (mut read, mut cancelled) = (Vec::new(), 0);
    loop {
        // Every read is dropped at its first wait.
        let poll = pin!(reader.next_message::<v1::Envelope>()).poll(&mut cx);
        match poll {
            Poll::Ready(Ok(Some(message))) => read.push(message),
            Poll::Ready(Ok(None)) => break,
            Poll::Ready(Err(error)) => panic!("{error:?}"),
            Poll::Pending => cancelled += 1,
        }
    }
    assert_eq!(read, messages);
    assert!(
        cancelled > 2 * messages.len(),
        "cancelled {cancelled} reads"
    );
}

/// Serves a frame's length and fails the test if anything reads further.
struct LengthOnly(Cursor<Vec<u8>>);

impl Read for LengthOnly {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.0.read(buf)?;
        assert!(read > 0 || buf.is_empty(), "read past the frame's length");
        Ok(read)
    }
}

#[test]
fn a_frame_over_the_message_limit_is_refused_before_its_body_is_read() {
    let mut header = Vec::new();
    prost::encode_length_delimiter(LIMITS.message + 1, &mut header).unwrap();
    let error =
        read_frame(&mut LengthOnly(Cursor::new(header.clone())), LIMITS.message).unwrap_err();
    assert!(matches!(
        error,
        FrameError::TooLarge { length, limit } if length == LIMITS.message as u64 + 1 && limit == LIMITS.message
    ));
    let error =
        read_message::<v1::Envelope, _>(&mut LengthOnly(Cursor::new(header)), &LIMITS).unwrap_err();
    assert!(matches!(error, FrameError::TooLarge { .. }));
}

#[tokio::test]
async fn a_frame_over_the_message_limit_is_refused_before_its_body_is_read_async() {
    let mut header = Vec::new();
    prost::encode_length_delimiter(LIMITS.message + 1, &mut header).unwrap();
    // Reading on would end the stream and report it truncated instead.
    let error = FrameReader::new(&header[..], LIMITS)
        .next()
        .await
        .unwrap_err();
    assert!(matches!(error, FrameError::TooLarge { .. }));
}

#[tokio::test]
async fn a_length_past_a_u64_is_refused() {
    let lengths = [
        [0xff; 11].to_vec(),
        [[0x80; 9].as_slice(), &[0x02]].concat(),
        [[0x80; 9].as_slice(), &[0x03]].concat(),
    ];
    let limits = Limits {
        message: usize::MAX,
        ..LIMITS
    };
    for length in lengths {
        let error = read_frame(&mut &length[..], usize::MAX).unwrap_err();
        assert!(
            matches!(
                error,
                FrameError::TooLarge {
                    length: u64::MAX,
                    ..
                }
            ),
            "{length:02x?}: {error:?}"
        );
        let error = FrameReader::new(&length[..], limits)
            .next()
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                FrameError::TooLarge {
                    length: u64::MAX,
                    ..
                }
            ),
            "{length:02x?} async: {error:?}"
        );
    }
}

#[tokio::test]
async fn a_message_over_the_writers_limits_is_not_written() {
    let message = envelope(v1::envelope::Kind::Login(login()));
    let len = message.encoded_len();
    let small = Limits {
        message: len - 1,
        ..LIMITS
    };
    let thin = Limits {
        metadata: len - "hunter2".len() - 1,
        ..LIMITS
    };
    let mut written = Vec::new();
    assert!(matches!(
        write_message(&mut written, &message, &small),
        Err(FrameError::TooLarge { length, limit }) if length == len as u64 && limit == len - 1
    ));
    assert!(matches!(
        write_message(&mut written, &message, &thin),
        Err(FrameError::MetadataTooLarge { .. })
    ));
    assert!(matches!(
        newport_protocol::encoding::protobuf::tokio::write_message(&mut written, &message, &small)
            .await,
        Err(FrameError::TooLarge { .. })
    ));
    assert!(matches!(
        newport_protocol::encoding::protobuf::tokio::write_message(&mut written, &message, &thin)
            .await,
        Err(FrameError::MetadataTooLarge { .. })
    ));
    assert!(written.is_empty());

    let exact = Limits {
        message: len,
        metadata: len - "hunter2".len(),
    };
    write_message(&mut written, &message, &exact).unwrap();
    assert_eq!(
        read_message(&mut &written[..], &exact).unwrap(),
        Some(message)
    );
}

#[tokio::test]
async fn a_stream_ending_inside_a_frame_is_truncated_async() {
    let frame = stream(&[envelope(v1::envelope::Kind::Login(login()))]);
    for end in [1, frame.len() - 1] {
        let error = FrameReader::new(&frame[..end], LIMITS)
            .next()
            .await
            .unwrap_err();
        assert!(
            matches!(error, FrameError::Truncated),
            "ending at {end}: {error:?}"
        );
    }
}

#[test]
fn a_stream_ending_inside_a_frame_is_truncated() {
    let frame = stream(&[envelope(v1::envelope::Kind::Login(login()))]);
    for end in [1, frame.len() - 1] {
        let error = read_frame(&mut &frame[..end], LIMITS.message).unwrap_err();
        assert!(
            matches!(error, FrameError::Truncated),
            "ending at {end}: {error:?}"
        );
    }
    let error = read_frame(&mut &[0x80][..], LIMITS.message).unwrap_err();
    assert!(matches!(error, FrameError::Truncated));
}

#[test]
fn bytes_fields_do_not_count_as_metadata() {
    let big = Bytes::from(vec![1; 4 * LIMITS.metadata]);
    let mut chunk = v1::Chunk {
        data: big.clone(),
        parts: vec![big.clone(), big.clone()],
        attachments: [("icon".to_owned(), big.clone())].into(),
        children: [(
            "first".to_owned(),
            v1::Nested {
                child: Some(Box::new(v1::Nested {
                    child: None,
                    data: big.clone(),
                })),
                data: big.clone(),
            },
        )]
        .into(),
        ..chunk()
    };
    let message = envelope(v1::envelope::Kind::Chunk(chunk.clone()));
    let frame = Bytes::from(message.encode_to_vec());
    assert_eq!(decode::<v1::Envelope>(frame, &LIMITS).unwrap(), message);

    chunk.name = "n".repeat(LIMITS.metadata);
    let frame = Bytes::from(envelope(v1::envelope::Kind::Chunk(chunk)).encode_to_vec());
    let error = decode::<v1::Envelope>(frame, &LIMITS).unwrap_err();
    assert!(matches!(
        error,
        FrameError::MetadataTooLarge { length, limit } if length > LIMITS.metadata && limit == LIMITS.metadata
    ));
}

#[test]
fn a_frame_over_the_metadata_limit_is_refused_before_decoding() {
    let message = v1::Login {
        user: "u".repeat(LIMITS.metadata),
        ..login()
    };
    let mut stream = Vec::new();
    // The body ends in a field the decoder rejects (wire type 7), so
    // decoding first would report that instead.
    let mut body = message.encode_to_vec();
    body.push(0x0f);
    write_frame(&mut stream, &body).unwrap();
    let error = read_message::<v1::Login, _>(&mut &stream[..], &LIMITS).unwrap_err();
    assert!(
        matches!(error, FrameError::MetadataTooLarge { .. }),
        "{error:?}"
    );

    body.truncate(body.len() - 1);
    let limits = Limits {
        metadata: body.len(),
        ..LIMITS
    };
    assert_eq!(
        decode::<v1::Login>(Bytes::from(body), &limits).unwrap(),
        message
    );
}

#[test]
fn a_frame_over_the_message_limit_is_refused_when_decoded_directly() {
    let frame = Bytes::from(envelope(v1::envelope::Kind::Chunk(chunk())).encode_to_vec());
    let limits = Limits {
        message: frame.len() - 1,
        ..LIMITS
    };
    assert!(matches!(
        decode::<v1::Envelope>(frame, &limits),
        Err(FrameError::TooLarge { .. })
    ));
}

#[test]
fn messages_nest_at_most_the_maximum_depth() {
    // The envelope holds the outermost Nested, one level down.
    let deepest = envelope(v1::envelope::Kind::Nested(nested(MAX_DEPTH - 1)));
    let frame = Bytes::from(deepest.encode_to_vec());
    assert_eq!(decode::<v1::Envelope>(frame, &LIMITS).unwrap(), deepest);

    let too_deep = envelope(v1::envelope::Kind::Nested(nested(MAX_DEPTH)));
    let frame = Bytes::from(too_deep.encode_to_vec());
    assert!(matches!(
        decode::<v1::Envelope>(frame, &LIMITS),
        Err(FrameError::TooDeep)
    ));
}

#[test]
fn a_malformed_frame_is_a_decode_error() {
    let frame = Bytes::from_static(&[0x0a, 0x05, 0x01]);
    assert!(matches!(
        decode::<v1::Envelope>(frame, &LIMITS),
        Err(FrameError::Decode(_))
    ));
}

#[test]
fn an_older_peer_skips_unknown_fields_and_oneof_cases() {
    let newer = [
        v2::Envelope {
            kind: Some(v2::envelope::Kind::Chunk(v2::Chunk {
                id: 7,
                data: Bytes::from_static(b"hello"),
                note: "only newer peers know this".into(),
                ..Default::default()
            })),
        },
        v2::Envelope {
            kind: Some(v2::envelope::Kind::Ping(v2::Ping { sent: 1 })),
        },
    ];
    let mut reader = Cursor::new(stream(&newer));
    let chunk: v1::Envelope = read_message(&mut reader, &LIMITS).unwrap().unwrap();
    assert_eq!(
        chunk,
        envelope(v1::envelope::Kind::Chunk(v1::Chunk {
            id: 7,
            data: Bytes::from_static(b"hello"),
            ..Default::default()
        }))
    );
    let ping: v1::Envelope = read_message(&mut reader, &LIMITS).unwrap().unwrap();
    assert_eq!(ping, v1::Envelope { kind: None });
}

#[test]
fn bytes_fields_borrow_from_the_frame() {
    let mut reader = Cursor::new(stream(&[envelope(v1::envelope::Kind::Chunk(chunk()))]));
    let frame = read_frame(&mut reader, LIMITS.message).unwrap().unwrap();
    let range = frame.as_ptr_range();
    let Some(v1::envelope::Kind::Chunk(chunk)) =
        decode::<v1::Envelope>(frame.clone(), &LIMITS).unwrap().kind
    else {
        panic!("expected a chunk");
    };
    let borrowed = [
        &chunk.data,
        &chunk.parts[0],
        &chunk.parts[1],
        &chunk.attachments["icon"],
    ];
    for bytes in borrowed.into_iter().chain([&chunk.children["first"].data]) {
        assert!(
            range.contains(&bytes.as_ptr()),
            "{bytes:?} was copied out of the frame"
        );
    }
}

#[test]
fn secrets_are_redacted_in_debug_output() {
    let debug = format!("{:?}", login());
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("password: Some([redacted])"), "{debug}");
}

#[test]
fn the_trace_prints_every_message_as_json_with_bytes_as_lengths_and_secrets_redacted() {
    let capture = stream(&every_message());
    let mut output = Vec::new();
    newport_protocol::encoding::protobuf::trace::trace(
        DESCRIPTORS,
        "wiretest.v1.Envelope",
        &mut &capture[..],
        &mut output,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(
        json_lines(&output),
        json(&[
            r#"{"chunk":{"attachments":{"icon":3},"children":{"first":{"data":1}},"data":5,"id":"7","name":"notes.txt","parts":[2,3]}}"#,
            r#"{"error":{"code":"busy","message":"Another request is running.","retry":"RETRY_YES"}}"#,
            r#"{"login":{"password":"[redacted]","user":"rui"}}"#,
            r#"{"nested":{"child":{"child":{"child":{"data":4}}}}}"#,
            r#"{"present":{}}"#,
            r#"{}"#,
        ])
    );
    assert!(!output.contains("aHVudGVyMg"), "the secret's base64 leaked");
}

#[test]
fn the_trace_reads_the_crates_own_schema() {
    let capture = stream(&[Secret {
        value: Bytes::from_static(b"hunter2"),
    }]);
    let mut output = Vec::new();
    newport_protocol::encoding::protobuf::trace::trace(
        newport_protocol::encoding::protobuf::DESCRIPTORS,
        "newport.wire.Secret",
        &mut &capture[..],
        &mut output,
    )
    .unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), "\"[redacted]\"\n");
}

#[test]
fn the_trace_names_a_message_missing_from_the_descriptor_set() {
    let error = newport_protocol::encoding::protobuf::trace::trace(
        DESCRIPTORS,
        "wiretest.v1.Missing",
        &mut io::empty(),
        &mut io::sink(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("wiretest.v1.Missing"), "{error}");
}

#[test]
fn a_schema_takes_another_crates_types_and_their_layouts() {
    let big = Bytes::from(vec![1; 4 * LIMITS.metadata]);
    let wrapped = Wrapped {
        chunk: Some(v1::Chunk {
            data: big,
            ..chunk()
        }),
        ..Default::default()
    };
    let frame = Bytes::from(wrapped.encode_to_vec());
    assert_eq!(decode::<Wrapped>(frame, &LIMITS).unwrap(), wrapped);
}

fn any(type_name: &str, message: &impl Message) -> prost_types::Any {
    prost_types::Any {
        type_url: format!("type.googleapis.com/{type_name}"),
        value: message.encode_to_vec(),
    }
}

#[test]
fn the_trace_redacts_inside_well_known_wrappers() {
    let secret = Secret {
        value: Bytes::from_static(b"hunter2"),
    };
    let capture = stream(&[
        Wrapped {
            blob: Some(b"hunter2".to_vec()),
            ..Default::default()
        },
        Wrapped {
            any: Some(any("newport.wire.Secret", &secret)),
            ..Default::default()
        },
        Wrapped {
            any: Some(any("wiretest.v1.Login", &login())),
            ..Default::default()
        },
        Wrapped {
            any: Some(any("wiretest.v1.Missing", &secret)),
            ..Default::default()
        },
    ]);
    let mut output = Vec::new();
    newport_protocol::encoding::protobuf::trace::trace(
        EXT_DESCRIPTORS,
        "wiretest.ext.Wrapped",
        &mut &capture[..],
        &mut output,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(
        json_lines(&output),
        json(&[
            r#"{"blob":7}"#,
            r#"{"any":{"@type":"type.googleapis.com/newport.wire.Secret","value":"[redacted]"}}"#,
            r#"{"any":{"@type":"type.googleapis.com/wiretest.v1.Login","password":"[redacted]","user":"rui"}}"#,
            r#"{"any":{"@type":"type.googleapis.com/wiretest.v1.Missing","redacted":true}}"#,
        ])
    );
    assert!(!output.contains("aHVudGVyMg"), "the secret's base64 leaked");
}

#[test]
fn the_trace_reports_a_frame_that_does_not_decode_and_goes_on() {
    let mut capture = Vec::new();
    write_message(&mut capture, &login(), &LIMITS).unwrap();
    write_frame(&mut capture, &[0x0a, 0x05, 0x01]).unwrap();
    write_message(&mut capture, &login(), &LIMITS).unwrap();
    let mut output = Vec::new();
    newport_protocol::encoding::protobuf::trace::trace(
        DESCRIPTORS,
        "wiretest.v1.Login",
        &mut &capture[..],
        &mut output,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    let lines = output.lines().collect::<Vec<_>>();
    let login = json(&[r#"{"password":"[redacted]","user":"rui"}"#]);
    assert_eq!(lines.len(), 3, "{output}");
    assert_eq!(json(&lines[..1]), login);
    assert!(
        lines[1].starts_with(r#"{"@error":"#) && lines[1].ends_with(r#","@frame":1,"@length":3}"#),
        "{}",
        lines[1]
    );
    assert_eq!(json(&lines[2..]), login);
}

/// Lines of JSON as values, so key order, which serde_json's features decide,
/// doesn't matter.
fn json(lines: &[&str]) -> Vec<serde_json::Value> {
    lines
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn json_lines(output: &str) -> Vec<serde_json::Value> {
    json(&output.lines().collect::<Vec<_>>())
}
