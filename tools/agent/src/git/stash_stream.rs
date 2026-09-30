//! Bounded reflog reader. Parsing follows libgit2's refdb_fs/signature semantics
//! and is differential-tested against the native parser before adoption.
use super::*;
use std::{
    fs::{File, OpenOptions},
    io,
    ops::Range,
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
};
const BLOCK: usize = 64 * 1024;
struct Reader {
    file: File,
    start: u64,
    valid: usize,
    buffer: Box<[u8; BLOCK]>,
    length: u64,
}
impl Reader {
    fn byte(&mut self, at: u64) -> Result<u8, Error> {
        if at >= self.length {
            return Err(io_error(io::Error::from(io::ErrorKind::UnexpectedEof)));
        }
        if self.valid == 0 || at < self.start || at >= self.start + self.valid as u64 {
            self.start = at / BLOCK as u64 * BLOCK as u64;
            self.valid = (self.length - self.start).min(BLOCK as u64) as usize;
            self.file
                .read_exact_at(&mut self.buffer[..self.valid], self.start)
                .map_err(io_error)?;
        }
        Ok(self.buffer[(at - self.start) as usize])
    }
    fn range(&mut self, range: Range<u64>) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::with_capacity((range.end - range.start) as usize);
        for at in range {
            bytes.push(self.byte(at)?);
        }
        Ok(bytes)
    }
    fn find(
        &mut self,
        range: Range<u64>,
        predicate: impl Fn(u8) -> bool,
    ) -> Result<Option<u64>, Error> {
        for at in range {
            if predicate(self.byte(at)?) {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
    fn rfind(
        &mut self,
        range: Range<u64>,
        predicate: impl Fn(u8) -> bool,
    ) -> Result<Option<u64>, Error> {
        for at in range.rev() {
            if predicate(self.byte(at)?) {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
    fn hash(&mut self, hash: &mut Sha256, range: Range<u64>) -> Result<(), Error> {
        hash.update((range.end - range.start).to_be_bytes());
        let mut at = range.start;
        while at < range.end {
            self.byte(at)?;
            let start = (at - self.start) as usize;
            let length = (range.end - at).min((self.valid - start) as u64) as usize;
            hash.update(&self.buffer[start..start + length]);
            at += length as u64;
        }
        Ok(())
    }
    fn cstring(&mut self, range: Range<u64>) -> Result<Range<u64>, Error> {
        let end = self.find(range.clone(), |b| b == 0)?.unwrap_or(range.end);
        Ok(range.start..end)
    }
    fn trimmed(&mut self, range: Range<u64>) -> Result<Range<u64>, Error> {
        let crud = |b: u8| b <= 32 || b",:;<>\"\\'".contains(&b);
        let start = self.find(range.clone(), |b| !crud(b))?.unwrap_or(range.end);
        let end = self
            .rfind(start..range.end, |b| !crud(b))?
            .map_or(start, |at| at + 1);
        self.cstring(start..end)
    }
}
struct Entry {
    old: git2::Oid,
    new: git2::Oid,
    time: i64,
    offset: i32,
    name: Range<u64>,
    email: Range<u64>,
    message: Range<u64>,
}
fn integer(reader: &mut Reader, range: Range<u64>) -> Result<Option<(i64, u64)>, Error> {
    let mut at = range.start;
    while at < range.end && reader.byte(at)?.is_ascii_whitespace() {
        at += 1;
    }
    let negative = at < range.end && reader.byte(at)? == b'-';
    if at < range.end && matches!(reader.byte(at)?, b'+' | b'-') {
        at += 1;
    }
    let first = at;
    let mut value = Some(0i64);
    while at < range.end {
        let b = reader.byte(at)?;
        if !b.is_ascii_digit() {
            break;
        }
        value = value.and_then(|v| v.checked_mul(10)).and_then(|v| {
            if negative {
                v.checked_sub(i64::from(b - b'0'))
            } else {
                v.checked_add(i64::from(b - b'0'))
            }
        });
        at += 1;
    }
    Ok(if first == at {
        None
    } else {
        value.map(|value| (value, at))
    })
}
fn parse(
    reader: &mut Reader,
    line: Range<u64>,
    format: git2::ObjectFormat,
    message_limit: Option<u64>,
) -> Result<Option<Entry>, Error> {
    let width = if format == git2::ObjectFormat::Sha1 {
        40
    } else {
        64
    };
    if line.end - line.start < (width * 2 + 1) as u64 {
        return Ok(None);
    }
    let middle = line.start + width as u64;
    if reader.byte(middle)? != b' ' {
        return Ok(None);
    }
    let parse_oid = |bytes: Vec<u8>| {
        std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| git2::Oid::from_str_ext(s, format).ok())
    };
    let Some(old) = parse_oid(reader.range(line.start..middle)?) else {
        return Ok(None);
    };
    let signature = middle + 1 + width as u64;
    let Some(new) = parse_oid(reader.range(middle + 1..signature)?) else {
        return Ok(None);
    };
    let tab = reader.find(signature..line.end, |b| b == b'\t')?;
    let end = tab.unwrap_or(line.end);
    let Some(left) = reader.rfind(signature..end, |b| b == b'<')? else {
        return Ok(None);
    };
    let Some(right) = reader.rfind(signature..end, |b| b == b'>')? else {
        return Ok(None);
    };
    if right <= left {
        return Ok(None);
    }
    let name = reader.trimmed(signature..left)?;
    let email = reader.trimmed(left + 1..right)?;
    let (mut time, mut offset) = (0, 0);
    if right + 2 < end {
        let Some((seconds, after)) = integer(reader, right + 2..end)? else {
            return Ok(None);
        };
        time = seconds;
        if after + 1 < end {
            let zone = reader.byte(after + 1)?;
            let value = if matches!(zone, b'+' | b'-') {
                integer(reader, after + 2..end)?
                    .and_then(|(v, _)| i32::try_from(v).ok())
                    .unwrap_or(0)
            } else {
                0
            };
            let (hours, minutes) = (value / 100, value % 100);
            if hours <= 14 && minutes <= 59 {
                offset = hours * 60 + minutes;
                if zone == b'-' {
                    offset = -offset;
                }
            }
        }
    }
    let message_start = tab.map_or(line.end, |p| p + 1);
    let message_end = message_limit.map_or(line.end, |limit| {
        message_start.saturating_add(limit).min(line.end)
    });
    let message = reader.cstring(message_start..message_end)?;
    Ok(Some(Entry {
        old,
        new,
        time,
        offset,
        name,
        email,
        message,
    }))
}
type Identity = (u64, u64, u64, i64, i64, i64, i64);
fn identity(m: &fs::Metadata) -> Identity {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
struct Source {
    file: std::sync::Arc<File>,
    path: std::path::PathBuf,
    identity: Identity,
    format: git2::ObjectFormat,
}
pub(super) struct Captured {
    pub entries: Vec<Range<u64>>,
    pub bytes: usize,
    source: Option<Source>,
}
impl Captured {
    pub(super) fn rows(&self, offset: usize, count: usize) -> Result<Vec<Row>, Error> {
        let Some(source) = &self.source else {
            return Ok(Vec::new());
        };
        let validate = || {
            let error = || {
                Error::new(
                    "SNAPSHOT_EXPIRED",
                    "The stash log changed. Restart from the first page.",
                )
            };
            let current = fs::metadata(&source.path).map_err(|_| error())?;
            if identity(&current) != source.identity {
                return Err(error());
            }
            Ok(())
        };
        validate()?;
        let mut reader = Reader {
            file: source.file.try_clone().map_err(io_error)?,
            start: 0,
            valid: 0,
            buffer: Box::new([0; BLOCK]),
            length: source.identity.2,
        };
        let mut rows = Vec::with_capacity(count);
        for (index, line) in self.entries.iter().enumerate().skip(offset).take(count) {
            let entry =
                parse(&mut reader, line.clone(), source.format, Some(1025))?.ok_or_else(|| {
                    Error::new(
                        "SNAPSHOT_EXPIRED",
                        "The stash log changed. Restart from the first page.",
                    )
                })?;
            rows.push(materialize(&mut reader, index, &entry)?);
        }
        validate()?;
        Ok(rows)
    }
}
fn materialize(reader: &mut Reader, index: usize, entry: &Entry) -> Result<Row, Error> {
    Ok(Row {
        index,
        oid: entry.new,
        previous: entry.old,
        message: reader
            .range(entry.message.start..(entry.message.start + 1024).min(entry.message.end))?,
        truncated: entry.message.end - entry.message.start > 1024,
        time: entry.time,
    })
}
pub(super) struct Read {
    pub token: String,
    pub total: usize,
    pub selected: Vec<Row>,
    pub captured: Option<Captured>,
}
pub(super) fn read(repo: &Repository, offset: usize, count: usize) -> Result<Read, Error> {
    let mut hash = Sha256::new();
    hash.update(b"newport-stash-list-v2\0");
    let mut result = Read {
        token: String::new(),
        total: 0,
        selected: Vec::new(),
        captured: Some(Captured {
            entries: Vec::new(),
            bytes: 0,
            source: None,
        }),
    };
    let path = repo.commondir().join("logs/refs/stash");
    let exists = match repo.find_reference(STASH) {
        Ok(_) => true,
        Err(e) if e.code() == git2::ErrorCode::NotFound => false,
        Err(e) => return Err(engine(e)),
    };
    let file = if exists {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => Some(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_error(e)),
        }
    } else {
        None
    };
    if let Some(file) = file {
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() {
            return Err(Error::new("IO_ERROR", "Stash log is not a regular file."));
        }
        let mut reader = Reader {
            file,
            start: 0,
            valid: 0,
            buffer: Box::new([0; BLOCK]),
            length: metadata.len(),
        };
        let mut end = reader.length;
        while end > 0 {
            if reader.byte(end - 1)? == b'\n' {
                end -= 1;
            }
            let start = reader.rfind(0..end, |b| b == b'\n')?.map_or(0, |n| n + 1);
            if let Some(entry) = parse(&mut reader, start..end, repo.object_format(), None)? {
                hash.update(entry.old.as_bytes());
                hash.update(entry.new.as_bytes());
                hash.update(entry.time.to_be_bytes());
                hash.update(entry.offset.to_be_bytes());
                for range in [
                    entry.name.clone(),
                    entry.email.clone(),
                    entry.message.clone(),
                ] {
                    reader.hash(&mut hash, range)?;
                }
                let select = result.total >= offset && result.total - offset < count;
                if select {
                    result
                        .selected
                        .push(materialize(&mut reader, result.total, &entry)?);
                }
                if let Some(captured) = &mut result.captured {
                    captured.entries.push(start..end);
                    captured.bytes += 2 * std::mem::size_of::<Range<u64>>();
                }
                if result
                    .captured
                    .as_ref()
                    .is_some_and(|c| c.bytes > CACHE_INDEX_BYTES)
                {
                    result.captured = None;
                }
                result.total += 1;
            }
            end = start;
        }
        let after = fs::metadata(&path)
            .map_err(|_| Error::new("REPOSITORY_BUSY", "The stash log changed while reading."))?;
        if identity(&metadata) != identity(&after) {
            return Err(Error::new(
                "REPOSITORY_BUSY",
                "The stash log changed while reading.",
            ));
        }
        if let Some(captured) = &mut result.captured {
            captured.source = Some(Source {
                file: std::sync::Arc::new(reader.file),
                path,
                identity: identity(&metadata),
                format: repo.object_format(),
            });
        }
    }
    result.token = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_matches_native_canonical_tokens_for_edge_cases() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let oid = repo.blob(b"blob").unwrap();
        repo.reference(STASH, oid, false, "").unwrap();
        let path = repo.commondir().join("logs/refs/stash");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let signatures: [&[u8]; 15] = [
            b" Name <mail> 123 +0100",
            b" Name <mail> -123 -0530",
            b" ,Name; <mail> 0 +1400",
            b" Name <mail> 1 +1500",
            b" Name <mail> 1 +0260",
            b" Name <mail> 1 garbage",
            b" Name <mail>",
            b" Name <mail> 123",
            b"Name <mail> 1 +-0100",
            b" Name <mail> 9223372036854775808 +0000",
            b" Name <mail> -9223372036854775808 +0000",
            b" N\xffme <m\xfeil> 3 -0000",
            b" N\0ame <m\0ail> 3 +0000",
            b" Name <nested<mail>> 4 +0000",
            b" malformed signature",
        ];
        for signature in signatures {
            for message in [
                b"message".as_slice(),
                b"message\0hidden",
                b"\xff\xfe",
                b"",
                b"message\r",
            ] {
                for newline in [false, true] {
                    let mut data = format!("{oid} {oid}").into_bytes();
                    data.extend_from_slice(signature);
                    data.push(b'\t');
                    data.extend_from_slice(message);
                    if newline {
                        data.push(b'\n');
                    }
                    fs::write(&path, data).unwrap();
                    let native = rows(log(&repo).unwrap().as_ref()).unwrap();
                    let streamed = read(&repo, 0, 200).unwrap();
                    assert_eq!(
                        streamed.token, native.1,
                        "signature={signature:?} message={message:?} newline={newline}"
                    );
                    assert_eq!(
                        streamed.selected.iter().map(Row::json).collect::<Vec<_>>(),
                        native.0
                    );
                }
            }
        }
    }
    #[test]
    fn multiline_sha1_sha256_tokens_and_page_indices_match_native() {
        for format in [git2::ObjectFormat::Sha1, git2::ObjectFormat::Sha256] {
            let dir = tempfile::tempdir().unwrap();
            let mut options = git2::RepositoryInitOptions::new();
            options.object_format(format);
            let repo = Repository::init_opts(dir.path(), &options).unwrap();
            let oid = repo.blob(b"blob").unwrap();
            repo.reference(STASH, oid, false, "").unwrap();
            let path = repo.commondir().join("logs/refs/stash");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut bytes = Vec::new();
            for index in 0..400 {
                if index % 3 == 0 {
                    bytes.extend_from_slice(b"garbage line\n\n");
                }
                bytes.extend_from_slice(
                    format!("{oid} {oid} Name <mail> {} +0000", index - 200).as_bytes(),
                );
                if index % 7 != 0 {
                    bytes.push(b'\t');
                    bytes.extend_from_slice(format!("message {index}").as_bytes());
                    if index % 5 == 0 {
                        bytes.extend_from_slice(b"\0hidden");
                    }
                }
                if index != 399 {
                    bytes.push(b'\n');
                }
            }
            fs::write(&path, bytes).unwrap();
            let native = rows(log(&repo).unwrap().as_ref()).unwrap();
            let first = read(&repo, 0, 2).unwrap();
            let second = read(&repo, 2, 2).unwrap();
            assert_eq!(first.token, native.1);
            assert_eq!(second.token, native.1);
            assert_eq!(first.total, 400);
            assert_eq!(
                first.selected.iter().map(Row::json).collect::<Vec<_>>(),
                native.0[..2]
            );
            assert_eq!(
                second.selected.iter().map(Row::json).collect::<Vec<_>>(),
                native.0[2..4]
            );
            let captured = first.captured.unwrap();
            assert_eq!(
                captured
                    .rows(2, 2)
                    .unwrap()
                    .iter()
                    .map(Row::json)
                    .collect::<Vec<_>>(),
                native.0[2..4]
            );
            // A reference deletion does not change a captured log's bytes; old
            // read-only pages may finish, while future reads see an empty list.
            fs::remove_file(repo.commondir().join(STASH)).unwrap();
            assert_eq!(captured.rows(2, 2).unwrap().len(), 2);
            assert_eq!(read(&repo, 0, 2).unwrap().total, 0);
        }
    }
    #[test]
    fn oversized_log_and_entry_stay_bounded_and_cached_edits_expire() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let oid = repo.blob(b"blob").unwrap();
        repo.reference(STASH, oid, false, "").unwrap();
        let path = repo.commondir().join("logs/refs/stash");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = format!("{oid} {oid} Name <mail> 1 +0000\t").into_bytes();
        bytes.extend(std::iter::repeat_n(b'x', 17 * 1024 * 1024));
        bytes.push(b'\n');
        bytes.extend_from_slice(format!("{oid} {oid} Name <mail> 2 +0000\tnewest\n").as_bytes());
        fs::write(&path, &bytes).unwrap();
        drop(bytes);
        let native = repo.reflog(STASH).unwrap();
        let expected = list_token(Some(&native)).unwrap();
        drop(native);
        let streamed = read(&repo, 0, 1).unwrap();
        assert_eq!(streamed.total, 2);
        assert_eq!(streamed.token, expected);
        assert_eq!(streamed.selected[0].message, b"newest");
        let captured = streamed.captured.unwrap();
        assert!(captured.bytes < 1024);
        let older = captured.rows(1, 1).unwrap();
        assert_eq!(older[0].message.len(), 1024);
        assert!(older[0].truncated);
        let source = OpenOptions::new().write(true).open(&path).unwrap();
        source.write_all_at(b"y", 128).unwrap();
        assert!(matches!(captured.rows(1,1),Err(Error{code,..})if code=="SNAPSHOT_EXPIRED"));
    }
    #[test]
    fn non_regular_logs_are_rejected_and_missing_logs_remain_absent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let oid = repo.blob(b"blob").unwrap();
        repo.reference(STASH, oid, false, "").unwrap();
        let path = repo.commondir().join("logs/refs/stash");
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        assert_eq!(read(&repo, 0, 2).unwrap().total, 0);
        assert!(!path.exists());
        fs::create_dir_all(&path).unwrap();
        assert!(matches!(read(&repo,0,2),Err(Error{code,..})if code=="IO_ERROR"));
    }
}
