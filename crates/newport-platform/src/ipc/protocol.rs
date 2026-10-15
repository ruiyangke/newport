//! Private desktop transport shared by the native backend and client.
//! A frame is a big-endian u32 byte length followed by one MessagePack value.
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const VERSION: u32 = 2;
pub const MAX_REQUEST: usize = 1024 * 1024;
pub const MAX_RESPONSE: usize = 32 * 1024 * 1024;
/// Successful open keeps this socket alive as the PTY's ownership lease.
/// EOF releases the PTY; terminal data still uses the ordinary read/write RPCs.
pub const TERMINAL_OPEN_LEASE: &str = "$terminal_open_lease";
/// Raise the existing application window without creating another profile owner.
pub const ACTIVATE_WINDOW: &str = "native.activate";
/// Admission priority only; callers still require normal connection validation.
/// Only explicit off requests may use capacity reserved for revocation.
pub fn is_integration_revocation(method: &str, args: &Value) -> bool {
    args["enabled"] == false
        && (method == "set_clipboard_enabled"
            || method == "set_integration_enabled"
                && matches!(args["feature"].as_str(), Some("clipboard" | "browser")))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub method: String,
    #[serde(default)]
    pub args: Value,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub result: Value,
    pub error: Option<Value>,
}

/// A local consent wait is not network inactivity. Intermediate frames keep
/// the request alive without dispatching it again or changing its result.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentApproval {
    pub version: u32,
    pub awaiting_agent: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ResponseFrame {
    AgentApproval(AgentApproval),
    Complete(Response),
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_explicit_known_integration_revocations_receive_reserved_admission() {
        for method in ["set_integration_enabled", "set_clipboard_enabled"] {
            for feature in ["browser", "clipboard"] {
                assert!(is_integration_revocation(
                    method,
                    &json!({"feature":feature,"enabled":false})
                ));
                for enabled in [json!(true), json!(0), json!("false"), Value::Null] {
                    assert!(!is_integration_revocation(
                        method,
                        &json!({"feature":feature,"enabled":enabled})
                    ));
                }
            }
        }
        assert!(!is_integration_revocation(
            "set_integration_enabled",
            &json!({"feature":"unknown","enabled":false})
        ));
        assert!(!is_integration_revocation(
            "set_integration_enabled",
            &json!({"enabled":false})
        ));
        assert!(!is_integration_revocation(
            "reinstall_agent",
            &json!({"enabled":false})
        ));
    }

    #[test]
    fn consent_progress_is_distinct_from_a_completed_result() {
        for awaiting_agent in [false, true] {
            let bytes = rmp_serde::to_vec_named(&AgentApproval {
                version: VERSION,
                awaiting_agent,
            })
            .unwrap();
            assert!(
                matches!(rmp_serde::from_slice::<ResponseFrame>(&bytes).unwrap(),
                ResponseFrame::AgentApproval(progress) if progress.awaiting_agent == awaiting_agent)
            );
        }
        let ambiguous = rmp_serde::to_vec_named(
            &json!({"version":VERSION,"awaiting_agent":true,"result":null,"error":null}),
        )
        .unwrap();
        assert!(rmp_serde::from_slice::<ResponseFrame>(&ambiguous).is_err());
    }

    #[test]
    fn request_round_trip_preserves_structured_git_arguments() {
        let args = json!({"serverId":"server","request":{"method":"repo.history","params":{"repoId":"repo","cursor":"opaque","pageSize":100}}});
        let request = Request {
            version: VERSION,
            method: "git_request".into(),
            args: args.clone(),
        };
        let bytes = rmp_serde::to_vec_named(&request).unwrap();
        let decoded: Request = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded.version, VERSION);
        assert_eq!(decoded.args, args);
    }
    #[test]
    fn structured_error_remains_distinct_from_success_null() {
        for error in [
            None,
            Some(json!({"code":"OUTCOME_UNKNOWN","message":"Check saved outcome"})),
        ] {
            let response = Response {
                version: VERSION,
                result: Value::Null,
                error: error.clone(),
            };
            let bytes = rmp_serde::to_vec_named(&response).unwrap();
            let decoded: Response = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(decoded.error, error);
        }
    }
    #[test]
    fn unknown_envelope_fields_are_rejected() {
        let data = rmp_serde::to_vec_named(
            &json!({"version":1,"method":"snapshot","args":{},"unexpected":true}),
        )
        .unwrap();
        assert!(rmp_serde::from_slice::<Request>(&data).is_err());
    }
}
