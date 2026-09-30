use super::super::tokens::{CursorRef, SnapshotRef};
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
pub(super) fn blob(
    repo: &Repo,
    id: &str,
    budget: Option<usize>,
    cursor: Option<String>,
    paged: bool,
) -> Result<Value, Error> {
    let object =
        oid(id.as_bytes()).map_err(|_| Error::invalid("A complete object ID is required."))?;
    let header = command::run_input(
        &repo.root,
        &["cat-file", "--batch-check"],
        format!("{id}\n").into_bytes(),
    )?
    .ok_or_else(failure)?;
    let parts = std::str::from_utf8(trim_line(&header))
        .map_err(|_| failure())?
        .split(' ')
        .collect::<Vec<_>>();
    if parts.len() != 3 || parts[1] != "blob" {
        return Err(Error::invalid("The object is not a file blob."));
    }
    let size = parts[2].parse::<usize>().map_err(|_| failure())?;
    if !paged && size > 512 * 1024 {
        return Ok(json!({"oid":object,"size":size,"bytesB64":null,"truncated":true}));
    }
    let budget = budget.unwrap_or(64 * 1024);
    if !(4096..=524288).contains(&budget) {
        return Err(Error::invalid("maxBytes must be 4096–524288."));
    }
    let snapshot = SnapshotRef {
        r: repo.id.clone(),
        q: format!("cli.blob:{id}"),
        p: None,
        f: id.into(),
    };
    let offset = if let Some(cursor) = cursor {
        let c = CursorRef::decode(&cursor)?;
        if c.s != snapshot || c.k.is_some() {
            return Err(Error::invalid("Cursor does not match this blob."));
        }
        c.o
    } else {
        0
    };
    if offset > size {
        return Err(Error::invalid("Cursor is past the end of this blob."));
    }
    let end = if paged {
        offset
            .saturating_add(budget.saturating_sub(4096).max(1024) * 3 / 4)
            .min(size)
    } else {
        size
    };
    let data = command::window(&repo.root, &["cat-file", "blob", id], offset, end - offset)?;
    if data.len() != end - offset {
        return Err(failure());
    }
    if !paged {
        return Ok(
            json!({"oid":object,"size":size,"bytesB64":STANDARD.encode(data),"truncated":false}),
        );
    }
    let rows = if end == offset {
        vec![]
    } else {
        vec![json!({"offset":offset,"bytesB64":STANDARD.encode(&data)})]
    };
    let next = (end < size).then(|| {
        CursorRef {
            s: snapshot.clone(),
            o: end,
            k: None,
        }
        .encode()
    });
    Ok(
        json!({"snapshot":snapshot.encode(),"entries":rows,"nextCursor":next,"metadata":{"oid":object,"size":size}}),
    )
}
