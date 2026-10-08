use super::super::{
    protocol::StatusFilter,
    tokens::{EntryRef, SnapshotRef},
};
use super::*;
use std::io::Read;

pub(super) struct Capture {
    pub rows: Vec<Value>,
    pub fingerprint: String,
    pub metadata: Value,
}
fn side(mode: &[u8], id: &[u8], path: &[u8]) -> Result<Value, Error> {
    if mode == b"000000" {
        return Ok(Value::Null);
    }
    Ok(
        json!({"mode":u32::from_str_radix(std::str::from_utf8(mode).map_err(|_|failure())?,8).map_err(|_|failure())?,"oid":oid(id)?,"path":WirePath::new(path)}),
    )
}
fn fields(record: &[u8], count: usize) -> Result<Vec<&[u8]>, Error> {
    let result = record.splitn(count, |b| *b == b' ').collect::<Vec<_>>();
    if result.len() != count {
        return Err(failure());
    }
    Ok(result)
}
fn flags(x: u8, y: u8) -> u32 {
    let index = match x {
        b'A' => 1,
        b'M' => 2,
        b'D' => 4,
        b'R' | b'C' => 8,
        b'T' => 16,
        _ => 0,
    };
    index
        | match y {
            b'M' => 256,
            b'D' => 512,
            b'T' => 1024,
            b'R' | b'C' => 2048,
            _ => 0,
        }
}
pub(super) fn capture(repo: &Repo) -> Result<Capture, Error> {
    if repo.bare {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "Bare repositories have no working tree.",
        ));
    }
    let bytes = repo.run(&[
        "-c",
        "status.renames=true",
        "status",
        "--porcelain=v2",
        "-z",
        "--untracked-files=normal",
        "--ignore-submodules=none",
    ])?;
    let mut records = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut rows = Vec::new();
    while let Some(record) = records.next() {
        let (path, old, xy, bits, conflict) = match record[0] {
            b'?' => (&record[2..], None, b"??".as_slice(), 128, Value::Null),
            b'1' | b'2' => {
                let f = fields(record, if record[0] == b'1' { 9 } else { 10 })?;
                if f[1].len() != 2 {
                    return Err(failure());
                }
                let path = *f.last().ok_or_else(failure)?;
                let old = if record[0] == b'2' {
                    Some(records.next().ok_or_else(failure)?)
                } else {
                    Some(path)
                };
                (path, old, f[1], flags(f[1][0], f[1][1]), Value::Null)
            }
            b'u' => {
                let f = fields(record, 11)?;
                let path = f[10];
                (
                    path,
                    Some(path),
                    f[1],
                    32768,
                    json!({"base":side(f[3],f[7],path)?,"ours":side(f[4],f[8],path)?,"theirs":side(f[5],f[9],path)?}),
                )
            }
            _ => return Err(failure()),
        };
        if xy.len() != 2 {
            return Err(failure());
        }
        let mut paths = vec![path.to_vec()];
        if let Some(old) = old {
            paths.push(old.to_vec());
        }
        paths.sort();
        paths.dedup();
        rows.push(json!({"entryId":EntryRef::new(&paths).encode(),"path":WirePath::new(path),"oldPath":old.map(WirePath::new),"flags":bits,"staged":bits&31!=0,"unstaged":bits&3840!=0,"untracked":bits&128!=0,"conflicted":bits&32768!=0,"conflict":conflict}));
    }
    rows.sort_by_key(|r| r["path"]["display"].as_str().unwrap_or_default().to_owned());
    let metadata = repo.summary()?;
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(&metadata).map_err(|_| failure())?);
    hash.update(&bytes);
    // Include identity and nanosecond change times, not just status letters.
    // Small-file content supplements metadata within a bounded read budget.
    let mut budget = 256 * 1024 * 1024u64;
    hash_file(&repo.git_dir.join("index"), &mut hash, &mut budget)?;
    if rows.iter().any(|r| r["untracked"] == true) {
        let paths = repo.run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        hash.update(&paths);
        for path in paths.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            hash_file(
                &repo.root.join(OsStr::from_bytes(path)),
                &mut hash,
                &mut budget,
            )?;
        }
    }
    for row in &rows {
        if row["untracked"] == true {
            continue;
        }
        let path: WirePath = serde_json::from_value(row["path"].clone()).map_err(|_| failure())?;
        hash_file(
            &repo.root.join(OsStr::from_bytes(&path.decode()?)),
            &mut hash,
            &mut budget,
        )?;
    }
    Ok(Capture {
        rows,
        fingerprint: hex(&hash.finalize()),
        metadata,
    })
}
fn hash_file(path: &Path, hash: &mut Sha256, budget: &mut u64) -> Result<(), Error> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            hash.update(b"missing");
            return Ok(());
        }
        // One unreadable entry must not fail the whole status: git itself
        // reports it and moves on. The marker keeps the fingerprint
        // deterministic, and a permission flap still changes it -- writes
        // then conflict until refreshed, which is the safe direction.
        Err(_) => {
            hash.update(b"unreadable");
            return Ok(());
        }
    };
    for value in [u64::from(meta.mode()), meta.len(), meta.dev(), meta.ino()] {
        hash.update(value.to_be_bytes());
    }
    for value in [
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    ] {
        hash.update(value.to_be_bytes());
    }
    if meta.file_type().is_symlink() {
        match fs::read_link(path) {
            Ok(target) => hash.update(target.as_os_str().as_bytes()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => hash.update(b"missing"),
            Err(_) => hash.update(b"unreadable"),
        }
    } else if meta.is_dir() {
        let read_dir = match fs::read_dir(path) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                hash.update(b"missing");
                return Ok(());
            }
            Err(_) => {
                hash.update(b"unreadable");
                return Ok(());
            }
        };
        // Collect-then-sort keeps the walk deterministic; entries that
        // vanish or deny listing mid-scan hash one fixed marker instead of
        // failing, so their order cannot flap the fingerprint.
        let mut unreadable = false;
        let mut children = Vec::new();
        for entry in read_dir {
            match entry {
                Ok(entry) => children.push(entry),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => hash.update(b"missing"),
                Err(_) => unreadable = true,
            }
        }
        if unreadable {
            hash.update(b"unreadable");
        }
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child.file_name();
            if name == ".git" {
                continue;
            }
            if *budget == 0 {
                return Err(Error::new(
                    "LIMIT_EXCEEDED",
                    "Status snapshot exceeds its read budget.",
                ));
            }
            *budget -= 1;
            hash.update(name.as_bytes());
            hash_file(&child.path(), hash, budget)?;
        }
    } else if meta.is_file() {
        // Large build artifacts must not make status unavailable or force a
        // full read on every page. Inode, size, mtime
        // and ctime guard these files; small contents provide extra protection.
        if meta.len() > 64 * 1024 || meta.len() > *budget {
            return Ok(());
        }
        *budget -= meta.len();
        // The file can vanish or lose permission between listing and reading
        // on a live tree; hash what happened instead of failing the status.
        let mut file = match fs::File::open(path).map(|file| file.take(meta.len() + 1)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                hash.update(b"missing");
                return Ok(());
            }
            Err(_) => {
                hash.update(b"unreadable");
                return Ok(());
            }
        };
        let mut buffer = [0; 8192];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => hash.update(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    hash.update(b"missing");
                    return Ok(());
                }
                Err(_) => {
                    hash.update(b"unreadable");
                    return Ok(());
                }
            }
        }
    } else {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "Special files cannot be included in a write snapshot.",
        ));
    }
    Ok(())
}
pub(super) fn page(
    repo: &Repo,
    filter: Option<StatusFilter>,
    count: usize,
    cursor: Option<String>,
) -> Result<Value, Error> {
    let filter = filter.unwrap_or_default();
    let group = filter.group.as_deref().unwrap_or("all");
    if filter.text.len() > 1024
        || filter.text.contains('\0')
        || !["all", "staged", "unstaged", "untracked", "conflicted"].contains(&group)
    {
        return Err(Error::invalid("Invalid changed-file filter."));
    }
    let mut capture = capture(repo)?;
    let text = filter.text.trim().to_lowercase();
    capture.rows.retain(|r| {
        (group == "all" || r[group] == true)
            && (text.is_empty()
                || format!(
                    "{} {}",
                    r["path"]["display"].as_str().unwrap_or_default(),
                    r["oldPath"]["display"].as_str().unwrap_or_default()
                )
                .to_lowercase()
                .contains(&text))
    });
    capture.metadata["matchedEntries"] = json!(capture.rows.len());
    let fingerprint = capture.fingerprint.clone();
    let mut result = pages::page(
        repo,
        format!("cli.status:{}", json!([text, group])),
        capture.fingerprint,
        capture.rows,
        count,
        cursor,
        capture.metadata,
    )?;
    // Filter cursors are query-specific, but write snapshots cover all changes.
    result["snapshot"] = json!(SnapshotRef {
        r: repo.id.clone(),
        q: "cli.status".into(),
        f: fingerprint,
        p: None
    }
    .encode());
    Ok(result)
}
