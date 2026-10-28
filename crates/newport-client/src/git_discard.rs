//! Immutable discard targets; execution keeps the originally reviewed snapshot.
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Index,
    Head,
}
#[derive(Clone, Debug)]
pub struct Review {
    snapshot: String,
    entry: String,
    index: bool,
    hunks: Option<Value>,
}
impl Review {
    pub fn file(status: &Value, entry: &Value) -> Result<Self, String> {
        let invalid =
            || "Review an individual, non-conflicted file before discarding changes.".to_owned();
        let snapshot = status["snapshot"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        let id = entry["entryId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        let entries = status["entries"].as_array().ok_or_else(invalid)?;
        let known = entries
            .iter()
            .find(|row| row["entryId"] == id)
            .ok_or_else(invalid)?;
        if known != entry
            || !status["metadata"]["integration"].is_null()
            || entries.iter().any(|row| row["conflicted"] == true)
            || (entry["untracked"] == true
                && entry["path"]["display"]
                    .as_str()
                    .is_none_or(|p| p.ends_with('/')))
            || !(entry["staged"] == true || entry["unstaged"] == true || entry["untracked"] == true)
        {
            return Err(invalid());
        }
        Ok(Self {
            snapshot: snapshot.into(),
            entry: id.into(),
            index: entry["unstaged"] == true || entry["untracked"] == true,
            hunks: None,
        })
    }
    pub fn selection(
        review: &crate::git_selection::Review,
        hunks: &[String],
        lines: Option<&[String]>,
    ) -> Result<Self, String> {
        if review.staged() {
            return Err("Only unstaged changes can be partially discarded.".into());
        }
        let action = review.action(hunks, lines)?;
        Ok(Self {
            snapshot: review.snapshot().into(),
            entry: action["entryIds"][0].as_str().unwrap().into(),
            index: true,
            hunks: Some(action["hunks"].clone()),
        })
    }
    pub fn snapshot(&self) -> &str {
        &self.snapshot
    }
    pub fn matches(&self, snapshot: &str, entry: &str) -> bool {
        self.snapshot == snapshot && self.entry == entry
    }
    pub fn supports_index(&self) -> bool {
        self.index
    }
    pub fn partial(&self) -> bool {
        self.hunks.is_some()
    }
    pub fn line_count(&self) -> Option<usize> {
        self.hunks.as_ref()?.get("lines")?.as_array().map(Vec::len)
    }
    pub fn action(&self, source: Source) -> Result<Value, String> {
        if (source == Source::Index && !self.index) || (source == Source::Head && self.partial()) {
            return Err("The restore source does not match the reviewed changes.".into());
        }
        let mut action = json!({"kind":"discard", "entryIds":[self.entry], "source": if source == Source::Index {"index"} else {"head"}});
        if let Some(hunks) = &self.hunks {
            action["hunks"] = hunks.clone();
        }
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_file_uses_reviewed_entry_snapshot_and_explicit_restore_source() {
        let entry =
            json!({"entryId":"e", "staged":true,"unstaged":false,"path":{"display":"file"}});
        let mut status = json!({"snapshot":"s","entries":[entry]});
        let review = Review::file(&status, &entry).unwrap();
        assert!(review.matches("s", "e"));
        assert!(!review.matches("new", "e"));
        assert!(!review.matches("s", "other"));
        assert_eq!(
            review.action(Source::Head).unwrap(),
            json!({"kind":"discard","entryIds":["e"],"source":"head"})
        );
        assert!(review.action(Source::Index).is_err());
        status["entries"][0]["unstaged"] = json!(true);
        assert!(Review::file(&status, &entry).is_err());
        let review = Review::file(&status, &status["entries"][0]).unwrap();
        assert_eq!(review.action(Source::Index).unwrap()["source"], "index");
        status["metadata"]["integration"] = json!({"kind":"merge"});
        assert!(Review::file(&status, &status["entries"][0]).is_err());
        status["metadata"]["integration"] = Value::Null;
        status["entries"]
            .as_array_mut()
            .unwrap()
            .push(json!({"conflicted":true}));
        assert!(Review::file(&status, &status["entries"][0]).is_err());
    }
    #[test]
    fn collapsed_untracked_directories_are_not_discard_targets() {
        let entry = json!({"entryId":"e","untracked":true,"path":{"display":"directory/"}});
        let status = json!({"snapshot":"s","entries":[entry]});
        assert!(Review::file(&status, &entry).is_err());
    }
    #[test]
    fn partial_discard_preserves_ids_and_rejects_staged_or_head_comparisons() {
        let mut page = json!({"nextCursor":null,"metadata":{"readOnly":false,"totalFiles":1,"sourceSnapshot":"s","entryId":"e","side":"index_to_worktree","contextLines":3},"entries":[{"fileIndex":0,"status":"Modified","oldMode":33188,"newMode":33188,"oldPath":{"bytesB64":"Zg=="},"newPath":{"bytesB64":"Zg=="},"hunks":[{"id":"h","lines":[{"id":"l","origin":"+","lineComplete":true}]}]}]});
        let selection = crate::git_selection::Review::from_page(&page).unwrap();
        let review = Review::selection(&selection, &["h".into()], Some(&["l".into()])).unwrap();
        assert_eq!(review.line_count(), Some(1));
        assert_eq!(
            review.action(Source::Index).unwrap(),
            json!({"kind":"discard","entryIds":["e"],"source":"index","hunks":{"ids":["h"],"lines":["l"],"contextLines":3}})
        );
        assert!(review.action(Source::Head).is_err());
        assert!(Review::selection(&selection, &["invalid".into()], None).is_err());
        page["metadata"]["side"] = json!("head_to_index");
        let staged = crate::git_selection::Review::from_page(&page).unwrap();
        assert!(Review::selection(&staged, &["h".into()], None).is_err());
    }
}
