//! The Newport account this computer is signed in to, sealed whole with the
//! master key: its credential and license token are secrets, and its trusted
//! time must not be set back.
use zeroize::{Zeroize, Zeroizing};

const SCHEMA_VERSION: u64 = 1;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(default = "schema_version")]
    schema_version: u64,
    /// Bearer credential for the desktop API. Never sent to the UI.
    pub credential: String,
    pub device_id: String,
    /// The API's license object, kept verbatim so newer fields survive.
    pub license: serde_json::Value,
    /// The latest time the account site vouched for, as UTC Unix seconds: a
    /// response's `Date` or a token's `iat`. Newport Remote's clock guard never
    /// lets this computer's clock run behind it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_time: Option<i64>,
}
fn schema_version() -> u64 {
    SCHEMA_VERSION
}
impl Record {
    pub fn new(credential: String, device_id: String, license: serde_json::Value) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            credential,
            device_id,
            license,
            trusted_time: None,
        }
    }
}
impl Drop for Record {
    fn drop(&mut self) {
        self.credential.zeroize();
        if let Some(serde_json::Value::String(token)) = self.license.get_mut("token") {
            token.zeroize();
        }
    }
}
impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// The sign-in saved as the profile's `account` secret.
pub(super) fn decode(data: &[u8]) -> Result<Record, String> {
    let record: Record =
        serde_json::from_slice(data).map_err(|_| "Invalid saved Newport sign-in.".to_string())?;
    if record.schema_version != SCHEMA_VERSION {
        return Err("Unsupported saved sign-in. Update Newport to use it.".into());
    }
    Ok(record)
}

pub(super) fn encode(record: &Record) -> Result<Zeroizing<Vec<u8>>, String> {
    serde_json::to_vec(record)
        .map(Zeroizing::new)
        .map_err(|_| "Cannot encode the Newport sign-in.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_round_trips_and_only_its_schema_is_read() {
        let record = Record::new(
            "credential".into(),
            "device".into(),
            serde_json::json!({"state":"trial","email":"a@example.com","token":"v4.public.x","future":1}),
        );
        assert_eq!(decode(&encode(&record).unwrap()).unwrap(), record);
        let mut other: serde_json::Value =
            serde_json::from_slice(&encode(&record).unwrap()).unwrap();
        other["schemaVersion"] = serde_json::json!(2);
        assert!(decode(&serde_json::to_vec(&other).unwrap()).is_err());
        assert!(decode(b"damaged").is_err());
    }
}
