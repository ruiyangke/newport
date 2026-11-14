//! Display admission: announce support before handing over a clipboard grant.
//! A prepared relay sends typed access on the same connection and waits for
//! authentication before forwarding Remote Apps frames.
#[cfg(feature = "protobuf")]
use crate::encoding::protobuf::{self as codec, generated::newport::remote::display as pb};
use crate::{
    clipboard::Access,
    common::{Error, Limits, Present, Retry},
};
use serde::{Deserialize, Serialize};
#[cfg(feature = "protobuf")]
use std::io::{Read, Write};

/// A relay's identity and optional clipboard access.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attach {
    pub session: String,
    pub computer: String,
    pub clipboard: Option<Access>,
}

/// An extensible operation envelope. Its body is interpreted only after admission.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Envelope {
    pub op: String,
    pub revision: u32,
    pub build: String,
    pub body: bytes::Bytes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvelopeReply {
    /// None when a peer replies with a case this build does not know.
    pub result: Option<Reply>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Accepted(Present),
    Refused(Refused),
    Ready(DisplayIdentity),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refused {
    pub error: Option<Error>,
    pub display: Option<DisplayIdentity>,
}

/// A credential-free request to begin clipboard admission.
pub const PREPARE: &str = "prepare-clipboard-attach";
/// Why a display refused an envelope of another revision.
pub const DISPLAY_REVISION: &str = "display-revision";
/// Why a display refused an op it doesn't know.
pub const UNKNOWN_OP: &str = "unknown-op";
/// Why a display refused an attach it can't serve.
pub const REFUSED: &str = "refused";
/// What an envelope, its body or its reply may take.
pub const LIMITS: Limits = Limits {
    message: 64 * 1024,
    metadata: 64 * 1024,
};
#[cfg(feature = "protobuf")]
const MAX_COMPUTER_BYTES: usize = 1024;

/// Which display answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayIdentity {
    pub revision: u32,
    pub build: String,
}
impl DisplayIdentity {
    /// This build's.
    pub fn own(build: &str) -> Self {
        Self {
            revision: crate::remote_apps::REVISION,
            build: build.to_owned(),
        }
    }
}

/// Ask whether this display accepts the typed clipboard handoff.
pub fn prepare(build: &str) -> Envelope {
    Envelope {
        op: PREPARE.into(),
        revision: crate::remote_apps::REVISION,
        build: build.into(),
        body: bytes::Bytes::new(),
    }
}

/// Validate the credential-free phase without admitting a viewer.
pub fn admit_prepare(envelope: Envelope, build: &str) -> Result<(), Refused> {
    let refused = |code: &str, message: String| Refused {
        error: Some(Error {
            code: code.into(),
            message,
            retry: Retry::No,
        }),
        display: Some(DisplayIdentity::own(build)),
    };
    if envelope.revision != crate::remote_apps::REVISION {
        return Err(refused(
            DISPLAY_REVISION,
            format!(
                "This server's apps use display protocol {}, and this Newport speaks {}. Close and reopen the server's apps after updating Newport.",
                crate::remote_apps::REVISION,
                envelope.revision
            ),
        ));
    }
    if envelope.op != PREPARE {
        return Err(refused(
            UNKNOWN_OP,
            format!("The display doesn't know \"{}\".", envelope.op),
        ));
    }
    if !envelope.body.is_empty() {
        return Err(refused(
            REFUSED,
            "Display preparation must contain no clipboard access.".into(),
        ));
    }
    Ok(())
}

/// Announces support for a typed attach on this connection.
pub fn ready(build: &str) -> EnvelopeReply {
    EnvelopeReply {
        result: Some(Reply::Ready(DisplayIdentity::own(build))),
    }
}

/// A reply that accepts the envelope.
pub fn accepted() -> EnvelopeReply {
    EnvelopeReply {
        result: Some(Reply::Accepted(Present {})),
    }
}
/// A reply that refuses it.
pub fn refusal(refused: Refused) -> EnvelopeReply {
    EnvelopeReply {
        result: Some(Reply::Refused(refused)),
    }
}

/// A display message with an optional protobuf representation.
#[cfg(feature = "protobuf")]
pub trait Message: sealed::Sealed + Sized {
    fn write(&self, writer: &mut impl Write) -> std::io::Result<()>;
    fn read(reader: &mut impl Read) -> std::io::Result<Option<Self>>;
}
#[cfg(feature = "protobuf")]
mod sealed {
    pub trait Sealed {}
}

/// Writes one display message and flushes the writer.
#[cfg(feature = "protobuf")]
pub fn write(writer: &mut impl Write, message: &impl Message) -> std::io::Result<()> {
    message.write(writer)?;
    writer.flush()
}
/// Reads a display message; None when the stream ends before its first frame.
#[cfg(feature = "protobuf")]
pub fn read<M: Message>(reader: &mut impl Read) -> std::io::Result<Option<M>> {
    M::read(reader)
}

/// The refusal a relay passes on when the display's reply can't be read: a
/// display older than the envelope.
pub fn unreadable_reply() -> Refused {
    Refused {
        error: Some(Error {
            code: DISPLAY_REVISION.into(),
            message: "This server's display does not support this clipboard handoff. Close and reopen the server's apps after updating Newport.".into(),
            retry: Retry::No,
        }),
        display: None,
    }
}

/// The Remote Apps `Rejected` a relay sends its client for a refusal.
pub fn rejected(refused: Refused) -> crate::remote_apps::v1::ServerMessage {
    let error = refused.error.unwrap_or_default();
    crate::remote_apps::v1::ServerMessage::Rejected {
        reason: error.code,
        message: error.message,
        display: refused.display,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub display: Option<String>,
    pub authority: Option<String>,
    /// Already normalized to an absolute path; no ambient environment or fd.
    pub wayland_socket: String,
}

#[cfg(all(test, feature = "protobuf"))]
pub(crate) use protobuf::tables;

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::*;

    fn io(error: codec::FrameError) -> std::io::Error {
        match error {
            codec::FrameError::Io(error) => error,
            codec::FrameError::Truncated => {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, error)
            }
            error => std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        }
    }
    fn validate_attach(value: &Attach) -> std::io::Result<()> {
        if value.session.is_empty()
            || value.session.len() > 64
            || !value
                .session
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value.computer.len() > MAX_COMPUTER_BYTES
            || value.computer.contains('\0')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid display attachment identity",
            ));
        }
        Ok(())
    }
    fn attach_to_wire(value: &Attach) -> std::io::Result<pb::Attach> {
        validate_attach(value)?;
        Ok(pb::Attach {
            session: value.session.clone(),
            computer: value.computer.clone(),
            clipboard: value
                .clipboard
                .as_ref()
                .map(crate::clipboard::service::wire::access_to_wire)
                .transpose()
                .map_err(invalid)?,
        })
    }
    fn attach_from_wire(value: pb::Attach) -> std::io::Result<Attach> {
        let value = Attach {
            session: value.session,
            computer: value.computer,
            clipboard: value
                .clipboard
                .map(crate::clipboard::service::wire::access_from_wire)
                .transpose()
                .map_err(invalid)?,
        };
        validate_attach(&value)?;
        Ok(value)
    }
    impl From<&Envelope> for pb::Envelope {
        fn from(value: &Envelope) -> Self {
            Self {
                op: value.op.clone(),
                revision: value.revision,
                build: value.build.clone(),
                body: value.body.clone(),
            }
        }
    }
    impl From<pb::Envelope> for Envelope {
        fn from(value: pb::Envelope) -> Self {
            Self {
                op: value.op,
                revision: value.revision,
                build: value.build,
                body: value.body,
            }
        }
    }
    impl From<&Refused> for pb::Refused {
        fn from(value: &Refused) -> Self {
            Self {
                error: value.error.as_ref().map(|value| codec::Error {
                    code: value.code.clone(),
                    message: value.message.clone(),
                    retry: match value.retry {
                        Retry::Unknown => codec::Retry::Unknown,
                        Retry::Yes => codec::Retry::Yes,
                        Retry::No => codec::Retry::No,
                    }
                    .into(),
                }),
                display: value.display.as_ref().map(Into::into),
            }
        }
    }
    impl From<pb::Refused> for Refused {
        fn from(value: pb::Refused) -> Self {
            Self {
                error: value.error.map(|value| Error {
                    code: value.code,
                    message: value.message,
                    retry: match codec::Retry::try_from(value.retry) {
                        Ok(codec::Retry::Yes) => Retry::Yes,
                        Ok(codec::Retry::No) => Retry::No,
                        _ => Retry::Unknown,
                    },
                }),
                display: value.display.map(Into::into),
            }
        }
    }
    impl From<&EnvelopeReply> for pb::EnvelopeReply {
        fn from(value: &EnvelopeReply) -> Self {
            Self {
                result: value.result.as_ref().map(|reply| match reply {
                    Reply::Accepted(_) => pb::envelope_reply::Result::Accepted(codec::Present {}),
                    Reply::Refused(refused) => pb::envelope_reply::Result::Refused(refused.into()),
                    Reply::Ready(display) => pb::envelope_reply::Result::Ready(display.into()),
                }),
            }
        }
    }
    impl From<pb::EnvelopeReply> for EnvelopeReply {
        fn from(value: pb::EnvelopeReply) -> Self {
            Self {
                result: value.result.map(|reply| match reply {
                    pb::envelope_reply::Result::Accepted(_) => Reply::Accepted(Present {}),
                    pb::envelope_reply::Result::Refused(refused) => Reply::Refused(refused.into()),
                    pb::envelope_reply::Result::Ready(display) => Reply::Ready(display.into()),
                }),
            }
        }
    }
    fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
    }

    #[derive(Clone, Copy)]
    #[cfg_attr(test, derive(Debug))]
    enum Fields {
        Envelope,
        Attach,
        Access,
        Secret,
        Identity,
        Refused,
        Error,
        Present,
    }
    enum Shape {
        Integer,
        Bytes,
        Message(Fields),
    }
    fn shape(fields: Fields, number: u32) -> Option<Shape> {
        use Fields as F;
        use Shape as S;
        match (fields, number) {
            (F::Envelope, 1 | 3 | 4)
            | (F::Attach, 1 | 2)
            | (F::Access, 1..=3)
            | (F::Secret, 1)
            | (F::Identity, 2)
            | (F::Error, 1 | 2) => Some(S::Bytes),
            (F::Envelope, 2) | (F::Identity, 1) | (F::Error, 3) => Some(S::Integer),
            (F::Attach, 3) => Some(S::Message(F::Access)),
            (F::Access, 4) => Some(S::Message(F::Secret)),
            (F::Refused, 1) => Some(S::Message(F::Error)),
            (F::Refused, 2) => Some(S::Message(F::Identity)),
            _ => None,
        }
    }
    fn scan(mut body: &[u8], kind: Fields) -> Result<(), codec::FrameError> {
        let mut seen = 0u8;
        while !body.is_empty() {
            let field = codec::fields::field(&mut body)?;
            let Some(shape) = shape(kind, field.number) else {
                continue;
            };
            let bit = 1 << field.number;
            if seen & bit != 0 {
                return Err(codec::FrameError::Malformed);
            }
            seen |= bit;
            match shape {
                Shape::Integer => {
                    field.u32()?;
                }
                Shape::Bytes => {
                    field.bytes()?;
                }
                Shape::Message(fields) => scan(field.bytes()?, fields)?,
            }
        }
        Ok(())
    }
    /// The case an `EnvelopeReply` holds in field `number`.
    fn reply(number: u32) -> Option<Fields> {
        Some(match number {
            1 => Fields::Present,
            2 => Fields::Refused,
            3 => Fields::Identity,
            _ => return None,
        })
    }
    fn scan_reply(body: &[u8]) -> Result<(), codec::FrameError> {
        let field = codec::fields::single(body)?;
        let Some(kind) = reply(field.number) else {
            return Ok(());
        };
        scan(field.bytes()?, kind)
    }

    #[cfg(test)]
    use crate::desktop::wire::wire_tables::{Row, Table, Wire, numbers};

    #[cfg(test)]
    pub(crate) fn tables() -> Vec<(Table, &'static str)> {
        vec![
            (table(Fields::Envelope), "newport.remote.display.Envelope"),
            (
                Table::dispatch("display reply", |number| reply(number).map(table)),
                "newport.remote.display.EnvelopeReply",
            ),
            (table(Fields::Attach), "newport.remote.display.Attach"),
        ]
    }

    #[cfg(test)]
    #[track_caller]
    fn table(fields: Fields) -> Table {
        Table::new(format!("display {fields:?}"), move || {
            numbers()
                .filter_map(|number| {
                    let wire = match shape(fields, number)? {
                        Shape::Integer => Wire::Varint32,
                        Shape::Bytes => Wire::Len,
                        Shape::Message(child) => Wire::Message(table(child)),
                    };
                    Some(Row::new(number, wire))
                })
                .collect()
        })
    }

    macro_rules! message {
        ($rust:ty, $wire:ty, $scan:expr, $to:expr, $from:expr) => {
            impl sealed::Sealed for $rust {}
            impl Message for $rust {
                fn write(&self, writer: &mut impl Write) -> std::io::Result<()> {
                    let wire: $wire = ($to)(self)?;
                    codec::write_message(writer, &wire, &LIMITS).map_err(io)
                }
                fn read(reader: &mut impl Read) -> std::io::Result<Option<Self>> {
                    let Some(body) = codec::read_frame(reader, LIMITS.message).map_err(io)? else {
                        return Ok(None);
                    };
                    ($scan)(&body).map_err(io)?;
                    let wire: $wire = codec::decode(body, &LIMITS).map_err(io)?;
                    ($from)(wire).map(Some)
                }
            }
        };
    }
    message!(
        Envelope,
        pb::Envelope,
        |body| scan(body, Fields::Envelope),
        |value: &Envelope| Ok::<_, std::io::Error>(pb::Envelope::from(value)),
        |value: pb::Envelope| Ok::<_, std::io::Error>(Envelope::from(value))
    );
    message!(
        EnvelopeReply,
        pb::EnvelopeReply,
        scan_reply,
        |value: &EnvelopeReply| Ok::<_, std::io::Error>(pb::EnvelopeReply::from(value)),
        |value: pb::EnvelopeReply| Ok::<_, std::io::Error>(EnvelopeReply::from(value))
    );
    message!(
        Attach,
        pb::Attach,
        |body| scan(body, Fields::Attach),
        attach_to_wire,
        attach_from_wire
    );
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use prost::Message as _;

    fn access() -> Access {
        Access {
            path: "/run/user/1000/newport/clipboard.sock".into(),
            runtime: "runtime".into(),
            worker: "display".into(),
            token: "11111111-2222-3333-4444-555555555555".into(),
        }
    }
    fn attach() -> Attach {
        Attach {
            session: "s-1".into(),
            computer: "studio-mac".into(),
            clipboard: Some(access()),
        }
    }
    fn framed(body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        codec::write_frame(&mut bytes, body).unwrap();
        bytes
    }
    fn nested(number: u32, body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        prost::encoding::encode_key(
            number,
            prost::encoding::WireType::LengthDelimited,
            &mut bytes,
        );
        prost::encoding::encode_varint(u64::try_from(body.len()).unwrap(), &mut bytes);
        bytes.extend_from_slice(body);
        bytes
    }
    fn wire_attach() -> pb::Attach {
        pb::Attach {
            session: "s-1".into(),
            computer: "studio-mac".into(),
            clipboard: Some(crate::clipboard::service::wire::access_to_wire(&access()).unwrap()),
        }
    }

    #[test]
    fn prepare_is_credential_free_and_ready_names_the_display() {
        let probe = prepare("relay-build");
        assert_eq!(probe.op, PREPARE);
        assert!(probe.body.is_empty());
        assert!(admit_prepare(probe.clone(), "display-build").is_ok());
        let mut bytes = Vec::new();
        write(&mut bytes, &probe).unwrap();
        assert_eq!(
            read::<Envelope>(&mut bytes.as_slice()).unwrap(),
            Some(probe)
        );
        bytes.clear();
        write(&mut bytes, &ready("display-build")).unwrap();
        assert_eq!(
            read::<EnvelopeReply>(&mut bytes.as_slice()).unwrap(),
            Some(ready("display-build"))
        );
    }

    #[test]
    fn preparation_rejects_wrong_revisions_operations_and_nonempty_bodies() {
        for (probe, code) in [
            (
                Envelope {
                    revision: 0,
                    ..prepare("relay")
                },
                DISPLAY_REVISION,
            ),
            (
                Envelope {
                    revision: crate::remote_apps::REVISION + 1,
                    ..prepare("relay")
                },
                DISPLAY_REVISION,
            ),
            (
                Envelope {
                    op: "attach".into(),
                    ..prepare("relay")
                },
                UNKNOWN_OP,
            ),
            (
                Envelope {
                    body: vec![1].into(),
                    ..prepare("relay")
                },
                REFUSED,
            ),
        ] {
            let refusal = admit_prepare(probe, "display").unwrap_err();
            assert_eq!(refusal.error.as_ref().unwrap().code, code);
            assert_eq!(refusal.display.as_ref().unwrap().build, "display");
            assert!(matches!(
                rejected(refusal),
                crate::remote_apps::v1::ServerMessage::Rejected { .. }
            ));
        }
    }

    #[test]
    fn typed_access_roundtrips_and_debug_excludes_its_credential() {
        let value = attach();
        let mut bytes = Vec::new();
        write(&mut bytes, &value).unwrap();
        assert_eq!(
            read::<Attach>(&mut bytes.as_slice()).unwrap(),
            Some(value.clone())
        );
        let debug = format!("{value:?}");
        assert!(!debug.contains(&access().token));
        assert!(!debug.contains("clipboard.sock"));
        for value in [
            Attach {
                session: String::new(),
                ..attach()
            },
            Attach {
                computer: "x".repeat(MAX_COMPUTER_BYTES + 1),
                ..attach()
            },
            Attach {
                computer: "bad\0name".into(),
                ..attach()
            },
            Attach {
                clipboard: Some(Access {
                    token: "invalid".into(),
                    ..access()
                }),
                ..attach()
            },
        ] {
            let mut bytes = Vec::new();
            assert!(write(&mut bytes, &value).is_err());
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn duplicate_prepare_fields_and_reply_cases_are_rejected() {
        let mut probe = pb::Envelope::from(&prepare("relay")).encode_to_vec();
        probe.extend(nested(1, PREPARE.as_bytes()));
        assert!(read::<Envelope>(&mut framed(&probe).as_slice()).is_err());
        let ready = pb::EnvelopeReply::from(&ready("display")).encode_to_vec();
        let accepted = pb::EnvelopeReply::from(&accepted()).encode_to_vec();
        for tail in [&ready, &accepted] {
            let mut mixed = ready.clone();
            mixed.extend_from_slice(tail);
            assert!(read::<EnvelopeReply>(&mut framed(&mixed).as_slice()).is_err());
        }
        let mut identity = pb::DisplayIdentity {
            revision: 1,
            build: "display".into(),
        }
        .encode_to_vec();
        identity.extend_from_slice(&[0x08, 0x01]);
        assert!(read::<EnvelopeReply>(&mut framed(&nested(3, &identity)).as_slice()).is_err());
    }

    #[test]
    fn duplicate_nested_access_fields_cannot_change_the_authority() {
        let access = crate::clipboard::service::wire::access_to_wire(&access()).unwrap();
        let mut duplicated = access.encode_to_vec();
        duplicated.extend(nested(2, b"another-runtime"));
        let mut attach = wire_attach();
        attach.clipboard = None;
        let mut body = attach.encode_to_vec();
        body.extend(nested(3, &duplicated));
        assert!(read::<Attach>(&mut framed(&body).as_slice()).is_err());
        let mut secret = access.token.unwrap().encode_to_vec();
        secret.extend(nested(1, b"another-token"));
        let mut authority =
            crate::clipboard::service::wire::access_to_wire(&self::access()).unwrap();
        authority.token = None;
        let mut authority = authority.encode_to_vec();
        authority.extend(nested(4, &secret));
        let mut body = attach.encode_to_vec();
        body.extend(nested(3, &authority));
        assert!(read::<Attach>(&mut framed(&body).as_slice()).is_err());
    }

    #[test]
    fn additive_unknown_fields_are_skipped_inside_the_existing_budget() {
        let mut probe = pb::Envelope::from(&prepare("relay")).encode_to_vec();
        probe.extend_from_slice(&[0xa0, 0x01, 0x01]);
        let decoded = read::<Envelope>(&mut framed(&probe).as_slice())
            .unwrap()
            .unwrap();
        assert!(admit_prepare(decoded, "display").is_ok());
        let mut access = crate::clipboard::service::wire::access_to_wire(&access())
            .unwrap()
            .encode_to_vec();
        access.extend(nested(31, b"future"));
        let mut attach = wire_attach();
        attach.clipboard = None;
        let mut body = attach.encode_to_vec();
        body.extend(nested(3, &access));
        assert_eq!(
            read::<Attach>(&mut framed(&body).as_slice()).unwrap(),
            Some(self::attach())
        );
        let mut oversized = Vec::new();
        prost::encoding::encode_varint(u64::try_from(LIMITS.message + 1).unwrap(), &mut oversized);
        assert_eq!(
            read::<Attach>(&mut oversized.as_slice())
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn eof_and_truncation_are_distinct() {
        assert!(read::<Envelope>(&mut [].as_slice()).unwrap().is_none());
        assert_eq!(
            read::<Envelope>(&mut [0x05, 0x0a].as_slice())
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }
}
