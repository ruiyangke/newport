//! Snapshot-bound Git page assembly, independent of the rendering framework.
use serde_json::Value;

const INVALID: &str = "Invalid repository page. Refresh to retry.";
const CHANGED: &str = "The repository listing changed. Refresh before loading more.";
// Matches the desktop Git response decoder. Accumulated listings may be larger.
const MAX_PAGE_ENTRIES: usize = 200;

pub(crate) fn is_page_method(method: &str) -> bool {
    matches!(
        method,
        "repo.status"
            | "repo.history"
            | "repo.commit_files"
            | "repo.branches"
            | "repo.remote_names"
            | "repo.remote_refs"
            | "repo.stashes"
            | "repo.tags"
            | "repo.worktrees"
            | "repo.blob_page"
            | "repo.diff_page"
            | "repo.commit_diff_page"
    )
}

/// Validate an individual wire response before storing or presenting it. The
/// same envelope applies to first and continuation pages, not assembled lists.
pub fn validate(page: &Value) -> Result<(), String> {
    if Envelope::read(page)?.entries.len() > MAX_PAGE_ENTRIES {
        return Err(INVALID.into());
    }
    Ok(())
}

struct Envelope<'a> {
    snapshot: &'a str,
    entries: &'a [Value],
    next_cursor: Option<&'a str>,
    metadata: &'a Value,
}

impl<'a> Envelope<'a> {
    fn read(page: &'a Value) -> Result<Self, String> {
        let snapshot = page["snapshot"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(INVALID)?;
        let entries = page["entries"].as_array().ok_or(INVALID)?;
        // Missing and null are different: only an explicit null ends a listing.
        let next_cursor = match page.get("nextCursor") {
            Some(Value::Null) => None,
            Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.as_str()),
            _ => return Err(INVALID.into()),
        };
        let metadata = page
            .get("metadata")
            .filter(|m| m.is_object())
            .ok_or(INVALID)?;
        Ok(Self {
            snapshot,
            entries,
            next_cursor,
            metadata,
        })
    }
}

/// Check snapshot, metadata and cursor identity without assembling entries.
/// Entry-specific decoding remains the responsibility of the caller.
pub fn validate_continuation(
    previous: &Value,
    incoming: &Value,
    cursor: &str,
) -> Result<(), String> {
    let before = Envelope::read(previous)?;
    validate(incoming)?;
    let after = Envelope::read(incoming)?;
    if before.next_cursor != Some(cursor)
        || before.snapshot != after.snapshot
        || before.metadata != after.metadata
        || after.next_cursor == Some(cursor)
    {
        return Err(CHANGED.into());
    }
    Ok(())
}

/// Append a common listing. Diff consumers use git_diff_page::merge to check
/// fragment boundaries too. Errors leave the accepted page untouched.
pub fn merge(previous: &Value, mut incoming: Value, cursor: &str) -> Result<Value, String> {
    validate_continuation(previous, &incoming, cursor)?;
    let before = Envelope::read(previous)?;
    let after = Envelope::read(&incoming)?;
    let mut entries = Vec::with_capacity(before.entries.len() + after.entries.len());
    entries.extend_from_slice(before.entries);
    entries.append(
        incoming["entries"]
            .as_array_mut()
            .expect("validated entries"),
    );
    incoming["entries"] = Value::Array(entries);
    Ok(incoming)
}

#[cfg(test)]
mod tests {
    use super::merge;
    use serde_json::{Value, json};

    fn page(entries: Value, next: Option<&str>) -> Value {
        json!({"snapshot":"s","metadata":{"comparison":"one"},"entries":entries,"nextCursor":next})
    }

    #[test]
    fn appends_in_order_and_keeps_the_incoming_envelope() {
        let first = page(json!([{"id":"one"}]), Some("next"));
        let mut last = page(json!([{"id":"two"}]), None);
        last["extension"] = json!("preserved");
        let merged = merge(&first, last, "next").unwrap();
        assert_eq!(merged["entries"], json!([{"id":"one"},{"id":"two"}]));
        assert!(merged["nextCursor"].is_null());
        assert_eq!(merged["extension"], "preserved");
        assert_eq!(first["entries"], json!([{"id":"one"}]));
    }

    #[test]
    fn rejects_changed_comparisons_wrong_requests_and_nonadvancing_cursors() {
        let first = page(json!([1]), Some("next"));
        let last = page(json!([2]), None);
        assert!(merge(&first, last.clone(), "wrong").is_err());
        assert!(merge(&first, first.clone(), "next").is_err());
        for (field, value) in [
            ("snapshot", json!("new")),
            ("metadata", json!({"comparison":"two"})),
        ] {
            let mut changed = last.clone();
            changed[field] = value;
            assert!(merge(&first, changed, "next").is_err());
        }
        let finished = page(json!([1]), None);
        assert!(merge(&finished, last, "next").is_err());
        assert_eq!(first, page(json!([1]), Some("next")));
    }

    #[test]
    fn malformed_pages_cannot_erase_rows_or_masquerade_as_the_end() {
        let first = page(json!([1]), Some("next"));
        let last = page(json!([2]), None);
        for (field, values) in [
            ("snapshot", vec![Value::Null, json!(17), json!("")]),
            ("entries", vec![Value::Null, json!({}), json!("rows")]),
            ("metadata", vec![Value::Null, json!([]), json!("metadata")]),
            (
                "nextCursor",
                vec![json!(false), json!(17), json!(""), json!({})],
            ),
        ] {
            let mut missing = last.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(merge(&first, missing, "next").is_err(), "missing {field}");
            let mut missing = first.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                merge(&missing, last.clone(), "next").is_err(),
                "missing previous {field}"
            );
            for value in values {
                let mut invalid = last.clone();
                invalid[field] = value.clone();
                assert!(
                    merge(&first, invalid, "next").is_err(),
                    "incoming {field}: {value}"
                );
                let mut invalid = first.clone();
                invalid[field] = value;
                assert!(
                    merge(&invalid, last.clone(), "next").is_err(),
                    "previous {field}"
                );
            }
        }
    }

    #[test]
    fn bounds_each_response_without_limiting_accumulated_rows() {
        let first = page(json!((0..600).collect::<Vec<_>>()), Some("next"));
        let last = page(json!((600..800).collect::<Vec<_>>()), None);
        assert_eq!(
            merge(&first, last, "next").unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            800
        );
        let oversized = page(json!((600..801).collect::<Vec<_>>()), None);
        assert!(merge(&first, oversized, "next").is_err());
    }

    #[test]
    fn empty_terminal_pages_and_fragmented_diff_entries_are_not_deduplicated() {
        let first = page(json!([{"path":"file","hunks":[1]}]), Some("next"));
        let continuation = page(json!([{"path":"file","hunks":[2]}]), Some("last"));
        let merged = merge(&first, continuation, "next").unwrap();
        let complete = merge(&merged, page(json!([]), None), "last").unwrap();
        assert_eq!(
            complete["entries"],
            json!([{"path":"file","hunks":[1]},{"path":"file","hunks":[2]}])
        );
        assert!(complete["nextCursor"].is_null());
    }
}
