//! Window invalidation is narrower than snapshot acceptance. Every snapshot is
//! retained and reconciled; only state consumed by this window requests a redraw.
use serde_json::Value;

// Server-keyed fields from manager::Runtime. Unknown fields remain conservative:
// adding a new backend field cannot silently suppress an existing UI update.
const SERVER_FIELDS: &[&str] = &[
    "agentTransport",
    "agentRuntime",
    "agentPermissions",
    "agentStatus",
    "agentInstallation",
    "agentOperations",
    "clipboard",
    "clipboardMessages",
    "clipboardPathNeeded",
    "health",
    "connectivity",
    "connectionRevisions",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    General,
    Setup,
    Integration,
}

impl Surface {
    fn watches(self, field: &str) -> bool {
        // Health is shared by the header; connection epochs invalidate the
        // current page's owner. The wizard displays installation progress.
        // Remaining agent state belongs to the Integration page. Accepting it
        // still updates permission checks and retained state on every page.
        matches!(field, "health" | "connectionRevisions")
            || self == Self::Integration
            || self == Self::Setup && field == "agentInstallation"
    }
}

pub(super) fn changed(previous: &Value, next: &Value, server: &str, surface: Surface) -> bool {
    let (Some(previous_fields), Some(next_fields)) = (previous.as_object(), next.as_object())
    else {
        return previous != next;
    };
    // Revision advances for any server's runtime observation. Configuration,
    // backend identity, load errors and unknown top-level fields still matter.
    let global = |key: &&String| !matches!(key.as_str(), "runtime" | "revision");
    if !previous_fields
        .iter()
        .filter(|(key, _)| global(key))
        .eq(next_fields.iter().filter(|(key, _)| global(key)))
    {
        return true;
    }
    let (previous, next) = (&previous["runtime"], &next["runtime"]);
    let (Some(previous_fields), Some(next_fields)) = (previous.as_object(), next.as_object())
    else {
        return previous != next;
    };
    // Tunnel state remains shared: the sidebar displays the global connected
    // count, and Connections also consumes tunnel health and action state.
    let global =
        |key: &&String| key.as_str() != "awaitingAgent" && !SERVER_FIELDS.contains(&key.as_str());
    if !previous_fields
        .iter()
        .filter(|(key, _)| global(key))
        .eq(next_fields.iter().filter(|(key, _)| global(key)))
    {
        return true;
    }
    if SERVER_FIELDS.iter().any(|key| {
        if !surface.watches(key) {
            return false;
        }
        let (previous, next) = (&previous[*key], &next[*key]);
        if (previous.is_null() || previous.is_object()) && (next.is_null() || next.is_object()) {
            previous[server] != next[server]
        } else {
            // Do not conceal a malformed or newly shaped protocol field.
            previous != next
        }
    }) {
        return true;
    }
    let awaiting = |runtime: &Value| {
        runtime["awaitingAgent"]
            .as_array()
            .is_some_and(|servers| servers.iter().any(|id| id == server))
    };
    awaiting(previous) != awaiting(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_known_runtime_owner_is_selected_independently() {
        let before = json!({"revision":1,"config":{},"runtime":{}});
        for key in SERVER_FIELDS {
            let mut next = before.clone();
            next["revision"] = json!(2);
            next["runtime"][*key]["other"] = json!({"changed":true});
            assert!(
                !changed(&before, &next, "selected", Surface::Integration),
                "{key}"
            );
            assert!(
                changed(&before, &next, "other", Surface::Integration),
                "{key}"
            );
            assert!(
                changed(&next, &before, "other", Surface::Integration),
                "removing {key}"
            );
        }
    }

    #[test]
    fn shared_state_and_unknown_fields_remain_observable() {
        let before = json!({"revision":1,"config":{},"runtime":{}});
        for (path, value) in [
            ("/instanceId", json!("new-backend")),
            ("/loadError", json!("cannot read profile")),
            ("/config", json!({"servers":[]})),
            ("/futureField", json!(true)),
            ("/runtime/tunnels", json!({"other":"connected"})),
            ("/runtime/tunnelHealth", json!({"other":"error"})),
            ("/runtime/futureField", json!({"other":true})),
        ] {
            let mut next = before.clone();
            let parts: Vec<_> = path.split('/').skip(1).collect();
            let mut target = &mut next;
            for part in parts {
                target = &mut target[part];
            }
            *target = value;
            assert!(
                changed(&before, &next, "selected", Surface::Integration),
                "{path}"
            );
        }
    }

    #[test]
    fn approval_wait_is_scoped_and_revision_alone_is_not_presentation() {
        let before = json!({"revision":1,"runtime":{"awaitingAgent":["other"]}});
        let mut next = before.clone();
        next["revision"] = json!(2);
        next["runtime"]["awaitingAgent"] = json!([]);
        assert!(!changed(&before, &next, "selected", Surface::Integration));
        assert!(changed(&before, &next, "other", Surface::Integration));
    }
}
