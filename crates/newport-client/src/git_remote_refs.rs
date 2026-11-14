//! Bounded remote advertisements retained by a single remote/filter read session.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_protocol::git::{Page, Path, Remote, RemoteReference, RemoteReferenceMetadata};
use std::collections::HashSet;

pub const PAGE_SIZE: usize = 50;
const INVALID: &str = "The remote returned an invalid reference listing. Refresh to retry.";
const CHANGED: &str = "The remote reference listing changed. Refresh to retry.";

/// One page of a remote's advertised references.
pub type ReferencePage = Page<RemoteReference, RemoteReferenceMetadata>;

#[derive(Default)]
pub struct Listing {
    page: Option<ReferencePage>,
    references: HashSet<String>,
    cursors: HashSet<String>,
}

/// The encoded bytes of a nonempty path.
fn bytes(path: &Path) -> Option<&str> {
    let decoded = STANDARD.decode(&path.bytes_b64).ok()?;
    (!decoded.is_empty()).then_some(path.bytes_b64.as_str())
}

fn reference(entry: &RemoteReference) -> Option<&str> {
    let key = bytes(&entry.reference)?;
    if !matches!(
        entry.kind.as_str(),
        "head" | "branch" | "tag" | "peeled_tag" | "other"
    ) {
        return None;
    }
    let oid = &entry.oid.hex;
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if let Some(target) = &entry.symbolic_target {
        bytes(target)?;
    }
    Some(key)
}

impl Listing {
    pub fn page(&self) -> Option<&ReferencePage> {
        self.page.as_ref()
    }

    /// Failure leaves all accepted rows and the continuation unchanged. The UI
    /// stops auto-loading on the returned error and offers an explicit refresh.
    pub fn accept(
        &mut self,
        mut incoming: ReferencePage,
        remote: &Remote,
        cursor: Option<&str>,
    ) -> Result<(), String> {
        let metadata = &incoming.metadata;
        let entries = &incoming.entries;
        if incoming.snapshot.is_empty() || incoming.next_cursor.as_deref() == Some("") {
            return Err(INVALID.into());
        }
        let next = incoming.next_cursor.clone();
        if remote.name.is_empty()
            || remote.token.is_empty()
            || metadata.remote != remote.name
            || metadata.remote_token != remote.token
            || metadata.basis != "remote_advertisement"
            || !metadata.for_push
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
            && (self.page.as_ref().is_none_or(|page| {
                page.next_cursor.as_deref() != Some(cursor)
                    || page.snapshot != incoming.snapshot
                    || page.metadata != *metadata
            }) || self.cursors.contains(cursor)
                || next.as_deref() == Some(cursor)
                || next
                    .as_ref()
                    .is_some_and(|next| self.cursors.contains(next)))
        {
            return Err(CHANGED.into());
        }
        // A bounded page must agree with the advertised total; a truncated
        // advertisement never claims completeness.
        let total = metadata.total_entries;
        let loaded = keys.len()
            + if cursor.is_some() {
                self.references.len()
            } else {
                0
            };
        if loaded as u64 > total
            || (next.is_some() && loaded as u64 >= total)
            || (next.is_none() && !metadata.truncated && loaded as u64 != total)
        {
            return Err(INVALID.into());
        }
        if let Some(cursor) = cursor {
            let mut previous = self
                .page
                .take()
                .expect("a continuation requires an accepted first page")
                .entries;
            previous.append(&mut incoming.entries);
            incoming.entries = previous;
            self.cursors.insert(cursor.to_owned());
            self.references.extend(keys);
        } else {
            self.references = keys;
            self.cursors.clear();
        }
        self.page = Some(incoming);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git::ObjectId;

    fn remote() -> Remote {
        Remote {
            name: "team/upstream".into(),
            url: None,
            push_url: None,
            token: "config".into(),
        }
    }
    fn entry(name: &str) -> RemoteReference {
        RemoteReference {
            reference: Path::new(name.as_bytes()),
            kind: "branch".into(),
            oid: ObjectId {
                algorithm: "sha1".into(),
                hex: "a".repeat(40),
            },
            symbolic_target: None,
        }
    }
    /// One page whose listing holds exactly these names.
    fn page(names: &[&str], next: Option<&str>) -> ReferencePage {
        paged(names, next, names.len())
    }
    /// One page of a listing that holds `total` references in all.
    fn paged(names: &[&str], next: Option<&str>, total: usize) -> ReferencePage {
        Page {
            snapshot: "s".into(),
            entries: names.iter().map(|name| entry(name)).collect(),
            next_cursor: next.map(Into::into),
            metadata: RemoteReferenceMetadata {
                remote: "team/upstream".into(),
                remote_token: "config".into(),
                for_push: true,
                basis: "remote_advertisement".into(),
                truncated: false,
                total_entries: total as u64,
            },
        }
    }
    #[test]
    fn appends_only_the_requested_snapshot_and_refresh_starts_new_identity() {
        let mut list = Listing::default();
        list.accept(paged(&["main"], Some("a"), 2), &remote(), None)
            .unwrap();
        let before = list.page().cloned();
        let mut snapshot = paged(&["topic"], None, 2);
        snapshot.snapshot = "changed".into();
        let mut metadata = paged(&["topic"], None, 2);
        metadata.metadata.truncated = true;
        for invalid in [snapshot, metadata] {
            assert!(list.accept(invalid, &remote(), Some("a")).is_err());
            assert_eq!(list.page().cloned(), before);
        }
        list.accept(paged(&["topic"], None, 2), &remote(), Some("a"))
            .unwrap();
        assert_eq!(list.page().unwrap().entries.len(), 2);
        list.accept(paged(&["main"], Some("a"), 2), &remote(), None)
            .unwrap();
        list.accept(paged(&["topic"], None, 2), &remote(), Some("a"))
            .unwrap();
    }
    #[test]
    fn duplicate_rows_and_cursor_cycles_cannot_drain_pagination() {
        let mut list = Listing::default();
        assert!(
            list.accept(page(&["main", "main"], None), &remote(), None)
                .is_err()
        );
        list.accept(paged(&["main"], Some("a"), 3), &remote(), None)
            .unwrap();
        assert!(
            list.accept(paged(&["main"], Some("b"), 3), &remote(), Some("a"))
                .is_err()
        );
        assert!(
            list.accept(paged(&[], Some("b"), 3), &remote(), Some("a"))
                .is_err()
        );
        assert!(
            list.accept(paged(&["topic"], Some("a"), 3), &remote(), Some("a"))
                .is_err()
        );
        list.accept(paged(&["topic"], Some("b"), 3), &remote(), Some("a"))
            .unwrap();
        let before = list.page().cloned();
        assert!(
            list.accept(paged(&["third"], Some("a"), 3), &remote(), Some("b"))
                .is_err()
        );
        assert_eq!(list.page().cloned(), before);
    }
    #[test]
    fn scope_and_advertisement_limits_are_checked() {
        let valid = page(&["main"], None);
        let changes: [&dyn Fn(&mut ReferencePage); 10] = [
            &|page| page.snapshot.clear(),
            &|page| page.metadata.remote_token = "other".into(),
            &|page| page.metadata.remote = "origin".into(),
            &|page| page.metadata.for_push = false,
            &|page| page.metadata.basis = "stored".into(),
            &|page| page.entries[0].reference.bytes_b64 = "!".into(),
            &|page| page.entries[0].oid.hex = "abc".into(),
            &|page| page.entries[0].kind = "unknown".into(),
            &|page| {
                page.entries[0].symbolic_target = Some(Path {
                    bytes_b64: String::new(),
                    display: String::new(),
                })
            },
            &|page| page.next_cursor = Some(String::new()),
        ];
        let mut cases = Vec::new();
        for change in changes {
            let mut bad = valid.clone();
            change(&mut bad);
            cases.push(bad);
        }
        let names: Vec<_> = (0..=PAGE_SIZE).map(|i| format!("branch-{i}")).collect();
        cases.push(page(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
            None,
        ));
        for bad in cases {
            let mut list = Listing::default();
            assert!(
                list.accept(bad.clone(), &remote(), None).is_err(),
                "{bad:?}"
            );
            assert!(list.page().is_none());
        }
    }
    #[test]
    fn totals_and_truncation_do_not_falsely_claim_completion() {
        let mut list = Listing::default();
        let mut data = page(&["main"], None);
        data.metadata.total_entries = 2;
        assert!(list.accept(data.clone(), &remote(), None).is_err());
        data.metadata.truncated = true;
        list.accept(data, &remote(), None).unwrap();
        list.accept(page(&[], None), &remote(), None).unwrap();
    }
    #[test]
    fn display_collisions_do_not_merge_different_raw_references() {
        let mut data = page(&["refs/heads/�", "refs/heads/�"], None);
        data.entries[0].reference = Path::new(b"refs/heads/\xff");
        data.entries[1].reference = Path::new(b"refs/heads/\xfe");
        Listing::default().accept(data, &remote(), None).unwrap();
    }
}
