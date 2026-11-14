//! Snapshot-bound Git page assembly, independent of the rendering framework.
use newport_protocol::git::Page;

const INVALID: &str = "Invalid repository page. Refresh to retry.";
const CHANGED: &str = "The repository listing changed. Refresh before loading more.";
// Matches the desktop Git response decoder. Accumulated listings may be larger.
const MAX_PAGE_ENTRIES: usize = 200;

/// Check one decoded wire page before storing or presenting it: a snapshot,
/// a cursor that is either absent or nonempty, and a bounded entry count.
pub fn validate_page<T, M>(page: &Page<T, M>) -> Result<(), String> {
    if page.snapshot.is_empty()
        || page.next_cursor.as_deref() == Some("")
        || page.entries.len() > MAX_PAGE_ENTRIES
    {
        return Err(INVALID.into());
    }
    Ok(())
}

/// Check that `incoming` continues `previous` at `cursor`: same snapshot and
/// metadata, and a cursor that advances. `previous` may be an accumulated
/// listing, so only `incoming` is held to the per-response entry bound.
pub fn validate_page_continuation<T, M: PartialEq>(
    previous: &Page<T, M>,
    incoming: &Page<T, M>,
    cursor: &str,
) -> Result<(), String> {
    if previous.snapshot.is_empty() || previous.next_cursor.as_deref() == Some("") {
        return Err(INVALID.into());
    }
    validate_page(incoming)?;
    if previous.next_cursor.as_deref() != Some(cursor)
        || previous.snapshot != incoming.snapshot
        || previous.metadata != incoming.metadata
        || incoming.next_cursor.as_deref() == Some(cursor)
    {
        return Err(CHANGED.into());
    }
    Ok(())
}

/// Append a continuation page to an accepted listing, keeping the incoming
/// envelope. Errors leave the accepted page untouched.
pub fn merge_pages<T: Clone, M: PartialEq>(
    previous: &Page<T, M>,
    mut incoming: Page<T, M>,
    cursor: &str,
) -> Result<Page<T, M>, String> {
    validate_page_continuation(previous, &incoming, cursor)?;
    let mut entries = Vec::with_capacity(previous.entries.len() + incoming.entries.len());
    entries.extend_from_slice(&previous.entries);
    entries.append(&mut incoming.entries);
    incoming.entries = entries;
    Ok(incoming)
}

#[cfg(test)]
mod typed_tests {
    use super::merge_pages;
    use newport_protocol::git::{Page, TotalEntries};

    fn page(entries: Vec<u32>, total: u64, next: Option<&str>) -> Page<u32, TotalEntries> {
        Page {
            snapshot: "s".into(),
            entries,
            next_cursor: next.map(Into::into),
            metadata: TotalEntries {
                total_entries: total,
            },
        }
    }

    #[test]
    fn typed_pages_append_in_order_under_the_incoming_envelope() {
        let first = page(vec![1], 2, Some("next"));
        let merged = merge_pages(&first, page(vec![2], 2, None), "next").unwrap();
        assert_eq!(merged.entries, vec![1, 2]);
        assert!(merged.next_cursor.is_none());
        assert_eq!(first.entries, vec![1]);
    }

    #[test]
    fn typed_pages_reject_changed_comparisons_and_stalled_cursors() {
        let first = page(vec![1], 2, Some("next"));
        let last = page(vec![2], 2, None);
        assert!(merge_pages(&first, last.clone(), "wrong").is_err());
        assert!(merge_pages(&first, first.clone(), "next").is_err());
        let mut changed = last.clone();
        changed.snapshot = "new".into();
        assert!(merge_pages(&first, changed, "next").is_err());
        assert!(merge_pages(&first, page(vec![2], 3, None), "next").is_err());
        assert!(merge_pages(&first, page(vec![2], 2, Some("")), "next").is_err());
        assert!(merge_pages(&page(vec![1], 2, None), last.clone(), "next").is_err());
        let oversized = page((0..201).collect(), 2, None);
        assert!(merge_pages(&first, oversized, "next").is_err());
        let mut unnamed = first.clone();
        unnamed.snapshot.clear();
        assert!(merge_pages(&unnamed, last.clone(), "next").is_err());
        let mut empty_cursor = first.clone();
        empty_cursor.next_cursor = Some(String::new());
        assert!(merge_pages(&empty_cursor, last, "").is_err());
    }

    #[test]
    fn bounds_each_response_without_limiting_accumulated_rows() {
        let first = page((0..600).collect(), 800, Some("next"));
        let merged = merge_pages(&first, page((600..800).collect(), 800, None), "next").unwrap();
        assert_eq!(merged.entries.len(), 800);
        let oversized = page((600..801).collect(), 800, None);
        assert!(merge_pages(&first, oversized, "next").is_err());
    }

    #[test]
    fn empty_terminal_pages_keep_every_accumulated_row() {
        let first = page(vec![1], 2, Some("next"));
        let merged = merge_pages(&first, page(vec![2], 2, Some("last")), "next").unwrap();
        let complete = merge_pages(&merged, page(vec![], 2, None), "last").unwrap();
        assert_eq!(complete.entries, vec![1, 2]);
        assert!(complete.next_cursor.is_none());
    }
}
