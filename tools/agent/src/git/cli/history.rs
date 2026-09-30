use super::super::tokens::{CursorRef, SnapshotRef};
use super::*;

fn valid_oid(value: &str) -> Result<(), Error> {
    oid(value.as_bytes())
        .map(|_| ())
        .map_err(|_| Error::invalid("A complete commit ID is required."))
}
fn signature(value: &[u8]) -> Result<Value, Error> {
    let end = value.iter().rposition(|b| *b == b'>').ok_or_else(failure)?;
    let start = value[..end]
        .iter()
        .rposition(|b| *b == b'<')
        .ok_or_else(failure)?;
    Ok(
        json!({"name":String::from_utf8_lossy(value[..start].strip_suffix(b" ").unwrap_or(&value[..start])),"email":String::from_utf8_lossy(&value[start+1..end])}),
    )
}
fn row(id: &str, raw: &[u8], budget: usize) -> Result<Value, Error> {
    let split = raw
        .windows(2)
        .position(|w| w == b"\n\n")
        .ok_or_else(failure)?;
    let mut parents = Vec::new();
    let mut author = None;
    let mut time = None;
    let mut offset = None;
    for line in raw[..split].split(|b| *b == b'\n') {
        if let Some(value) = line.strip_prefix(b"parent ") {
            parents.push(oid(value)?);
        }
        if let Some(value) = line.strip_prefix(b"author ") {
            author = Some(signature(value)?);
        }
        if let Some(value) = line.strip_prefix(b"committer ") {
            let fields = value.rsplitn(3, |b| *b == b' ').collect::<Vec<_>>();
            if fields.len() != 3 {
                return Err(failure());
            }
            time = Some(
                std::str::from_utf8(fields[1])
                    .map_err(|_| failure())?
                    .parse::<i64>()
                    .map_err(|_| failure())?,
            );
            let zone = fields[0];
            if zone.len() != 5
                || !matches!(zone[0], b'+' | b'-')
                || !zone[1..].iter().all(u8::is_ascii_digit)
            {
                return Err(failure());
            }
            let hours = ((zone[1] - b'0') as i32) * 10 + (zone[2] - b'0') as i32;
            let minutes = ((zone[3] - b'0') as i32) * 10 + (zone[4] - b'0') as i32;
            offset = Some((hours * 60 + minutes) * if zone[0] == b'-' { -1 } else { 1 });
        }
    }
    let message = &raw[split + 2..];
    Ok(
        json!({"oid":oid(id.as_bytes())?,"parents":parents,"message":WirePath::new(&message[..message.len().min(budget)]),"messageTruncated":message.len()>budget,"author":author.ok_or_else(failure)?,"time":time.ok_or_else(failure)?,"offsetMinutes":offset.ok_or_else(failure)?}),
    )
}
fn commits(repo: &Repo, ids: &[&str], budget: usize) -> Result<Vec<Value>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    for id in ids {
        valid_oid(id)?;
    }
    let input = format!("{}\n", ids.join("\n")).into_bytes();
    let bytes =
        command::run_input(&repo.root, &["cat-file", "--batch"], input)?.ok_or_else(failure)?;
    let mut rest = bytes.as_slice();
    let mut rows = Vec::new();
    for id in ids {
        let end = rest.iter().position(|b| *b == b'\n').ok_or_else(failure)?;
        let header = std::str::from_utf8(&rest[..end])
            .map_err(|_| failure())?
            .split(' ')
            .collect::<Vec<_>>();
        if header.len() != 3 || header[0] != *id || header[1] != "commit" {
            return Err(Error::new("NOT_FOUND", "The commit is not available."));
        }
        let size = header[2].parse::<usize>().map_err(|_| failure())?;
        rest = &rest[end + 1..];
        if size >= rest.len() || rest[size] != b'\n' {
            return Err(failure());
        }
        rows.push(row(id, &rest[..size], budget)?);
        rest = &rest[size + 1..];
    }
    if !rest.is_empty() {
        return Err(failure());
    }
    Ok(rows)
}
pub(super) fn commit(repo: &Repo, id: &str, budget: usize) -> Result<Value, Error> {
    commits(repo, &[id], budget)?.pop().ok_or_else(failure)
}
pub(super) fn page(
    repo: Repo,
    count: usize,
    cursor: Option<String>,
    revision: String,
    budget: Option<usize>,
) -> Result<Value, Error> {
    let budget = budget.unwrap_or(16384);
    if !(1..=200).contains(&count)
        || !(1..=16384).contains(&budget)
        || revision.len() > 1024
        || revision.contains('\0')
    {
        return Err(Error::invalid("Invalid history page parameters."));
    }
    // Engine-specific opaque cursors prevent accidental cross-engine reuse.
    let query = format!("cli.history:{}", json!([revision, budget]));
    let previous = cursor.as_deref().map(CursorRef::decode).transpose()?;
    if previous
        .as_ref()
        .is_some_and(|c| c.s.r != repo.id || c.s.q != query || c.k.is_some())
    {
        return Err(Error::invalid("Cursor does not match this query."));
    }
    let offset = previous.as_ref().map_or(0, |c| c.o);
    let tip = if let Some(c) = &previous {
        c.s.p.clone()
    } else {
        match command::run(
            &repo.root,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{revision}^{{commit}}"),
            ],
        )? {
            Some(value) => {
                Some(String::from_utf8(trim_line(&value).to_vec()).map_err(|_| failure())?)
            }
            None if revision == "HEAD" && repo.head()?["unborn"] == true => None,
            None => {
                return Err(Error::new(
                    "NOT_FOUND",
                    "The branch or commit no longer exists.",
                ))
            }
        }
    };
    if let Some(tip) = &tip {
        valid_oid(tip)?;
    }
    let mut hash = Sha256::new();
    hash.update(tip.as_deref().unwrap_or_default());
    let shallow = repo.common.join("shallow");
    if shallow.metadata().is_ok_and(|m| m.len() > 8 * 1024 * 1024) {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "Shallow boundary exceeds the CLI backend limit.",
        ));
    }
    match fs::read(shallow) {
        Ok(value) => hash.update(value),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(io_error(e)),
    }
    let fingerprint = hex(&hash.finalize());
    if previous.as_ref().is_some_and(|c| c.s.f != fingerprint) {
        return Err(Error::new(
            "SNAPSHOT_EXPIRED",
            "History changed. Restart from the first page.",
        ));
    }
    let snapshot = SnapshotRef {
        r: repo.id.clone(),
        q: query,
        f: fingerprint,
        p: tip.clone(),
    };
    let token = snapshot.encode();
    let Some(tip) = tip else {
        return Ok(json!({"snapshot":token,"entries":[],"nextCursor":null,"metadata":{}}));
    };
    let list = repo.run(&[
        "rev-list",
        "--date-order",
        &format!("--skip={offset}"),
        &format!("--max-count={}", count + 1),
        &tip,
        "--",
    ])?;
    let text = std::str::from_utf8(&list).map_err(|_| failure())?;
    let ids = text.lines().collect::<Vec<_>>();
    if ids.is_empty() && offset > 0 {
        return Err(Error::invalid("Cursor is past the end of history."));
    }
    let mut rows = commits(&repo, &ids[..ids.len().min(count)], budget)?;
    let mut bytes = 0;
    let mut end = 0;
    for row in &rows {
        let size = serde_json::to_vec(row).map_err(|_| failure())?.len();
        if size > super::super::protocol::MAX_FRAME / 2 {
            return Err(Error::new(
                "LIMIT_EXCEEDED",
                "Commit exceeds the response limit.",
            ));
        }
        if bytes + size > super::super::protocol::MAX_FRAME / 2 {
            break;
        }
        bytes += size;
        end += 1;
    }
    rows.truncate(end);
    let next = (ids.len() > end).then(|| {
        CursorRef {
            s: snapshot,
            o: offset + end,
            k: None,
        }
        .encode()
    });
    Ok(
        json!({"snapshot":token,"entries":rows,"nextCursor":next,"metadata":{"resolvedRevision":oid(tip.as_bytes())?,"truncated":false}}),
    )
}
