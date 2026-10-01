//! Version 4: positional message envelopes, integer-key payload maps, native bytes.
//! Envelope tags and positions are explicit and frozen here. Payloads serialize
//! directly from the application values, without building a second value tree.
use super::{
    codec_fields::{field_id, field_name},
    invalid, CommandLog, Error, Message, MAX_DIFF,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
    ser::{SerializeMap, SerializeSeq},
    Deserialize, Deserializer, Serialize, Serializer,
};
use serde_json::Value;
use std::{
    fmt,
    io::{self, Cursor},
};

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Binary,
    Lines,
    Line,
}
impl Mode {
    fn field(key: &str) -> Self {
        match key {
            "bytesB64" | "contentBytesB64" => Self::Binary,
            "lines" => Self::Lines,
            _ => Self::Normal,
        }
    }
    fn item(self, index: usize) -> Self {
        match (self, index) {
            (Self::Lines, _) => Self::Line,
            (Self::Line, 7) => Self::Binary,
            _ => Self::Normal,
        }
    }
}
struct BodyRef<'a>(&'a Value, Mode, usize);
fn body(value: &Value) -> BodyRef<'_> {
    BodyRef(value, Mode::Normal, 0)
}
impl Serialize for BodyRef<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.2 > 32 {
            return Err(serde::ser::Error::custom(
                "MessagePack nesting limit exceeded",
            ));
        }
        match self.0 {
            Value::Null => s.serialize_none(),
            Value::Bool(v) => s.serialize_bool(*v),
            Value::Number(v) => v.serialize(s),
            Value::String(v) if matches!(self.1, Mode::Binary) => {
                s.serialize_bytes(&STANDARD.decode(v).map_err(serde::ser::Error::custom)?)
            }
            Value::String(v) => s.serialize_str(v),
            Value::Array(rows) => {
                let mut seq = s.serialize_seq(Some(rows.len()))?;
                for (i, v) in rows.iter().enumerate() {
                    seq.serialize_element(&BodyRef(v, self.1.item(i), self.2 + 1))?;
                }
                seq.end()
            }
            Value::Object(rows) => {
                let mut map = s.serialize_map(Some(rows.len()))?;
                for (key, v) in rows {
                    let value = BodyRef(v, Mode::field(key), self.2 + 1);
                    if let Some(id) = field_id(key) {
                        map.serialize_entry(&id, &value)?;
                    } else {
                        map.serialize_entry(key, &value)?;
                    }
                }
                map.end()
            }
        }
    }
}
struct Key(String);
impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Keys;
        impl Visitor<'_> for Keys {
            type Value = Key;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a field ID or extension key")
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> Result<Key, E> {
                field_name(n)
                    .map(|s| Key(s.into()))
                    .ok_or_else(|| E::custom("Unknown field ID"))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Key, E> {
                Ok(Key(s.into()))
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<Key, E> {
                Ok(Key(s))
            }
        }
        d.deserialize_any(Keys)
    }
}
struct BodySeed(Mode, usize);
impl<'de> DeserializeSeed<'de> for BodySeed {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        if self.1 > 32 {
            return Err(de::Error::custom("MessagePack nesting limit exceeded"));
        }
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for BodySeed {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a bounded protocol value")
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("Non-finite number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Value, E> {
        if !matches!(self.0, Mode::Binary) {
            return Err(E::custom("Unexpected binary value"));
        }
        Ok(Value::String(STANDARD.encode(v)))
    }
    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Value, E> {
        self.visit_bytes(&v)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut rows = Vec::new();
        while let Some(v) = seq.next_element_seed(BodySeed(self.0.item(rows.len()), self.1 + 1))? {
            rows.push(v);
        }
        Ok(Value::Array(rows))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut rows = serde_json::Map::new();
        while let Some(Key(k)) = map.next_key()? {
            let v = map.next_value_seed(BodySeed(Mode::field(&k), self.1 + 1))?;
            if rows.insert(k, v).is_some() {
                return Err(de::Error::custom("Duplicate field"));
            }
        }
        Ok(Value::Object(rows))
    }
}
struct Body(Value);
impl<'de> Deserialize<'de> for Body {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        BodySeed(Mode::Normal, 0).deserialize(d).map(Self)
    }
}
fn serialize(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = rmp_serde::to_vec(value).map_err(invalid)?;
    if bytes.len() > MAX_DIFF {
        return Err(invalid("MessagePack payload too large"));
    }
    Ok(bytes)
}
fn deserialize<T: de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.len() > MAX_DIFF {
        return Err(invalid("MessagePack payload too large"));
    }
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(bytes));
    decoder.set_max_depth(40);
    let value = T::deserialize(&mut decoder).map_err(invalid)?;
    if decoder.position() != bytes.len() as u64 {
        return Err(invalid("Trailing MessagePack data"));
    }
    Ok(value)
}
pub(super) fn encode_value(value: &Value) -> io::Result<Vec<u8>> {
    serialize(&body(value))
}
pub(super) fn decode_value(bytes: &[u8]) -> io::Result<Value> {
    deserialize::<Body>(bytes).map(|v| v.0)
}

pub(super) fn encode(message: &Message) -> io::Result<Vec<u8>> {
    match message {
        Message::Hello {
            protocol,
            versions,
            instance_id,
            limits,
        } => serialize(&(0u8, protocol, versions, instance_id, body(limits))),
        Message::Initialize {
            id,
            version,
            client_id,
            client_version,
            command_logs,
        } => serialize(&(1u8, id, version, client_id, client_version, command_logs)),
        Message::Ready {
            id,
            version,
            capabilities,
        } => serialize(&(2u8, id, version, body(capabilities))),
        Message::Request { id, method, params } => serialize(&(3u8, id, method, body(params))),
        Message::Response {
            id,
            result: Some(value),
            error: None,
        } => serialize(&(4u8, id, body(value))),
        Message::Response {
            id,
            result: None,
            error: Some(e),
        } => serialize(&(5u8, id, (&e.code, &e.message, &e.retry))),
        Message::Response { .. } => Err(invalid("Expected exactly one result or error")),
        Message::CommandLog { id, entry: e } => serialize(&(
            6u8,
            id,
            (
                &e.command,
                e.duration_ms,
                e.exit_code,
                &e.output,
                e.interrupted,
            ),
        )),
        Message::Ping { nonce } => serialize(&(7u8, nonce)),
        Message::Pong { nonce } => serialize(&(8u8, nonce)),
        Message::Begin {
            id,
            stream_id,
            snapshot,
        } => serialize(&(9u8, id, stream_id, snapshot)),
        Message::Chunk {
            id,
            stream_id,
            seq,
            bytes_b64,
        } => {
            let bytes = STANDARD.decode(bytes_b64).map_err(invalid)?;
            serialize(&(10u8, id, stream_id, seq, serde_bytes::Bytes::new(&bytes)))
        }
        Message::Ack { stream_id, seq } => serialize(&(11u8, stream_id, seq)),
    }
}
struct Envelope(Message);
impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Messages;
        fn next<'de, T: Deserialize<'de>, A: SeqAccess<'de>>(seq: &mut A) -> Result<T, A::Error> {
            seq.next_element()?
                .ok_or_else(|| de::Error::custom("Missing positional field"))
        }
        impl<'de> Visitor<'de> for Messages {
            type Value = Envelope;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a positional message envelope")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Envelope, A::Error> {
                let tag: u8 = next(&mut s)?;
                let message = match tag {
                    0 => Message::Hello {
                        protocol: next(&mut s)?,
                        versions: next(&mut s)?,
                        instance_id: next(&mut s)?,
                        limits: next::<Body, _>(&mut s)?.0,
                    },
                    1 => Message::Initialize {
                        id: next(&mut s)?,
                        version: next(&mut s)?,
                        client_id: next(&mut s)?,
                        client_version: next(&mut s)?,
                        command_logs: next(&mut s)?,
                    },
                    2 => Message::Ready {
                        id: next(&mut s)?,
                        version: next(&mut s)?,
                        capabilities: next::<Body, _>(&mut s)?.0,
                    },
                    3 => Message::Request {
                        id: next(&mut s)?,
                        method: next(&mut s)?,
                        params: next::<Body, _>(&mut s)?.0,
                    },
                    4 => Message::success(next(&mut s)?, next::<Body, _>(&mut s)?.0),
                    5 => {
                        let id = next(&mut s)?;
                        let (code, message, retry) = next(&mut s)?;
                        Message::failure(
                            id,
                            Error {
                                code,
                                message,
                                retry,
                            },
                        )
                    }
                    6 => {
                        let id = next(&mut s)?;
                        let (command, duration_ms, exit_code, output, interrupted) = next(&mut s)?;
                        Message::CommandLog {
                            id,
                            entry: CommandLog {
                                command,
                                duration_ms,
                                exit_code,
                                output,
                                interrupted,
                            },
                        }
                    }
                    7 => Message::Ping {
                        nonce: next(&mut s)?,
                    },
                    8 => Message::Pong {
                        nonce: next(&mut s)?,
                    },
                    9 => Message::Begin {
                        id: next(&mut s)?,
                        stream_id: next(&mut s)?,
                        snapshot: next(&mut s)?,
                    },
                    10 => {
                        let id = next(&mut s)?;
                        let stream_id = next(&mut s)?;
                        let seq = next(&mut s)?;
                        let bytes: serde_bytes::ByteBuf = next(&mut s)?;
                        Message::Chunk {
                            id,
                            stream_id,
                            seq,
                            bytes_b64: STANDARD.encode(bytes),
                        }
                    }
                    11 => Message::Ack {
                        stream_id: next(&mut s)?,
                        seq: next(&mut s)?,
                    },
                    _ => return Err(de::Error::custom("Unknown message tag")),
                };
                if s.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("Extra positional field"));
                }
                Ok(Envelope(message))
            }
        }
        d.deserialize_seq(Messages)
    }
}
pub(super) fn decode(bytes: &[u8]) -> io::Result<Message> {
    deserialize::<Envelope>(bytes).map(|e| e.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn positional_envelopes_cover_every_message_and_reject_truncation() {
        let messages = vec![
            Message::Hello {
                protocol: "newport.git".into(),
                versions: vec![super::super::VERSION],
                instance_id: "instance".into(),
                limits: json!({"maxFrameBytes":1048576}),
            },
            Message::Initialize {
                id: "id".into(),
                version: super::super::VERSION,
                client_id: "client".into(),
                client_version: "test".into(),
                command_logs: true,
            },
            Message::Ready {
                id: "id".into(),
                version: super::super::VERSION,
                capabilities: json!({"methods":["repo.open"]}),
            },
            Message::Request {
                id: "id".into(),
                method: "repo.open".into(),
                params: json!({"path":{"bytesB64":STANDARD.encode(b"/tmp/nonutf8\xff"),"display":"test"}}),
            },
            Message::success("id".into(), json!({"entries":[],"nextCursor":null})),
            Message::failure("id".into(), Error::invalid("bad request")),
            Message::CommandLog {
                id: "id".into(),
                entry: CommandLog {
                    command: "git status".into(),
                    duration_ms: 9,
                    exit_code: Some(0),
                    output: "hello\rworld".into(),
                    interrupted: false,
                },
            },
            Message::Ping { nonce: "x".into() },
            Message::Pong { nonce: "x".into() },
            Message::Begin {
                id: "id".into(),
                stream_id: "stream".into(),
                snapshot: "snapshot".into(),
            },
            Message::Chunk {
                id: "id".into(),
                stream_id: "stream".into(),
                seq: 1,
                bytes_b64: STANDARD.encode([0, 255, 10]),
            },
            Message::Ack {
                stream_id: "stream".into(),
                seq: 1,
            },
        ];
        for message in messages {
            let bytes = encode(&message).unwrap();
            assert!(
                (0x90..=0x9f).contains(&bytes[0]),
                "envelopes must be arrays"
            );
            assert_eq!(
                serde_json::to_value(decode(&bytes).unwrap()).unwrap(),
                serde_json::to_value(&message).unwrap()
            );
            for end in 0..bytes.len() {
                assert!(decode(&bytes[..end]).is_err());
            }
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert!(decode(&trailing).is_err());
        }
        assert_eq!(
            encode(&Message::Ping { nonce: "x".into() }).unwrap(),
            vec![0x92, 7, 0xa1, b'x']
        );
        for bytes in [
            vec![0x91, 99],
            vec![0x93, 7, 0xa1, b'x', 0],
            vec![0x81, 0, 7],
        ] {
            assert!(decode(&bytes).is_err());
        }
    }
    #[test]
    fn values_preserve_bytes_numeric_boundaries_and_diff_tuples() {
        let value = json!({"bytesB64":STANDARD.encode(b"\xff\0\n"),"extension":{"test":[u64::MAX,i64::MIN,1.25,null,true]},"lines":[[0,0,true,"+",null,1,1,STANDARD.encode(b"\xff\n"),"entry"]]});
        let bytes = encode_value(&value).unwrap();
        assert_eq!(decode_value(&bytes).unwrap(), value);
        assert_eq!(
            encode_value(&json!({"bytesB64":STANDARD.encode([0,255])})).unwrap(),
            vec![0x81, 14, 0xc4, 2, 0, 255]
        );
    }
    #[test]
    fn rejects_duplicate_fields_aliases_extensions_and_excessive_depth() {
        for bytes in [
            vec![0x82, 1, 0, 1, 1],                // Duplicate integer key.
            vec![0x82, 1, 0, 0xa2, b'i', b'd', 1], // Same field through string alias.
            vec![0x81, 0xcd, 0xff, 0xff, 0],       // Unknown integer field ID.
            vec![0xc4, 1, 0],                      // Binary outside a byte field.
            vec![0xd4, 1, 0],                      // Extension types are not part of this contract.
            vec![0x81, 1],                         // Truncated map.
        ] {
            assert!(decode_value(&bytes).is_err());
        }
        let mut deep = vec![0x91; 50];
        deep.push(0xc0);
        assert!(decode_value(&deep).is_err());
        assert!(decode_value(&vec![0; MAX_DIFF + 1]).is_err());
        assert!(decode_value(&rmp_serde::to_vec(&f64::NAN).unwrap()).is_err());
    }
}
