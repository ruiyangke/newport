use super::*;
#[test]
fn all_messages_round_trip_without_an_integration_request_id() {
    for request in [
        Request::Offer,
        Request::Read {
            revision: i64::MIN,
            format: "text/plain".into(),
        },
    ] {
        let frame = encode_request(&request).unwrap();
        assert_eq!(read_request(&mut frame.as_slice()).unwrap(), request);
        for end in 0..frame.len() {
            assert!(read_request(&mut &frame[..end]).is_err());
        }
    }
    for response in [
        Response::Hello { version: VERSION },
        Response::Offer(imports::Offer {
            revision: i64::MIN,
            formats: vec!["text/plain".into()],
        }),
        Response::Chunk(Chunk {
            bytes: vec![],
            eof: true,
        }),
        Response::Chunk(Chunk {
            bytes: vec![255; imports::CHUNK],
            eof: false,
        }),
        Response::Failed(common::Error {
            code: "unavailable".into(),
            message: "Clipboard changed".into(),
            retry: common::Retry::No,
        }),
    ] {
        let frame = encode_response(&response).unwrap();
        assert_eq!(read_response(&mut frame.as_slice()).unwrap(), response);
        for end in [0, 1, frame.len() - 1] {
            assert!(read_response(&mut &frame[..end]).is_err());
        }
    }
    assert_eq!(encode_request(&Request::Offer).unwrap(), [2, 10, 0]);
}
#[test]
fn invalid_operations_bounds_and_duplicate_fields_fail() {
    for body in [&[10, 0, 18, 0][..], &[26, 0], &[18, 4, 8, 0, 8, 1]] {
        assert!(decode_request(Bytes::copy_from_slice(body)).is_err());
    }
    assert!(decode_response(Bytes::from_static(&[10, 6, 8, 128, 128, 128, 128, 16])).is_err());
    assert!(decode_response(Bytes::from_static(&[26, 4, 16, 1, 16, 1])).is_err());
    assert_eq!(
        decode_request(Bytes::from_static(&[10, 2, 8, 7])).unwrap(),
        Request::Offer
    );
    assert!(
        encode_request(&Request::Read {
            revision: 0,
            format: "../text".into()
        })
        .is_err()
    );
    assert!(
        encode_response(&Response::Chunk(Chunk {
            bytes: vec![],
            eof: false
        }))
        .is_err()
    );
    assert!(
        encode_response(&Response::Chunk(Chunk {
            bytes: vec![0; imports::CHUNK + 1],
            eof: true
        }))
        .is_err()
    );
    assert!(
        encode_response(&Response::Offer(imports::Offer {
            revision: 0,
            formats: vec!["text/plain".into(); imports::MAX_FORMATS + 1]
        }))
        .is_err()
    );
}
