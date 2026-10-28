//! Partial index changes refer to server-generated IDs, never client patch text.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Review {
    snapshot: String,
    entry: String,
    staged: bool,
    context: u64,
    hunks: BTreeMap<String, BTreeSet<String>>,
    deletions: BTreeSet<String>,
    complete_hunks: BTreeSet<String>,
}
impl Review {
    pub fn from_page(page: &Value) -> Result<Self, String> {
        let unavailable = || {
            "Individual hunks are unavailable for this comparison. Use the file actions.".to_owned()
        };
        let has_more = match &page["nextCursor"] {
            Value::Null => false,
            Value::String(cursor) if !cursor.is_empty() => true,
            _ => return Err(unavailable()),
        };
        let meta = &page["metadata"];
        if meta["readOnly"] != false || meta["truncated"] == true || meta["totalFiles"] != 1 {
            return Err(unavailable());
        }
        let snapshot = meta["sourceSnapshot"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(unavailable)?;
        let entry = meta["entryId"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(unavailable)?;
        let staged = match meta["side"].as_str() {
            Some("head_to_index") => true,
            Some("index_to_worktree") => false,
            _ => return Err(unavailable()),
        };
        let context = meta["contextLines"]
            .as_u64()
            .filter(|v| *v <= 25)
            .ok_or_else(unavailable)?;
        let files = page["entries"]
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(unavailable)?;
        let mut hunks: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut deletions = BTreeSet::new();
        let mut totals = BTreeMap::new();
        let mut completed: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
        for file in files {
            // Accumulated pages contain repeated fragments of the same file/hunk.
            let text_mode = |mode: &Value| matches!(mode.as_u64(), Some(0o100644 | 0o100755));
            let modes = match file["status"].as_str() {
                Some("Modified") => {
                    file["oldMode"] == file["newMode"] && text_mode(&file["oldMode"])
                }
                Some("Added" | "Untracked") => text_mode(&file["newMode"]),
                Some("Deleted") => text_mode(&file["oldMode"]),
                _ => false,
            };
            if file["fileIndex"] != 0
                || file["binary"] == true
                || !file["omissionReason"].is_null()
                || !modes
                || !file["oldPath"]["bytesB64"].is_string()
                || file["oldPath"]["bytesB64"] != file["newPath"]["bytesB64"]
            {
                return Err(unavailable());
            }
            for hunk in file["hunks"].as_array().ok_or_else(unavailable)? {
                let id = hunk["id"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(unavailable)?;
                let lines = hunks.entry(id.to_owned()).or_default();
                if let Some(total) = hunk.get("totalLines") {
                    let total = total.as_u64().filter(|n| *n > 0).ok_or_else(unavailable)?;
                    if totals
                        .insert(id.to_owned(), total)
                        .is_some_and(|old| old != total)
                    {
                        return Err(unavailable());
                    }
                }
                for line in hunk["lines"].as_array().ok_or_else(unavailable)? {
                    if line["lineComplete"] == true
                        && let Some(index) = line["lineIndex"].as_u64()
                    {
                        completed.entry(id.to_owned()).or_default().insert(index);
                    }
                    if matches!(line["origin"].as_str(), Some("+" | "-"))
                        && line["lineComplete"] == true
                    {
                        let id = line["id"]
                            .as_str()
                            .filter(|v| !v.is_empty())
                            .ok_or_else(unavailable)?;
                        lines.insert(id.to_owned());
                        if line["origin"] == "-" {
                            deletions.insert(id.to_owned());
                        }
                    }
                }
            }
        }
        // Context-only prefixes and unfinished lines must not disable earlier
        // selectable lines. Whole-hunk actions require every logical line,
        // including context and EOF notes, across all loaded fragments.
        hunks.retain(|_, lines| !lines.is_empty());
        if hunks.is_empty() {
            return Err(unavailable());
        }
        let complete_hunks = hunks
            .keys()
            .filter(|id| {
                match totals.get(*id) {
                    Some(total) => completed.get(*id).is_some_and(|indices| {
                        indices.len() as u64 == *total
                            && indices.first() == Some(&0)
                            && indices.last() == Some(&(total - 1))
                    }),
                    // Older non-paginated responses have no per-hunk totals.
                    None => !has_more,
                }
            })
            .cloned()
            .collect();
        Ok(Self {
            snapshot: snapshot.into(),
            entry: entry.into(),
            staged,
            context,
            hunks,
            deletions,
            complete_hunks,
        })
    }
    pub fn snapshot(&self) -> &str {
        &self.snapshot
    }
    pub fn staged(&self) -> bool {
        self.staged
    }
    pub fn matches(&self, snapshot: &str, entry: &str, staged: bool, context: u8) -> bool {
        self.snapshot == snapshot
            && self.entry == entry
            && self.staged == staged
            && self.context == u64::from(context)
    }
    pub fn contains_hunk(&self, id: &str) -> bool {
        self.hunks.contains_key(id)
    }
    pub fn hunk_complete(&self, id: &str) -> bool {
        self.complete_hunks.contains(id)
    }
    pub fn contains_line(&self, hunk: &str, line: &str) -> bool {
        self.hunks
            .get(hunk)
            .is_some_and(|lines| lines.contains(line))
    }
    /// A partially selected run becomes fully selected; a full run is cleared.
    /// Validate the entire group before touching selection.
    pub fn toggle_lines(
        &self,
        selected: &mut BTreeSet<String>,
        lines: &[String],
    ) -> Result<(), String> {
        self.hunks_for_lines(lines)?;
        let clear = lines.iter().all(|line| selected.contains(line));
        for line in lines {
            if clear {
                selected.remove(line);
            } else {
                selected.insert(line.clone());
            }
        }
        Ok(())
    }
    /// Selecting an addition alone does not imply selecting its replacement's deletion.
    pub fn retains_deletions(&self, selected: &BTreeSet<String>) -> bool {
        self.hunks.values().any(|lines| {
            lines
                .iter()
                .any(|line| selected.contains(line) && !self.deletions.contains(line))
                && lines
                    .iter()
                    .any(|line| !selected.contains(line) && self.deletions.contains(line))
        })
    }
    /// Resolve selections against this review, never against a fresh diff.
    pub fn hunks_for_lines(&self, lines: &[String]) -> Result<Vec<String>, String> {
        let hunks: Vec<_> = self
            .hunks
            .iter()
            .filter(|(_, known)| lines.iter().any(|line| known.contains(line)))
            .map(|(hunk, _)| hunk.clone())
            .collect();
        self.action(&hunks, Some(lines))?;
        Ok(hunks)
    }
    pub fn action(&self, ids: &[String], lines: Option<&[String]>) -> Result<Value, String> {
        let invalid =
            || "Select complete hunks or changed lines from the reviewed comparison.".to_owned();
        let unique: BTreeSet<_> = ids.iter().collect();
        if ids.is_empty()
            || ids.len() > 1024
            || unique.len() != ids.len()
            || ids.iter().any(|id| !self.hunks.contains_key(id))
            || (lines.is_none() && ids.iter().any(|id| !self.hunk_complete(id)))
        {
            return Err(invalid());
        }
        if let Some(lines) = lines {
            let unique: BTreeSet<_> = lines.iter().collect();
            if lines.is_empty()
                || unique.len() != lines.len()
                || lines
                    .iter()
                    .any(|line| !ids.iter().any(|id| self.hunks[id].contains(line)))
            {
                return Err(invalid());
            }
        }
        let mut action = json!({"kind":if self.staged {"unstage"} else {"stage"}, "entryIds":[self.entry],
            "hunks":{"ids":ids,"contextLines":self.context}});
        if let Some(lines) = lines {
            action["hunks"]["lines"] = json!(lines);
        }
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn page() -> Value {
        json!({"snapshot":"diff", "nextCursor":null, "metadata":{"readOnly":false,"totalFiles":1,"sourceSnapshot":"status","entryId":"entry","side":"index_to_worktree","contextLines":3},
            "entries":[{"fileIndex":0,"status":"Modified","oldMode":33188,"newMode":33188,"oldPath":{"bytesB64":"Zg=="},"newPath":{"bytesB64":"Zg=="},"hunks":[{"id":"h1","lines":[{"id":"l1","origin":"+","lineComplete":true}]}]}]})
    }
    #[test]
    fn loaded_lines_are_actionable_before_the_hunk_is_complete() {
        for side in ["index_to_worktree", "head_to_index"] {
            let mut page = page();
            page["nextCursor"] = json!("more");
            page["metadata"]["side"] = json!(side);
            page["entries"][0]["hunks"][0]["totalLines"] = json!(3);
            page["entries"][0]["hunks"][0]["lines"] = json!([
                {"lineIndex":0,"origin":" ","lineComplete":true},
                {"lineIndex":1,"id":"l1","origin":"+","lineComplete":true},
                {"lineIndex":2,"origin":"+","lineComplete":false}
            ]);
            let review = Review::from_page(&page).unwrap();
            assert!(review.contains_line("h1", "l1"));
            assert!(!review.hunk_complete("h1"));
            assert!(review.action(&["h1".into()], None).is_err());
            assert!(review.action(&["h1".into()], Some(&["l1".into()])).is_ok());
            assert!(
                review
                    .action(&["h1".into()], Some(&["unloaded".into()]))
                    .is_err()
            );
            let mut second = page["entries"][0].clone();
            second["hunks"][0]["lines"] = json!([
                {"lineIndex":2,"id":"l2","origin":"+","lineComplete":true}
            ]);
            // Other hunks remain unloaded; this hunk is now complete.
            page["entries"].as_array_mut().unwrap().push(second);
            let review = Review::from_page(&page).unwrap();
            assert!(review.hunk_complete("h1"));
            assert!(review.action(&["h1".into()], None).is_ok());
            assert!(
                review
                    .action(&["h1".into()], Some(&["l1".into(), "l2".into()]))
                    .is_ok()
            );
        }
    }
    #[test]
    fn context_only_tail_does_not_block_earlier_hunks() {
        let mut page = page();
        page["nextCursor"] = json!("more");
        page["entries"][0]["hunks"][0]["totalLines"] = json!(1);
        page["entries"][0]["hunks"][0]["lines"][0]["lineIndex"] = json!(0);
        page["entries"][0]["hunks"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id":"h2","totalLines":5,"lines":[{"lineIndex":0,"origin":" ","lineComplete":true}]
            }));
        let review = Review::from_page(&page).unwrap();
        assert!(review.action(&["h1".into()], None).is_ok());
        assert!(!review.contains_hunk("h2"));
    }
    #[test]
    fn missing_lines_and_eof_notes_do_not_make_a_complete_hunk() {
        let mut page = page();
        page["entries"][0]["hunks"][0]["totalLines"] = json!(3);
        page["entries"][0]["hunks"][0]["lines"] = json!([
            {"lineIndex":0,"id":"l1","origin":"+","lineComplete":true},
            {"lineIndex":2,"origin":">","lineComplete":true}
        ]);
        assert!(!Review::from_page(&page).unwrap().hunk_complete("h1"));
        page["nextCursor"] = json!("more");
        page["entries"][0]["hunks"][0]
            .as_object_mut()
            .unwrap()
            .remove("totalLines");
        assert!(!Review::from_page(&page).unwrap().hunk_complete("h1"));
        for cursor in [json!(false), json!("")] {
            page["nextCursor"] = cursor;
            assert!(Review::from_page(&page).is_err());
        }
    }
    #[test]
    fn preserves_snapshot_context_side_and_opaque_ids() {
        let mut page = page();
        for staged in [false, true] {
            page["metadata"]["side"] = json!(if staged {
                "head_to_index"
            } else {
                "index_to_worktree"
            });
            let review = Review::from_page(&page).unwrap();
            assert!(review.matches("status", "entry", staged, 3));
            assert!(!review.matches("newer", "entry", staged, 3));
            assert!(!review.matches("status", "other", staged, 3));
            assert!(!review.matches("status", "entry", !staged, 3));
            assert!(!review.matches("status", "entry", staged, 0));
            assert_eq!(
                review.action(&["h1".into()], None).unwrap(),
                json!({"kind":if staged {"unstage"} else {"stage"},"entryIds":["entry"],"hunks":{"ids":["h1"],"contextLines":3}})
            );
            assert_eq!(
                review.action(&["h1".into()], Some(&["l1".into()])).unwrap()["hunks"]["lines"],
                json!(["l1"])
            );
        }
    }
    #[test]
    fn rejects_incomplete_unsupported_and_invalid_selections() {
        let original = page();
        for pointer in [
            "/metadata/readOnly",
            "/metadata/totalFiles",
            "/metadata/side",
            "/entries/0/oldMode",
            "/entries/0/oldPath/bytesB64",
            "/entries/0/hunks/0/id",
        ] {
            let mut page = original.clone();
            *page.pointer_mut(pointer).unwrap() = if pointer.ends_with("/id") {
                json!("")
            } else {
                json!("invalid")
            };
            assert!(Review::from_page(&page).is_err(), "{pointer}");
        }
        for field in ["binary", "omissionReason"] {
            let mut page = original.clone();
            page["entries"][0][field] = json!(true);
            assert!(Review::from_page(&page).is_err());
        }
        let review = Review::from_page(&original).unwrap();
        for ids in [
            vec![],
            vec!["unknown".into()],
            vec!["h1".into(), "h1".into()],
        ] {
            assert!(review.action(&ids, None).is_err());
        }
        for lines in [
            vec![],
            vec!["unknown".into()],
            vec!["l1".into(), "l1".into()],
        ] {
            assert!(review.action(&["h1".into()], Some(&lines)).is_err());
        }
    }
    #[test]
    fn selected_lines_resolve_only_their_hunks_and_preserve_unselected_deletions() {
        let mut page = page();
        page["entries"][0]["hunks"] = json!([
            {"id":"h1","lines":[{"id":"add","origin":"+","lineComplete":true},
                {"id":"remove","origin":"-","lineComplete":true}]},
            {"id":"h2","lines":[{"id":"other","origin":"+","lineComplete":true}]}
        ]);
        let review = Review::from_page(&page).unwrap();
        assert_eq!(review.hunks_for_lines(&["other".into()]).unwrap(), ["h2"]);
        assert_eq!(
            review
                .hunks_for_lines(&["other".into(), "add".into()])
                .unwrap(),
            ["h1", "h2"]
        );
        assert!(review.hunks_for_lines(&["context".into()]).is_err());
        assert!(review.hunks_for_lines(&[]).is_err());
        assert!(
            review
                .hunks_for_lines(&["add".into(), "add".into()])
                .is_err()
        );
        assert!(review.contains_line("h1", "add"));
        assert!(!review.contains_line("h2", "add"));
        assert!(review.retains_deletions(&BTreeSet::from(["add".into()])));
        assert!(!review.retains_deletions(&BTreeSet::from(["other".into()])));
        assert!(!review.retains_deletions(&BTreeSet::from(["add".into(), "remove".into()])));
    }
    #[test]
    fn paginated_fragments_share_hunk_identity() {
        let mut page = page();
        let mut second = page["entries"][0].clone();
        second["hunks"][0]["lines"][0]["id"] = json!("l2");
        page["entries"].as_array_mut().unwrap().push(second);
        let review = Review::from_page(&page).unwrap();
        assert!(
            review
                .action(&["h1".into()], Some(&["l1".into(), "l2".into()]))
                .is_ok()
        );
    }
    #[test]
    fn toggling_groups_completes_mixed_selection_and_preserves_other_runs() {
        let mut page = page();
        page["entries"][0]["hunks"][0]["lines"] = json!([
            {"id":"a","origin":"-","lineComplete":true},
            {"id":"b","origin":"+","lineComplete":true},
            {"id":"other","origin":"+","lineComplete":true}
        ]);
        let review = Review::from_page(&page).unwrap();
        let mut selection = BTreeSet::from(["a".into(), "other".into()]);
        let group = ["a".into(), "b".into()];
        review.toggle_lines(&mut selection, &group).unwrap();
        assert_eq!(
            selection,
            BTreeSet::from(["a".into(), "b".into(), "other".into()])
        );
        review.toggle_lines(&mut selection, &group).unwrap();
        assert_eq!(selection, BTreeSet::from(["other".into()]));
        assert!(
            review
                .toggle_lines(&mut selection, &["a".into(), "unknown".into()])
                .is_err()
        );
        assert_eq!(selection, BTreeSet::from(["other".into()]));
    }
}
