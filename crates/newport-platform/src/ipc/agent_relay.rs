//! Narrow native-process SSH agent relay contract. No socket paths or agent
//! mutation operations cross the desktop bridge.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const REGISTER: &str = "$agent.register";
pub const ATTACH: &str = "$agent.attach";
pub const MAX_CONTROL_FRAME: usize = 4096;
pub const MAX_AGENT_FRAME: usize = 256 * 1024;
pub const MAX_CONNECTIONS: usize = 16;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    System,
    OnePassword,
}
impl Source {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("system") {
            "system" => Ok(Self::System),
            "onePassword" => Ok(Self::OnePassword),
            _ => Err("Unknown SSH agent".into()),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Open {
    pub ticket: Uuid,
    pub source: Source,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attach {
    pub ticket: Uuid,
    pub error: Option<String>,
}

/// Agent request filtering is deliberately independent of any SSH library.
/// A sign request contains a public-key blob, signing data, and flags. Other
/// operations (key addition/removal, lock/unlock, extensions) are not relayed.
pub fn request_allowed(packet: &[u8]) -> bool {
    if packet == [11] {
        return true;
    }
    if packet.first() != Some(&13) || packet.len() > MAX_AGENT_FRAME {
        return false;
    }
    let mut rest = &packet[1..];
    for _ in 0..2 {
        let Some(prefix) = rest.get(..4) else {
            return false;
        };
        let length = u32::from_be_bytes(prefix.try_into().unwrap()) as usize;
        let Some(value) = rest.get(4..).and_then(|tail| tail.get(length..)) else {
            return false;
        };
        rest = value;
    }
    rest.len() == 4
}

pub fn response_allowed(request: u8, packet: &[u8]) -> bool {
    !packet.is_empty()
        && packet.len() <= MAX_AGENT_FRAME
        && (packet == [5] || matches!((request, packet[0]), (11, 12) | (13, 14)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_only_identity_and_well_formed_sign_requests() {
        assert!(request_allowed(&[11]));
        assert!(request_allowed(&[
            13, 0, 0, 0, 1, 42, 0, 0, 0, 1, 43, 0, 0, 0, 0
        ]));
        for packet in [
            vec![],
            vec![11, 0],
            vec![13],
            vec![17],
            vec![18],
            vec![19],
            vec![22],
            vec![23],
            vec![27],
            vec![13, 255, 255, 255, 255],
        ] {
            assert!(!request_allowed(&packet), "{packet:?}");
        }
    }

    #[test]
    fn responses_must_match_the_requested_operation() {
        assert!(response_allowed(11, &[12, 0, 0, 0, 0]));
        assert!(response_allowed(13, &[14, 0, 0, 0, 0]));
        assert!(response_allowed(13, &[5]));
        assert!(!response_allowed(11, &[14]));
        assert!(!response_allowed(13, &[12]));
        assert!(!response_allowed(13, &[6]));
    }
}
