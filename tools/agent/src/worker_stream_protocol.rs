//! Registered worker IO over an authenticated SSH-forwarded Unix socket.
//! This wire is separate from runtime control and from each service's own
//! protocol. Both ends negotiate the capability before using this transport.
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{self, Read, Write},
};

pub const CAPABILITY: &str = "worker-stream";
pub const VERSION: u32 = 1;
pub const MAX_CHUNK: usize = 32 * 1024;
pub const MAX_ARGUMENTS: usize = 128;
pub const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
// JSON escaping can expand a valid argument byte into six encoded bytes.
pub const MAX_OPEN_BYTES: usize = 512 * 1024;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid worker stream frame")
}
fn identifier(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|value| !value.is_nil())
}
pub fn valid_ticket(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    pub path: String,
    /// Never include this bearer credential in Debug output, logs or argv.
    #[serde(rename = "ticket")]
    token: String,
}
impl fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Endpoint")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}
impl Endpoint {
    pub fn new(path: String, token: String) -> io::Result<Self> {
        let endpoint = Self { path, token };
        endpoint.validate()?;
        Ok(endpoint)
    }
    pub fn ticket(&self) -> &str {
        &self.token
    }
    pub fn validate(&self) -> io::Result<()> {
        if !self.path.starts_with('/')
            || self.path.len() > 1024
            || self.path.chars().any(char::is_control)
            || self.path[1..]
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
            || !valid_ticket(&self.token)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Open {
    pub version: u32,
    pub runtime_id: String,
    pub worker_id: String,
    pub build_sha256: String,
    pub ticket: String,
    pub service: String,
    #[serde(deserialize_with = "bounded_arguments")]
    pub arguments: Vec<String>,
}
fn bounded_arguments<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    struct Arguments;
    impl<'de> serde::de::Visitor<'de> for Arguments {
        type Value = Vec<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("bounded literal worker arguments")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut arguments = Vec::new();
            let mut bytes = 0;
            while let Some(value) = sequence.next_element::<String>()? {
                bytes += value.len();
                if arguments.len() >= MAX_ARGUMENTS
                    || value.len() > 16384
                    || value.contains('\0')
                    || bytes > MAX_ARGUMENT_BYTES
                {
                    return Err(serde::de::Error::custom("Worker argument limit exceeded"));
                }
                arguments.push(value);
            }
            Ok(arguments)
        }
    }
    deserializer.deserialize_seq(Arguments)
}
impl Open {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != VERSION
            || !identifier(&self.runtime_id)
            || !identifier(&self.worker_id)
            || !super::valid_build(&self.build_sha256)
            || !valid_ticket(&self.ticket)
            || self.service.is_empty()
            || self.service.len() > 64
            || !self
                .service
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || self.arguments.len() > MAX_ARGUMENTS
            || self
                .arguments
                .iter()
                .any(|value| value.len() > 16384 || value.contains('\0'))
            || self.arguments.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        if bytes.len() > MAX_OPEN_BYTES {
            return Err(invalid());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_OPEN_BYTES {
            return Err(invalid());
        }
        let open: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        open.validate()?;
        Ok(open)
    }
    /// The caller supplies a reader with one deadline for the entire opening
    /// exchange; per-read timeouts alone allow an unbounded trickle attack.
    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut length = [0; 4];
        reader.read_exact(&mut length)?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_OPEN_BYTES {
            return Err(invalid());
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes)?;
        Self::decode(&bytes)
    }
    pub fn write(&self, writer: &mut impl Write) -> io::Result<()> {
        let bytes = self.encode()?;
        writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
        writer.write_all(&bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Admission {
    /// A worker process was spawned, not an application launch accepted or a
    /// service ready. Each service still completes its own admission/handshake.
    Started {
        runtime_id: String,
        worker_id: String,
        build_sha256: String,
    },
    Refused {
        reason: Refusal,
    },
}
impl Admission {
    /// A successful acknowledgment is usable only for this exact admission.
    /// It never authorizes replay after an uncertain transport failure.
    pub fn validate_for(&self, expected: &Open) -> io::Result<()> {
        expected.validate()?;
        match self {
            Self::Started {
                runtime_id,
                worker_id,
                build_sha256,
            } if runtime_id != &expected.runtime_id
                || worker_id != &expected.worker_id
                || build_sha256 != &expected.build_sha256 =>
            {
                Err(invalid())
            }
            _ => Ok(()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Refusal {
    Unsupported,
    Invalid,
    Stale,
    Expired,
    Consumed,
    Busy,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ToWorker,
    FromWorker,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Input = 1,
    /// Close child stdin while keeping the forwarding transport open for
    /// output and final status. Transport EOF is cancellation, not this frame.
    InputEof = 2,
    Output = 3,
    Error = 4,
    ExitStatus = 5,
    ExitSignal = 6,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub length: usize,
}
impl Header {
    pub fn decode(bytes: [u8; 5], direction: Direction) -> io::Result<Self> {
        use Direction::*;
        use Kind::*;
        let kind = match (direction, bytes[0]) {
            (ToWorker, 1) => Input,
            (ToWorker, 2) => InputEof,
            (FromWorker, 3) => Output,
            (FromWorker, 4) => Error,
            (FromWorker, 5) => ExitStatus,
            (FromWorker, 6) => ExitSignal,
            _ => return Err(invalid()),
        };
        let length = u32::from_be_bytes(bytes[1..].try_into().unwrap()) as usize;
        let valid = match kind {
            Input | Output | Error => (1..=MAX_CHUNK).contains(&length),
            InputEof => length == 0,
            ExitStatus | ExitSignal => length == 4,
        };
        if !valid {
            return Err(invalid());
        }
        Ok(Self { kind, length })
    }
    pub fn encode(&self, direction: Direction) -> io::Result<[u8; 5]> {
        let length = u32::try_from(self.length).map_err(|_| invalid())?;
        let mut bytes = [0; 5];
        bytes[0] = self.kind as u8;
        bytes[1..].copy_from_slice(&length.to_be_bytes());
        Self::decode(bytes, direction)?;
        Ok(bytes)
    }
}
#[derive(PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub data: Vec<u8>,
}
impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Service payloads can contain license credentials or clipboard data.
        formatter
            .debug_struct("Frame")
            .field("kind", &self.kind)
            .field("length", &self.data.len())
            .finish_non_exhaustive()
    }
}
impl Frame {
    pub fn read(reader: &mut impl Read, direction: Direction) -> io::Result<Self> {
        let mut bytes = [0; 5];
        reader.read_exact(&mut bytes)?;
        let header = Header::decode(bytes, direction)?;
        let mut data = vec![0; header.length];
        reader.read_exact(&mut data)?;
        let frame = Self {
            kind: header.kind,
            data,
        };
        frame.validate(direction)?;
        Ok(frame)
    }
    pub fn validate(&self, direction: Direction) -> io::Result<()> {
        Header {
            kind: self.kind,
            length: self.data.len(),
        }
        .encode(direction)?;
        if self.kind == Kind::ExitSignal {
            let signal = u32::from_be_bytes(self.data[..].try_into().unwrap());
            if !(1..=64).contains(&signal) {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn write(&self, writer: &mut impl Write, direction: Direction) -> io::Result<()> {
        self.validate(direction)?;
        writer.write_all(
            &Header {
                kind: self.kind,
                length: self.data.len(),
            }
            .encode(direction)?,
        )?;
        writer.write_all(&self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn opening() -> Open {
        Open {
            version: VERSION,
            runtime_id: uuid::Uuid::new_v4().to_string(),
            worker_id: uuid::Uuid::new_v4().to_string(),
            build_sha256: "a".repeat(64),
            ticket: "b".repeat(64),
            service: "fixture".into(),
            arguments: vec![],
        }
    }
    #[test]
    fn opening_accepts_literal_arguments_with_bounded_escaping() {
        let mut request = opening();
        request.arguments = vec!["\u{1}".repeat(16384); 4];
        let encoded = request.encode().unwrap();
        assert!(encoded.len() > MAX_ARGUMENT_BYTES && encoded.len() <= MAX_OPEN_BYTES);
        assert_eq!(Open::decode(&encoded).unwrap().arguments, request.arguments);
        let mut wire = vec![];
        request.write(&mut wire).unwrap();
        assert_eq!(
            Open::read(&mut wire.as_slice()).unwrap().arguments,
            request.arguments
        );
        request.arguments.push("x".into());
        assert!(request.encode().is_err());
        for mutate in [
            |v: &mut Open| v.version += 1,
            |v: &mut Open| v.runtime_id = uuid::Uuid::nil().to_string(),
            |v: &mut Open| v.worker_id.clear(),
            |v: &mut Open| v.build_sha256 = "other".into(),
            |v: &mut Open| v.ticket = "short".into(),
            |v: &mut Open| v.service = "../bin/sh".into(),
            |v: &mut Open| v.arguments = vec!["a\0b".into()],
            |v: &mut Open| v.arguments = vec!["".into(); MAX_ARGUMENTS + 1],
        ] {
            let mut value = opening();
            mutate(&mut value);
            assert!(value.encode().is_err());
            assert!(Open::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }
    #[test]
    fn admission_acknowledgment_cannot_be_reused_across_workers_or_builds() {
        let request = opening();
        let started = Admission::Started {
            runtime_id: request.runtime_id.clone(),
            worker_id: request.worker_id.clone(),
            build_sha256: request.build_sha256.clone(),
        };
        assert!(started.validate_for(&request).is_ok());
        for mutate in [
            |v: &mut Open| v.runtime_id = uuid::Uuid::new_v4().to_string(),
            |v: &mut Open| v.worker_id = uuid::Uuid::new_v4().to_string(),
            |v: &mut Open| v.build_sha256 = "c".repeat(64),
        ] {
            let mut other = request.clone();
            mutate(&mut other);
            assert!(started.validate_for(&other).is_err());
        }
        let mut bytes = vec![];
        super::super::write(&mut bytes, &started).unwrap();
        let decoded: Admission = super::super::read(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded, started);
        assert!(decoded.validate_for(&request).is_ok());
    }
    #[test]
    fn diagnostics_do_not_expose_admission_tickets_or_service_payloads() {
        let ticket = "d".repeat(64);
        let endpoint =
            Endpoint::new("/run/user/1000/newport/stream.sock".into(), ticket.clone()).unwrap();
        assert!(!format!("{endpoint:?}").contains(&ticket));
        assert_eq!(endpoint.ticket(), ticket);
        let frame = Frame {
            kind: Kind::Input,
            data: b"private clipboard content".to_vec(),
        };
        assert!(!format!("{frame:?}").contains("private clipboard content"));
        assert!(!format!("{frame:?}").contains("112, 114, 105"));
        for path in [
            "relative.sock",
            "/",
            "/run//stream",
            "/run/../stream",
            "/run/./stream",
            "/run/stream\n",
        ] {
            assert!(Endpoint::new(path.into(), ticket.clone()).is_err());
        }
        assert!(Endpoint::new("/run/stream".into(), "short".into()).is_err());
    }
    #[test]
    fn malformed_headers_are_refused_before_payload_reads() {
        for size in [0, u32::MAX, MAX_OPEN_BYTES as u32 + 1] {
            assert_eq!(
                Open::read(&mut &size.to_be_bytes()[..])
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        for (kind, length, direction) in [
            (Kind::Output, u32::MAX, Direction::FromWorker),
            (Kind::Output, 0, Direction::FromWorker),
            (Kind::InputEof, 1, Direction::ToWorker),
            (Kind::ExitStatus, 8, Direction::FromWorker),
            (Kind::Output, 8, Direction::ToWorker),
            (Kind::Input, 8, Direction::FromWorker),
        ] {
            let mut bytes = vec![kind as u8];
            bytes.extend(length.to_be_bytes());
            assert_eq!(
                Frame::read(&mut bytes.as_slice(), direction)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
    #[test]
    fn binary_streams_preserve_stdout_stderr_half_close_and_exit() {
        for (direction, frames) in [
            (
                Direction::ToWorker,
                vec![
                    Frame {
                        kind: Kind::Input,
                        data: vec![0, 255, 13, 10],
                    },
                    Frame {
                        kind: Kind::InputEof,
                        data: vec![],
                    },
                ],
            ),
            (
                Direction::FromWorker,
                vec![
                    Frame {
                        kind: Kind::Output,
                        data: vec![17; MAX_CHUNK],
                    },
                    Frame {
                        kind: Kind::Error,
                        data: b"diagnostic".to_vec(),
                    },
                    Frame {
                        kind: Kind::ExitStatus,
                        data: 23_u32.to_be_bytes().to_vec(),
                    },
                ],
            ),
        ] {
            let mut bytes = vec![];
            for frame in &frames {
                frame.write(&mut bytes, direction).unwrap();
            }
            let mut reader = bytes.as_slice();
            for frame in frames {
                assert_eq!(Frame::read(&mut reader, direction).unwrap(), frame);
            }
            assert!(reader.is_empty());
            assert_eq!(
                Frame::read(&mut reader, direction).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        for signal in [0_u32, 65, u32::MAX] {
            assert!(
                Frame {
                    kind: Kind::ExitSignal,
                    data: signal.to_be_bytes().to_vec()
                }
                .validate(Direction::FromWorker)
                .is_err()
            );
        }
    }
}
