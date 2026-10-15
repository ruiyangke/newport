//! Bounded remote advertisements retained by a single remote/filter read session.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::collections::HashSet;

pub const PAGE_SIZE: usize = 50;
const INVALID: &str = "The remote returned an invalid reference listing. Refresh to retry.";
const CHANGED: &str = "The remote reference listing changed. Refresh to retry.";

#[derive(Default)]
pub struct Listing {
    page: Value,
    references: HashSet<String>,
    cursors: HashSet<String>,
}

fn bytes(value: &Value) -> Option<&str> {
    value["display"].as_str()?;
    let encoded = value["bytesB64"].as_str()?;
    let decoded = STANDARD.decode(encoded).ok()?;
    (!decoded.is_empty()).then_some(encoded)
}

fn reference(entry: &Value) -> Option<&str> {
    let key = bytes(&entry["reference"])?;
    if !matches!(
        entry["kind"].as_str()?,
        "head" | "branch" | "tag" | "peeled_tag" | "other"
    ) {
        return None;
    }
    let oid = entry["oid"]["hex"].as_str()?;
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if !entry["symbolicTarget"].is_null() {
        bytes(&entry["symbolicTarget"])?;
    }
    Some(key)
}

impl Listing {
    pub fn page(&self) -> &Value {
        &self.page
    }

    /// Failure leaves all accepted rows and the continuation unchanged. The UI
    /// stops auto-loading on the returned error and offers an explicit refresh.
    pub fn accept(
        &mut self,
        mut incoming: Value,
        remote: &Value,
        cursor: Option<&str>,
    ) -> Result<(), String> {
        let metadata = &incoming["metadata"];
        let entries = incoming["entries"].as_array().ok_or(INVALID)?;
        let snapshot = incoming["snapshot"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(INVALID)?;
        let next = match &incoming["nextCursor"] {
            Value::Null => None,
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => return Err(INVALID.into()),
        };
        if remote["name"].as_str().is_none_or(str::is_empty)
            || remote["token"].as_str().is_none_or(str::is_empty)
            || metadata["remote"] != remote["name"]
            || metadata["remoteToken"] != remote["token"]
            || metadata["basis"] != "remote_advertisement"
            || metadata["forPush"] != true
            || !metadata["truncated"].is_boolean()
            || entries.len() > PAGE_SIZE
            || (entries.is_empty() && (next.is_some() || cursor.is_some()))
        {
            return Err(INVALID.into());
        }
        let mut keys = HashSet::with_capacity(entries.len());
        for entry in entries {
            let key = reference(entry).ok_or(INVALID)?;
            if !keys.insert(key.to_owned()) || (cursor.is_some() && self.references.contains(key)) {
                return Err(INVALID.into());
            }
        }
        if let Some(cursor) = cursor
            && (self.page["nextCursor"] != cursor
                || self.page["snapshot"] != snapshot
                || self.page["metadata"] != *metadata
                || self.cursors.contains(cursor)
                || next.as_deref() == Some(cursor)
                || next
                    .as_ref()
                    .is_some_and(|next| self.cursors.contains(next)))
        {
            return Err(CHANGED.into());
        }
        // Older agents may omit totals. If present, a bounded page must agree
        // with them; a truncated advertisement never claims completeness.
        if let Some(total) = metadata.get("totalEntries") {
            let total = total.as_u64().ok_or(INVALID)?;
            let loaded = keys.len()
                + if cursor.is_some() {
                    self.references.len()
                } else {
                    0
                };
            if loaded as u64 > total
                || (next.is_some() && loaded as u64 >= total)
                || (next.is_none() && metadata["truncated"] == false && loaded as u64 != total)
            {
                return Err(INVALID.into());
            }
        }
        if let Some(cursor) = cursor {
            let Value::Array(mut previous) = self.page["entries"].take() else {
                unreachable!("a continuation requires an accepted first page");
            };
            previous.append(
                incoming["entries"]
                    .as_array_mut()
                    .expect("validated entries"),
            );
            incoming["entries"] = Value::Array(previous);
            self.cursors.insert(cursor.to_owned());
            self.references.extend(keys);
        } else {
            self.references = keys;
            self.cursors.clear();
        }
        self.page = incoming;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn remote() -> Value {
        json!({"name":"team/upstream","token":"config"})
    }
    fn page(names: &[&str], next: Option<&str>) -> Value {
        json!({"snapshot":"s", "entries":names.iter().map(|name|json!({
            "reference":{"display":name,"bytesB64":STANDARD.encode(name)},
            "kind":"branch","oid":{"hex":"a".repeat(40)},"symbolicTarget":null
        })).collect::<Vec<_>>(),"nextCursor":next,"metadata":{
            "remote":"team/upstream","remoteToken":"config","forPush":true,
            "basis":"remote_advertisement","truncated":false
        }})
    }
    #[test]
    fn appends_only_the_requested_snapshot_and_refresh_starts_new_identity() {
        let mut list = Listing::default();
        list.accept(page(&["main"], Some("a")), &remote(), None)
            .unwrap();
        let before = list.page().clone();
        for field in ["snapshot", "metadata"] {
            let mut invalid = page(&["topic"], None);
            invalid[field] = json!("changed");
            assert!(list.accept(invalid, &remote(), Some("a")).is_err());
            assert_eq!(list.page(), &before);
        }
        list.accept(page(&["topic"], None), &remote(), Some("a"))
            .unwrap();
        assert_eq!(list.page()["entries"].as_array().unwrap().len(), 2);
        list.accept(page(&["main"], Some("a")), &remote(), None)
            .unwrap();
        list.accept(page(&["topic"], None), &remote(), Some("a"))
            .unwrap();
    }
    #[test]
    fn duplicate_rows_and_cursor_cycles_cannot_drain_pagination() {
        let mut list = Listing::default();
        assert!(
            list.accept(page(&["main", "main"], None), &remote(), None)
                .is_err()
        );
        list.accept(page(&["main"], Some("a")), &remote(), None)
            .unwrap();
        assert!(
            list.accept(page(&["main"], Some("b")), &remote(), Some("a"))
                .is_err()
        );
        assert!(
            list.accept(page(&[], Some("b")), &remote(), Some("a"))
                .is_err()
        );
        assert!(
            list.accept(page(&["topic"], Some("a")), &remote(), Some("a"))
                .is_err()
        );
        list.accept(page(&["topic"], Some("b")), &remote(), Some("a"))
            .unwrap();
        let before = list.page().clone();
        assert!(
            list.accept(page(&["third"], Some("a")), &remote(), Some("b"))
                .is_err()
        );
        assert_eq!(list.page(), &before);
    }
    #[test]
    fn scope_shape_and_advertisement_limits_are_checked() {
        let valid = page(&["main"], None);
        let mut cases = Vec::new();
        for (path, replacement) in [
            ("/snapshot", json!("")),
            ("/metadata/remoteToken", json!("other")),
            ("/metadata/remote", json!("origin")),
            ("/metadata/forPush", json!(false)),
            ("/metadata/basis", json!("stored")),
            ("/metadata/truncated", json!(null)),
            ("/entries/0/reference/bytesB64", json!("!")),
            ("/entries/0/oid/hex", json!("abc")),
            ("/entries/0/kind", json!("unknown")),
            ("/entries/0/symbolicTarget", json!({})),
            ("/nextCursor", json!("")),
        ] {
            let mut bad = valid.clone();
            *bad.pointer_mut(path).unwrap() = replacement;
            cases.push(bad);
        }
        let names: Vec<_> = (0..=PAGE_SIZE).map(|i| format!("branch-{i}")).collect();
        cases.push(page(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
            None,
        ));
        for bad in cases {
            let mut list = Listing::default();
            assert!(list.accept(bad.clone(), &remote(), None).is_err(), "{bad}");
            assert!(list.page().is_null());
        }
    }
    #[test]
    fn totals_and_truncation_do_not_falsely_claim_completion() {
        let mut list = Listing::default();
        let mut data = page(&["main"], None);
        data["metadata"]["totalEntries"] = json!(2);
        assert!(list.accept(data.clone(), &remote(), None).is_err());
        data["metadata"]["truncated"] = json!(true);
        list.accept(data, &remote(), None).unwrap();
        let mut complete = page(&[], None);
        complete["metadata"]["totalEntries"] = json!(0);
        list.accept(complete, &remote(), None).unwrap();
    }
    #[test]
    fn display_collisions_do_not_merge_different_raw_references() {
        let mut data = page(&["refs/heads/�", "refs/heads/�"], None);
        data["entries"][0]["reference"]["bytesB64"] = json!(STANDARD.encode(b"refs/heads/\xff"));
        data["entries"][1]["reference"]["bytesB64"] = json!(STANDARD.encode(b"refs/heads/\xfe"));
        Listing::default().accept(data, &remote(), None).unwrap();
    }
}
