//! Decode raw NUL-delimited deltas separately from patch text. Display paths
//! and patch headers are never used to select files for a mutation.
use super::super::{
    protocol::{DiffLineEncoding, Side},
    tokens::{CursorRef, EntryRef, SnapshotRef},
};
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::ffi::OsString;
fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn execute(repo: &Repo, args: &[OsString]) -> Result<Vec<u8>, Error> {
    command::run_os(&repo.root, args, vec![])?.ok_or_else(failure)
}
fn mode(bytes: &[u8]) -> Result<u32, Error> {
    u32::from_str_radix(std::str::from_utf8(bytes).map_err(|_| failure())?, 8)
        .map_err(|_| failure())
}
fn optional_oid(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.iter().all(|b| *b == b'0') {
        Ok(Value::Null)
    } else {
        oid(bytes)
    }
}
fn deltas(bytes: &[u8]) -> Result<Vec<Value>, Error> {
    let mut parts = bytes.split(|b| *b == 0).filter(|b| !b.is_empty());
    let mut rows = Vec::new();
    while let Some(header) = parts.next() {
        let fields = header
            .strip_prefix(b":")
            .ok_or_else(failure)?
            .split(|b| *b == b' ')
            .collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(failure());
        }
        let old = parts.next().ok_or_else(failure)?;
        let code = *fields[4].first().ok_or_else(failure)?;
        let new = if matches!(code, b'R' | b'C') {
            parts.next().ok_or_else(failure)?
        } else {
            old
        };
        rows.push(json!({"oldPath":WirePath::new(old),"newPath":WirePath::new(new),"oldOid":optional_oid(fields[2])?,"newOid":optional_oid(fields[3])?,"oldMode":mode(fields[0])?,"newMode":mode(fields[1])?,"status":match code{b'A'=>"Added",b'D'=>"Deleted",b'R'=>"Renamed",b'C'=>"Copied",b'T'=>"Typechange",b'U'=>"Conflicted",_=>"Modified"}}));
    }
    Ok(rows)
}
fn range(value: &[u8]) -> Result<(u64, u64), Error> {
    let text = std::str::from_utf8(value).map_err(|_| failure())?;
    let (mut start, mut count) = (text, "1");
    if let Some(parts) = text.split_once(',') {
        (start, count) = parts;
    }
    Ok((
        start.parse().map_err(|_| failure())?,
        count.parse().map_err(|_| failure())?,
    ))
}
fn patch(file: &mut Value, bytes: &[u8]) -> Result<(), Error> {
    let mut hunks = Vec::<Value>::new();
    let (mut old, mut new) = (0u64, 0u64);
    let (mut adds, mut dels) = (0, 0);
    let mut binary = false;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
            binary = true;
        }
        if line.starts_with(b"@@ ") {
            let fields = line.split(|b| *b == b' ').collect::<Vec<_>>();
            if fields.len() < 4 {
                return Err(failure());
            }
            let (os, oc) = range(fields[1].strip_prefix(b"-").ok_or_else(failure)?)?;
            let (ns, nc) = range(fields[2].strip_prefix(b"+").ok_or_else(failure)?)?;
            old = os;
            new = ns;
            hunks.push(json!({"id":null,"oldStart":os,"oldLines":oc,"newStart":ns,"newLines":nc,"lines":[]}));
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        let origin = match line.first() {
            Some(b' ') => " ",
            Some(b'+') => "+",
            Some(b'-') => "-",
            Some(b'\\') => "\\",
            _ => continue,
        };
        if origin == "\\" {
            if let Some(previous) = hunk["lines"].as_array_mut().ok_or_else(failure)?.last_mut() {
                let mut bytes = STANDARD
                    .decode(
                        previous["content"]["bytesB64"]
                            .as_str()
                            .ok_or_else(failure)?,
                    )
                    .map_err(|_| failure())?;
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                }
                previous["content"] = json!(WirePath::new(&bytes));
            }
            continue;
        }
        let old_line = (origin != "+").then_some(old);
        let new_line = (origin != "-").then_some(new);
        if old_line.is_some() {
            old += 1;
        }
        if new_line.is_some() {
            new += 1;
        }
        adds += usize::from(origin == "+");
        dels += usize::from(origin == "-");
        hunk["lines"].as_array_mut().ok_or_else(failure)?.push(json!({"id":null,"origin":origin,"oldLine":old_line,"newLine":new_line,"content":WirePath::new(&line[1..])}));
    }
    for hunk in &mut hunks {
        let id = pages::hash(&json!([file, hunk]))?;
        hunk["id"] = json!(id);
        for (ordinal, line) in hunk["lines"].as_array_mut().unwrap().iter_mut().enumerate() {
            if line["origin"] == "+" || line["origin"] == "-" {
                line["id"] = json!(pages::hash(&json!([id, ordinal, line]))?);
            }
        }
    }
    file["hunks"] = json!(hunks);
    file["binary"] = json!(binary);
    file["additions"] = json!(adds);
    file["deletions"] = json!(dels);
    Ok(())
}
fn read(
    repo: &Repo,
    base: Vec<OsString>,
    paths: &[Vec<u8>],
    context: u32,
) -> Result<Vec<Value>, Error> {
    if context > 100 {
        return Err(Error::invalid("contextLines exceeds 100."));
    }
    let common = strings(&[
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-abbrev",
        "--full-index",
        "--find-renames",
    ]);
    let mut raw = base.clone();
    raw.extend(common.clone());
    raw.extend(strings(&["--raw", "-z", "--"]));
    raw.extend(paths.iter().map(|p| OsString::from(OsStr::from_bytes(p))));
    let mut files = deltas(&execute(repo, &raw)?)?;
    if files.is_empty() {
        return Ok(files);
    }
    let mut command = base;
    command.extend(common);
    command.extend(strings(&["--patch", "--src-prefix=a/", "--dst-prefix=b/"]));
    command.push(format!("--unified={context}").into());
    command.push("--".into());
    command.extend(paths.iter().map(|p| OsString::from(OsStr::from_bytes(p))));
    let bytes = execute(repo, &command)?;
    let mut sections = Vec::new();
    let mut start = None;
    let mut pos = 0;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b"diff --git ") {
            if let Some(previous) = start {
                sections.push(&bytes[previous..pos]);
            }
            start = Some(pos);
        }
        pos += line.len();
    }
    if let Some(start) = start {
        sections.push(&bytes[start..]);
    }
    if sections.len() != files.len() {
        return Err(failure());
    }
    for (file, section) in files.iter_mut().zip(sections) {
        patch(file, section)?;
    }
    Ok(files)
}
fn comparison(repo: &Repo, id: &str, parent: usize) -> Result<(Vec<OsString>, Value), Error> {
    let commit = super::history::commit(repo, id, 1)?;
    let parents = commit["parents"].as_array().ok_or_else(failure)?;
    if (parents.is_empty() && parent != 0) || (!parents.is_empty() && parent >= parents.len()) {
        return Err(Error::invalid("Invalid parent index."));
    }
    let before = parents.get(parent).cloned().unwrap_or(Value::Null);
    let mut base = if let Some(before) = before["hex"].as_str() {
        strings(&["--literal-pathspecs", "diff", before, id])
    } else {
        strings(&[
            "--literal-pathspecs",
            "diff-tree",
            "-r",
            "--root",
            "--no-commit-id",
            id,
        ])
    };
    base.insert(0, format!("--attr-source={id}").into());
    Ok((
        base,
        json!({"truncated":false,"readOnly":true,"commitOid":commit["oid"],"parentOid":before,"parentIndex":if parents.is_empty(){None}else{Some(parent)},"parents":parents}),
    ))
}
pub(super) fn history(
    repo: &Repo,
    id: &str,
    parent: usize,
    path: Option<WirePath>,
    context: u32,
) -> Result<Value, Error> {
    let (base, mut metadata) = comparison(repo, id, parent)?;
    let paths = path
        .as_ref()
        .map(WirePath::decode)
        .transpose()?
        .into_iter()
        .collect::<Vec<_>>();
    let files = read(repo, base, &paths, context)?;
    metadata["files"] = json!(files);
    metadata["contextLines"] = json!(context);
    metadata["selectedPath"] = json!(path);
    Ok(metadata)
}
pub(super) fn working(
    repo: &Repo,
    source: &str,
    entry: &str,
    side: Side,
    context: u32,
) -> Result<Value, Error> {
    let snapshot = SnapshotRef::decode(source)?;
    let capture = status::capture(repo)?;
    if snapshot.r != repo.id
        || snapshot.q != "cli.status"
        || snapshot.f != capture.fingerprint
        || snapshot.p.is_some()
    {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The file changed. Refresh status.",
        ));
    }
    let selected = capture
        .rows
        .iter()
        .find(|r| r["entryId"] == entry)
        .ok_or_else(|| Error::new("STALE_ENTRY", "The changed file is no longer available."))?;
    let paths = EntryRef::decode(entry)?.paths()?;
    let mut base = strings(&["--literal-pathspecs", "diff"]);
    match side {
        Side::HeadToIndex => base.push("--cached".into()),
        Side::HeadToWorktree => {
            if capture.metadata["head"]["unborn"] != true {
                base.push("HEAD".into());
            }
        }
        Side::IndexToWorktree => (),
    };
    let mut files = read(repo, base, &paths, context)?;
    if selected["untracked"] == true && !matches!(side, Side::HeadToIndex) {
        let mut command = strings(&[
            "--literal-pathspecs",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ]);
        command.extend(paths.iter().map(|p| OsString::from(OsStr::from_bytes(p))));
        for path in execute(repo, &command)?
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
        {
            if files.len() >= 10000 {
                return Err(Error::new("LIMIT_EXCEEDED", "Too many files in this diff."));
            }
            let mut command = strings(&[
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ]);
            command.push(format!("--unified={context}").into());
            command.extend(strings(&["--", "/dev/null"]));
            command.push(OsString::from(OsStr::from_bytes(path)));
            let bytes = execute(repo, &command)?;
            let mut file = json!({"oldPath":WirePath::new(path),"newPath":WirePath::new(path),"oldMode":0,"newMode":33188,"oldOid":null,"newOid":null,"status":"Untracked"});
            patch(&mut file, &bytes)?;
            files.push(file);
        }
    }
    if status::capture(repo)?.fingerprint != snapshot.f {
        return Err(Error::new(
            "STALE_SNAPSHOT",
            "The file changed while reading its diff.",
        ));
    }
    Ok(
        json!({"files":files,"truncated":false,"readOnly":false,"sourceSnapshot":source,"entryId":entry,"side":side,"contextLines":context}),
    )
}
pub(super) fn commit_files(
    repo: &Repo,
    id: &str,
    parent: usize,
    count: usize,
    cursor: Option<String>,
) -> Result<Value, Error> {
    let (mut command, mut comparison) = comparison(repo, id, parent)?;
    command.extend(strings(&[
        "--no-ext-diff",
        "--no-textconv",
        "--no-abbrev",
        "--find-renames",
        "--raw",
        "-z",
        "--",
    ]));
    let rows = deltas(&execute(repo, &command)?)?;
    let fingerprint = pages::hash(&json!([rows, comparison]))?;
    comparison.as_object_mut().unwrap().remove("files");
    comparison["totalFiles"] = json!(rows.len());
    pages::page(
        repo,
        format!("cli.commit_files:{id}:{parent}"),
        fingerprint,
        rows,
        count,
        cursor,
        comparison,
    )
}
#[allow(clippy::too_many_arguments)]
pub(super) fn page(
    repo: &Repo,
    mut comparison: Value,
    query: String,
    count: usize,
    budget: Option<usize>,
    cursor: Option<String>,
    encoding: Option<DiffLineEncoding>,
) -> Result<Value, Error> {
    let budget = budget.unwrap_or(256 * 1024);
    if !(1..=5000).contains(&count) || !(4096..=524288).contains(&budget) {
        return Err(Error::invalid("Invalid diff page limits."));
    }
    let mut files = comparison["files"]
        .take()
        .as_array()
        .ok_or_else(failure)?
        .clone();
    comparison.as_object_mut().unwrap().remove("files");
    comparison.as_object_mut().unwrap().remove("truncated");
    let working = comparison["readOnly"] == false;
    for (file_index, file) in files.iter_mut().enumerate() {
        file["fileIndex"] = json!(file_index);
        file["omissionReason"] = if file["binary"] == true {
            json!("binary")
        } else {
            Value::Null
        };
        for (hunk_index, hunk) in file["hunks"]
            .as_array_mut()
            .ok_or_else(failure)?
            .iter_mut()
            .enumerate()
        {
            hunk["index"] = json!(hunk_index);
            let original = hunk["lines"].take().as_array().ok_or_else(failure)?.clone();
            hunk["totalLines"] = json!(original.len());
            let mut pieces = Vec::new();
            for (line_index, line) in original.iter().enumerate() {
                let bytes = STANDARD
                    .decode(line["content"]["bytesB64"].as_str().ok_or_else(failure)?)
                    .map_err(|_| failure())?;
                for offset in (0..bytes.len().max(1)).step_by(4096) {
                    let end = (offset + 4096).min(bytes.len());
                    let complete = end == bytes.len();
                    let mut piece = json!({"lineIndex":line_index,"byteOffset":offset,"lineComplete":complete,"origin":line["origin"],"oldLine":line["oldLine"],"newLine":line["newLine"],"contentBytesB64":STANDARD.encode(&bytes[offset..end])});
                    if working {
                        piece["id"] = if complete {
                            line["id"].clone()
                        } else {
                            Value::Null
                        };
                    }
                    pieces.push(piece);
                }
            }
            hunk["lines"] = json!(pieces);
        }
    }
    let mut units = Vec::new();
    for (f, file) in files.iter().enumerate() {
        let hunks = file["hunks"].as_array().ok_or_else(failure)?;
        if hunks.is_empty() {
            units.push((f, None));
        } else {
            for (h, hunk) in hunks.iter().enumerate() {
                for l in 0..hunk["lines"].as_array().ok_or_else(failure)?.len() {
                    units.push((f, Some((h, l))));
                }
            }
        }
    }
    comparison["totalUnits"] = json!(units.len());
    comparison["totalFiles"] = json!(files.len());
    comparison["hasOmissions"] = json!(files.iter().any(|f| f["binary"] == true));
    let snapshot = SnapshotRef {
        r: repo.id.clone(),
        q: query,
        f: pages::hash(&json!([files, comparison]))?,
        p: None,
    };
    let start = if let Some(cursor) = cursor {
        let c = CursorRef::decode(&cursor)?;
        if c.s.r != snapshot.r || c.s.q != snapshot.q || c.k.is_some() {
            return Err(Error::invalid("Cursor does not match this diff."));
        }
        if c.s.f != snapshot.f {
            return Err(Error::new("SNAPSHOT_EXPIRED", "The diff changed."));
        }
        c.o
    } else {
        0
    };
    if start > units.len() {
        return Err(Error::invalid("Cursor is past the end of this diff."));
    }
    let mut end = start.saturating_add(count).min(units.len());
    loop {
        let mut result = Vec::<Value>::new();
        let (mut previous_file, mut previous_hunk) = (None, None);
        for (f, unit) in &units[start..end] {
            if previous_file != Some(*f) {
                let mut row = files[*f].clone();
                row["hunks"] = json!([]);
                result.push(row);
                previous_file = Some(*f);
                previous_hunk = None;
            }
            if let Some((h, l)) = unit {
                let row = result.last_mut().unwrap();
                if previous_hunk != Some(*h) {
                    let mut hunk = files[*f]["hunks"][*h].clone();
                    hunk["lines"] = json!([]);
                    row["hunks"].as_array_mut().unwrap().push(hunk);
                    previous_hunk = Some(*h);
                }
                let mut line = files[*f]["hunks"][*h]["lines"][*l].clone();
                if encoding.is_some() {
                    let mut tuple = vec![
                        line["lineIndex"].clone(),
                        line["byteOffset"].clone(),
                        line["lineComplete"].clone(),
                        line["origin"].clone(),
                        line["oldLine"].clone(),
                        line["newLine"].clone(),
                        line["contentBytesB64"].clone(),
                    ];
                    if working {
                        tuple.push(line["id"].clone());
                    }
                    line = json!(tuple);
                }
                row["hunks"].as_array_mut().unwrap().last_mut().unwrap()["lines"]
                    .as_array_mut()
                    .unwrap()
                    .push(line);
            }
        }
        let next = (end < units.len()).then(|| {
            CursorRef {
                s: snapshot.clone(),
                o: end,
                k: None,
            }
            .encode()
        });
        let response = json!({"snapshot":snapshot.encode(),"entries":result,"nextCursor":next,"metadata":comparison});
        if serde_json::to_vec(&response).map_err(|_| failure())?.len() <= budget {
            return Ok(response);
        }
        if end <= start + 1 {
            return Err(Error::new(
                "LIMIT_EXCEEDED",
                "A diff line exceeds the requested page budget.",
            ));
        }
        end -= 1;
    }
}

/// Reconstruct a patch from current server-side lines, never client patch text.
pub(super) fn selection(
    repo: &Repo,
    source: &str,
    entry: &str,
    side: Side,
    selection: &super::super::protocol::HunkSelection,
    reverse: bool,
) -> Result<Vec<u8>, Error> {
    if selection.ids.is_empty() || selection.ids.len() > 1024 {
        return Err(Error::invalid("Select at least one hunk."));
    }
    let comparison = working(repo, source, entry, side, selection.context_lines)?;
    let selected = selection
        .ids
        .iter()
        .collect::<std::collections::HashSet<_>>();
    if selected.len() != selection.ids.len() {
        return Err(Error::invalid("Duplicate hunk selection."));
    }
    let requested = selection
        .lines
        .as_ref()
        .map(|v| v.iter().collect::<std::collections::HashSet<_>>());
    if requested.as_ref().is_some_and(|s| {
        s.is_empty() || selection.lines.as_ref().is_some_and(|v| s.len() != v.len())
    }) {
        return Err(Error::invalid("Invalid line selection."));
    }
    let mut seen_hunks = std::collections::HashSet::new();
    let mut seen_lines = std::collections::HashSet::new();
    let mut output = Vec::new();
    for file in comparison["files"].as_array().ok_or_else(failure)? {
        let path: WirePath =
            serde_json::from_value(file["newPath"].clone()).map_err(|_| failure())?;
        let path = path.decode()?;
        if file["binary"] == true || file["oldPath"] != file["newPath"] {
            return Err(Error::new(
                "UNSUPPORTED_CAPABILITY",
                "Partial edits require a text file without a rename.",
            ));
        }
        let quote = |prefix: &str| {
            format!(
                "\"{prefix}{}\"",
                path.iter()
                    .map(|b| format!("\\{b:03o}"))
                    .collect::<String>()
            )
        };
        let mut hunks = Vec::new();
        let mut after_lines = 0usize;
        for hunk in file["hunks"].as_array().ok_or_else(failure)? {
            let id = hunk["id"].as_str().ok_or_else(failure)?.to_string();
            if !selected.contains(&id) {
                continue;
            }
            seen_hunks.insert(id);
            let mut patch = Vec::new();
            let (mut old_count, mut new_count) = (0, 0);
            for line in hunk["lines"].as_array().ok_or_else(failure)? {
                let origin = line["origin"].as_str().ok_or_else(failure)?;
                let line_id = line["id"].as_str().map(str::to_owned);
                let wanted = requested
                    .as_ref()
                    .is_none_or(|ids| line_id.as_ref().is_some_and(|id| ids.contains(id)));
                if wanted && origin != " " {
                    if let Some(id) = line_id {
                        seen_lines.insert(id);
                    }
                }
                let mut origin = origin.as_bytes()[0];
                if reverse {
                    origin = match origin {
                        b'+' => b'-',
                        b'-' => b'+',
                        other => other,
                    };
                }
                if !wanted {
                    if origin == b'+' {
                        continue;
                    }
                    if origin == b'-' {
                        origin = b' ';
                    }
                }
                old_count += usize::from(origin != b'+');
                new_count += usize::from(origin != b'-');
                let content = STANDARD
                    .decode(line["content"]["bytesB64"].as_str().ok_or_else(failure)?)
                    .map_err(|_| failure())?;
                patch.push(origin);
                patch.extend(&content);
                if !content.ends_with(b"\n") {
                    patch.extend(b"\n\\ No newline at end of file\n");
                }
            }
            after_lines += new_count;
            let start = hunk[if reverse { "newStart" } else { "oldStart" }]
                .as_u64()
                .ok_or_else(failure)?;
            hunks.extend(
                format!(
                    "@@ -{start},{old_count} +{},{new_count} @@\n",
                    if new_count > 0 { start.max(1) } else { start }
                )
                .as_bytes(),
            );
            hunks.extend(patch);
        }
        if hunks.is_empty() {
            continue;
        }
        let old_mode = file[if reverse { "newMode" } else { "oldMode" }]
            .as_u64()
            .ok_or_else(failure)?;
        let mut new_mode = file[if reverse { "oldMode" } else { "newMode" }]
            .as_u64()
            .ok_or_else(failure)?;
        if new_mode == 0 && requested.is_some() && after_lines > 0 {
            new_mode = old_mode;
        }
        output.extend(format!("diff --git {} {}\n", quote("a/"), quote("b/")).as_bytes());
        if old_mode == 0 {
            output.extend(format!("new file mode {new_mode:o}\n").as_bytes());
        } else if new_mode == 0 {
            output.extend(format!("deleted file mode {old_mode:o}\n").as_bytes());
        }
        output.extend(
            format!(
                "--- {}\n+++ {}\n",
                if old_mode == 0 {
                    "/dev/null".into()
                } else {
                    quote("a/")
                },
                if new_mode == 0 {
                    "/dev/null".into()
                } else {
                    quote("b/")
                }
            )
            .as_bytes(),
        );
        output.extend(hunks);
    }
    if seen_hunks.len() != selected.len()
        || requested
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| !seen_lines.contains(*id)))
    {
        return Err(Error::new(
            "STALE_HUNK",
            "The selected diff lines changed. Reload the diff.",
        ));
    }
    Ok(output)
}
