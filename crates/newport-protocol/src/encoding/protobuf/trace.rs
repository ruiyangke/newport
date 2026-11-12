//! Captured streams printed as Protocol Buffers JSON, one message a line.

use std::{
    error::Error,
    io::{Read, Write},
};

use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MapKey, ReflectMessage,
    SerializeOptions, Value,
};
use serde_json::{Map, Value as Json, json};

use crate::encoding::protobuf::read_frame;

/// Larger than any protocol's limit, so the trace shows what a peer refused.
/// A body is read as it arrives, so a length alone allocates little.
const FRAME_LIMIT: usize = 1 << 30;

const SECRET: &str = "newport.wire.Secret";
const REDACTED: &str = "[redacted]";

type BoxError = Box<dyn Error + Send + Sync>;

/// Prints every frame of `capture` as the message `message` (a fully
/// qualified name in `descriptors`, an encoded `FileDescriptorSet`), with
/// `bytes` fields as their length and `newport.wire.Secret`s redacted.
///
/// A frame that doesn't decode prints as `{"@error": …, "@frame": index,
/// "@length": bytes}` and the trace goes on; a stream that can't be framed
/// ends it with an error.
pub fn trace(
    descriptors: &[u8],
    message: &str,
    capture: &mut (impl Read + ?Sized),
    output: &mut (impl Write + ?Sized),
) -> Result<(), BoxError> {
    let pool = DescriptorPool::decode(descriptors)?;
    let descriptor = pool
        .get_message_by_name(message)
        .ok_or_else(|| format!("{message} is not in the descriptor set"))?;
    let mut index = 0usize;
    while let Some(frame) =
        read_frame(capture, FRAME_LIMIT).map_err(|error| format!("frame {index}: {error}"))?
    {
        let length = frame.len();
        let json = DynamicMessage::decode(descriptor.clone(), frame)
            .map_err(BoxError::from)
            .and_then(|message| to_json(&message))
            .unwrap_or_else(
                |error| json!({ "@error": error.to_string(), "@frame": index, "@length": length }),
            );
        serde_json::to_writer(&mut *output, &json)?;
        output.write_all(b"\n")?;
        index += 1;
    }
    Ok(())
}

/// The well-known types with JSON forms of their own, which an `Any` holds
/// under `value`.
const WELL_KNOWN: &[&str] = &[
    "google.protobuf.Any",
    "google.protobuf.Timestamp",
    "google.protobuf.Duration",
    "google.protobuf.Struct",
    "google.protobuf.FloatValue",
    "google.protobuf.DoubleValue",
    "google.protobuf.Int32Value",
    "google.protobuf.Int64Value",
    "google.protobuf.UInt32Value",
    "google.protobuf.UInt64Value",
    "google.protobuf.BoolValue",
    "google.protobuf.StringValue",
    "google.protobuf.BytesValue",
    "google.protobuf.FieldMask",
    "google.protobuf.ListValue",
    "google.protobuf.Value",
    "google.protobuf.Empty",
];

/// `message` as JSON. Fields holding bytes or messages are left out of the
/// serialization and written here, so bytes never print and an `Any` of a
/// type the descriptor set lacks doesn't fail it.
fn to_json(message: &DynamicMessage) -> Result<Json, BoxError> {
    let descriptor = message.descriptor();
    match descriptor.full_name() {
        SECRET => return Ok(Json::from(REDACTED)),
        "google.protobuf.BytesValue" => {
            let len = message
                .get_field_by_name("value")
                .and_then(|value| value.as_bytes().map(|bytes| bytes.len()))
                .unwrap_or_default();
            return Ok(Json::from(len));
        }
        "google.protobuf.Any" => return any_json(message),
        // The other well-known types hold no bytes and no messages but their own.
        name if name.starts_with("google.protobuf.") => {
            return Ok(message.serialize_with_options(
                serde_json::value::Serializer,
                &SerializeOptions::new(),
            )?);
        }
        _ => {}
    }
    let mut rest = message.clone();
    let mut held = Vec::new();
    for (field, value) in message.fields() {
        if holds_bytes_or_messages(&field) {
            rest.clear_field(&field);
            held.push((field, value));
        }
    }
    let mut json =
        rest.serialize_with_options(serde_json::value::Serializer, &SerializeOptions::new())?;
    let object = json
        .as_object_mut()
        .ok_or_else(|| format!("{} did not serialize as an object", descriptor.full_name()))?;
    for (field, value) in held {
        object.insert(field.json_name().to_owned(), value_json(value)?);
    }
    Ok(json)
}

/// An `Any` as its type and the message it holds, or redacted whole when
/// the descriptor set lacks the type.
fn any_json(any: &DynamicMessage) -> Result<Json, BoxError> {
    let type_url = any
        .get_field_by_name("type_url")
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let name = type_url.rsplit('/').next().unwrap_or_default();
    let Some(descriptor) = any.descriptor().parent_pool().get_message_by_name(name) else {
        return Ok(json!({ "@type": type_url, "redacted": true }));
    };
    let value = any
        .get_field_by_name("value")
        .and_then(|value| value.as_bytes().cloned())
        .unwrap_or_default();
    let inner = to_json(&DynamicMessage::decode(descriptor, value)?)?;
    let well_known = WELL_KNOWN.contains(&name);
    let mut object = Map::new();
    object.insert("@type".to_owned(), Json::from(type_url));
    match inner {
        Json::Object(fields) if !well_known => object.extend(fields),
        inner => {
            object.insert("value".to_owned(), inner);
        }
    }
    Ok(Json::Object(object))
}

fn holds_bytes_or_messages(field: &FieldDescriptor) -> bool {
    let kind = match field.kind() {
        Kind::Message(entry) if field.is_map() => entry.map_entry_value_field().kind(),
        kind => kind,
    };
    matches!(kind, Kind::Bytes | Kind::Message(_))
}

/// A field that [`holds_bytes_or_messages`] as JSON.
fn value_json(value: &Value) -> Result<Json, BoxError> {
    Ok(match value {
        Value::Bytes(bytes) => Json::from(bytes.len()),
        Value::Message(message) => to_json(message)?,
        Value::List(values) => values.iter().map(value_json).collect::<Result<_, _>>()?,
        Value::Map(entries) => entries
            .iter()
            .map(|(key, value)| Ok((map_key(key), value_json(value)?)))
            .collect::<Result<Map<_, _>, BoxError>>()?
            .into(),
        _ => Json::Null,
    })
}

fn map_key(key: &MapKey) -> String {
    match key {
        MapKey::Bool(key) => key.to_string(),
        MapKey::I32(key) => key.to_string(),
        MapKey::I64(key) => key.to_string(),
        MapKey::U32(key) => key.to_string(),
        MapKey::U64(key) => key.to_string(),
        MapKey::String(key) => key.clone(),
    }
}
