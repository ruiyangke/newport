//! Bounded page envelopes with typed, opaque cursors.
use super::super::tokens::{CursorRef, SnapshotRef};
use super::*;
pub(super) fn page(
    repo: &Repo,
    query: String,
    fingerprint: String,
    rows: Vec<Value>,
    count: usize,
    cursor: Option<String>,
    metadata: Value,
) -> Result<Value, Error> {
    if !(1..=200).contains(&count) {
        return Err(Error::invalid("pageSize must be between 1 and 200."));
    }
    let snapshot = SnapshotRef {
        r: repo.id.clone(),
        q: query,
        p: None,
        f: fingerprint,
    };
    let offset = if let Some(cursor) = cursor {
        let c = CursorRef::decode(&cursor)?;
        if c.s.r != snapshot.r || c.s.q != snapshot.q || c.s.p.is_some() || c.k.is_some() {
            return Err(Error::invalid("Cursor does not match this query."));
        }
        if c.s.f != snapshot.f {
            return Err(Error::new(
                "SNAPSHOT_EXPIRED",
                "The listing changed. Restart from the first page.",
            ));
        }
        c.o
    } else {
        0
    };
    if offset > rows.len() {
        return Err(Error::invalid("Cursor is past the end of the listing."));
    }
    let total = rows.len();
    let mut result = Vec::new();
    let mut bytes = 0;
    for row in rows.into_iter().skip(offset).take(count) {
        let size = serde_json::to_vec(&row).map_err(|_| failure())?.len();
        if size > super::super::protocol::MAX_FRAME / 2 {
            return Err(Error::new(
                "LIMIT_EXCEEDED",
                "A listing row exceeds the response limit.",
            ));
        }
        if bytes + size > super::super::protocol::MAX_FRAME / 2 {
            break;
        }
        bytes += size;
        result.push(row);
    }
    let end = offset + result.len();
    let token = snapshot.encode();
    let next = (end < total).then(|| {
        CursorRef {
            s: snapshot,
            o: end,
            k: None,
        }
        .encode()
    });
    Ok(json!({"snapshot":token,"entries":result,"nextCursor":next,"metadata":metadata}))
}
pub(super) fn hash(value: &Value) -> Result<String, Error> {
    Ok(hex(&Sha256::digest(
        serde_json::to_vec(value).map_err(|_| failure())?,
    )))
}
