//! Field access for internal read models, never a wire decoder. Errors
//! name only the field; source values can contain credentials or large content.
use super::failure::invalid_result;
use newport_protocol::desktop::Failure;
use serde_json::Value;

pub(super) fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, Failure> {
    value.get(key).ok_or_else(|| invalid_result(key))
}
pub(super) fn text(value: &Value) -> Result<String, Failure> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid_result("text"))
}
pub(super) fn bool_value(value: &Value) -> Result<bool, Failure> {
    value.as_bool().ok_or_else(|| invalid_result("boolean"))
}
pub(super) fn unsigned(value: &Value) -> Result<u64, Failure> {
    value
        .as_u64()
        .ok_or_else(|| invalid_result("unsigned integer"))
}
pub(super) fn signed(value: &Value) -> Result<i64, Failure> {
    value
        .as_i64()
        .ok_or_else(|| invalid_result("signed integer"))
}
pub(super) fn number(value: &Value) -> Result<f64, Failure> {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid_result("number"))
}
pub(super) fn optional<T>(
    value: &Value,
    key: &str,
    read: impl FnOnce(&Value) -> Result<T, Failure>,
) -> Result<Option<T>, Failure> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => read(value).map(Some),
    }
}
pub(super) fn list<T>(
    value: &Value,
    mut read: impl FnMut(&Value) -> Result<T, Failure>,
) -> Result<Vec<T>, Failure> {
    value
        .as_array()
        .ok_or_else(|| invalid_result("list"))?
        .iter()
        .map(&mut read)
        .collect()
}
