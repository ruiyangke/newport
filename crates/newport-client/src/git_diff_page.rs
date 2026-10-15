//! Validate Git diff wire pages before publishing them to client state.
//! Mirrors the desktop response contract; accumulated page boundaries are separate.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

type Result<T> = std::result::Result<T, String>;
fn invalid(field: &str) -> String {
    format!("Invalid Git diff response ({field}). Refresh to retry.")
}
fn count(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or_else(|| invalid("count"))
}
fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| invalid("text"))
}
fn flag(value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(|| invalid("boolean"))
}
fn array(value: &Value, max: usize) -> Result<&[Value]> {
    value
        .as_array()
        .filter(|v| v.len() <= max)
        .map(Vec::as_slice)
        .ok_or_else(|| invalid("array"))
}
fn nullable<T>(
    value: Option<&Value>,
    parse: impl FnOnce(&Value) -> Result<T>,
) -> Result<Option<T>> {
    match value {
        Some(Value::Null) => Ok(None),
        Some(value) => parse(value).map(Some),
        None => Err(invalid("missing nullable field")),
    }
}
fn hex(value: &Value, len: usize) -> Result<()> {
    let value = text(value)?;
    if value.len() != len
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("identifier"));
    }
    Ok(())
}
fn oid(value: &Value) -> Result<()> {
    let len = match value
        .get("algorithm")
        .or_else(|| value.get("format"))
        .and_then(Value::as_str)
    {
        Some("sha1") => 40,
        Some("sha256") => 64,
        _ => return Err(invalid("object algorithm")),
    };
    hex(&value["hex"], len)
}
fn bytes(value: &Value) -> Result<()> {
    text(&value["display"])?;
    STANDARD
        .decode(text(&value["bytesB64"])?)
        .map_err(|_| invalid("base64"))?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Piece<'a> {
    index: u64,
    offset: u64,
    complete: bool,
    origin: &'a str,
    old: Option<u64>,
    new: Option<u64>,
    length: u64,
}
impl<'a> Piece<'a> {
    fn read(value: &'a Value, working: bool) -> Result<Self> {
        let encoded = text(&value["contentBytesB64"])?;
        if encoded.len() > 5464 {
            return Err(invalid("piece size"));
        }
        let length = STANDARD
            .decode(encoded)
            .map_err(|_| invalid("base64"))?
            .len() as u64;
        let complete = flag(&value["lineComplete"])?;
        let origin = text(&value["origin"])?;
        if length > 4096
            || (!complete && length != 4096)
            || !matches!(origin, " " | "+" | "-" | "=" | ">" | "<")
        {
            return Err(invalid("line piece"));
        }
        if working {
            let id = nullable(value.get("id"), |v| hex(v, 64))?;
            if (complete && matches!(origin, "+" | "-")) != id.is_some() {
                return Err(invalid("line selection ID"));
            }
        }
        Ok(Self {
            index: count(&value["lineIndex"])?,
            offset: count(&value["byteOffset"])?,
            complete,
            origin,
            old: nullable(value.get("oldLine"), count)?,
            new: nullable(value.get("newLine"), count)?,
            length,
        })
    }
    fn follows(self, before: Self) -> Result<()> {
        if self.index != before.index + u64::from(before.complete)
            || self.offset
                != if before.complete {
                    0
                } else {
                    before.offset + before.length
                }
            || (!before.complete
                && (self.origin != before.origin
                    || self.old != before.old
                    || self.new != before.new))
        {
            return Err(invalid("line continuation"));
        }
        Ok(())
    }
}

/// Accept object and tuple_v1 rows, normalizing tuples for all native consumers.
/// Called on individual wire responses, before they enter Resources or a cache.
pub fn decode(page: &mut Value, working: bool, from_start: bool) -> Result<()> {
    crate::git_page::validate(page)?;
    // Normalize only explicit tuples. Missing/invalid arrays remain errors below.
    for file in page["entries"].as_array_mut().expect("validated entries") {
        if let Some(hunks) = file.get_mut("hunks").and_then(Value::as_array_mut) {
            for hunk in hunks {
                if let Some(lines) = hunk.get_mut("lines").and_then(Value::as_array_mut) {
                    for line in lines {
                        if let Some(tuple) = line.as_array() {
                            if tuple.len() != if working { 8 } else { 7 } {
                                return Err(invalid("line tuple"));
                            }
                            let Value::Array(values) = std::mem::take(line) else {
                                unreachable!()
                            };
                            let fields = [
                                "lineIndex",
                                "byteOffset",
                                "lineComplete",
                                "origin",
                                "oldLine",
                                "newLine",
                                "contentBytesB64",
                                "id",
                            ];
                            *line = Value::Object(
                                fields
                                    .into_iter()
                                    .zip(values)
                                    .map(|(k, v)| (k.into(), v))
                                    .collect(),
                            );
                        }
                    }
                }
            }
        }
    }
    validate(page, working, from_start)
}

fn validate(page: &Value, working: bool, from_start: bool) -> Result<()> {
    let meta = &page["metadata"];
    if flag(&meta["readOnly"])? == working {
        return Err(invalid("comparison mode"));
    }
    if count(&meta["contextLines"])? > 100 {
        return Err(invalid("context lines"));
    }
    let total = count(&meta["totalUnits"])?;
    let omissions = flag(&meta["hasOmissions"])?;
    let total_files = if working {
        text(&meta["sourceSnapshot"])?;
        text(&meta["entryId"])?;
        if !matches!(
            text(&meta["side"])?,
            "head_to_index" | "index_to_worktree" | "head_to_worktree"
        ) {
            return Err(invalid("side"));
        }
        Some(count(&meta["totalFiles"])?)
    } else {
        oid(&meta["commitOid"])?;
        nullable(meta.get("parentOid"), oid)?;
        let index = nullable(meta.get("parentIndex"), count)?;
        let parents = array(&meta["parents"], 10000)?;
        for parent in parents {
            oid(parent)?;
        }
        if parents.is_empty() {
            if index.is_some() || !meta["parentOid"].is_null() {
                return Err(invalid("root parent"));
            }
        } else if index
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| parents.get(i))
            .is_none_or(|p| p["hex"] != meta["parentOid"]["hex"])
        {
            return Err(invalid("selected parent"));
        }
        bytes(&meta["selectedPath"])?;
        None
    };
    let files = array(&page["entries"], 200)?;
    let mut units = 0;
    let mut previous_file = None;
    let mut tail: Option<(Piece<'_>, Option<u64>)> = None;
    for (fi, file) in files.iter().enumerate() {
        if fi > 0 && tail.is_some_and(|(p, n)| !p.complete || n.is_some_and(|n| p.index + 1 != n)) {
            return Err(invalid("unfinished file"));
        }
        let file_index = count(&file["fileIndex"])?;
        if previous_file.is_some_and(|p| file_index <= p)
            || (working && from_start && file_index != fi as u64)
        {
            return Err(invalid("file order"));
        }
        previous_file = Some(file_index);
        nullable(file.get("oldPath"), bytes)?;
        nullable(file.get("newPath"), bytes)?;
        nullable(file.get("oldOid"), oid)?;
        nullable(file.get("newOid"), oid)?;
        count(&file["oldMode"])?;
        count(&file["newMode"])?;
        text(&file["status"])?;
        flag(&file["binary"])?;
        count(&file["additions"])?;
        count(&file["deletions"])?;
        let omitted = match file.get("omissionReason") {
            Some(Value::Null) => false,
            Some(Value::String(s)) if s == "binary" || s == "file_size_limit" => true,
            _ => return Err(invalid("omission reason")),
        };
        let hunks = array(&file["hunks"], 5000)?;
        if omitted && (!omissions || !hunks.is_empty()) {
            return Err(invalid("omitted content"));
        }
        if hunks.is_empty() {
            units += 1;
        }
        let mut previous_hunk = None;
        for (hi, hunk) in hunks.iter().enumerate() {
            if let Some((last, total_lines)) = tail
                && (!last.complete || total_lines.is_some_and(|n| last.index + 1 != n))
            {
                return Err(invalid("unfinished hunk"));
            }
            let index = count(&hunk["index"])?;
            if previous_hunk.is_some_and(|p| index != p + 1)
                || ((from_start || fi > 0) && hi == 0 && index != 0)
            {
                return Err(invalid("hunk order"));
            }
            previous_hunk = Some(index);
            for field in ["oldStart", "oldLines", "newStart", "newLines"] {
                count(&hunk[field])?;
            }
            let total_lines = if working {
                hex(&hunk["id"], 64)?;
                let n = count(&hunk["totalLines"])?;
                if n == 0 {
                    return Err(invalid("hunk length"));
                }
                Some(n)
            } else {
                None
            };
            let pieces = array(&hunk["lines"], 5000)?;
            if pieces.is_empty() {
                return Err(invalid("empty hunk"));
            }
            let mut previous_piece = None;
            for line in pieces {
                let piece = Piece::read(line, working)?;
                if total_lines.is_some_and(|n| piece.index >= n) {
                    return Err(invalid("hunk length"));
                }
                if let Some(before) = previous_piece {
                    piece.follows(before)?;
                } else if (from_start || fi > 0 || hi > 0)
                    && (piece.index != 0 || piece.offset != 0)
                {
                    return Err(invalid("hunk start"));
                }
                previous_piece = Some(piece);
                units += 1;
                if units > 5000 {
                    return Err(invalid("page units"));
                }
            }
            tail = previous_piece.map(|p| (p, total_lines));
        }
    }
    let more = page["nextCursor"].is_string();
    if units > 5000 || units > total || (more && units == 0) {
        return Err(invalid("page units"));
    }
    if !more && tail.is_some_and(|(p, n)| !p.complete || n.is_some_and(|n| p.index + 1 != n)) {
        return Err(invalid("incomplete final hunk"));
    }
    if from_start
        && ((more && units >= total)
            || (!more && units != total)
            || total_files
                .is_some_and(|n| files.len() as u64 > n || (!more && files.len() as u64 != n)))
    {
        return Err(invalid("diff completeness"));
    }
    Ok(())
}

/// Join validated pages into canonical files/hunks. Only the incoming bytes and
/// boundary pieces are decoded again; cumulative counts use array lengths. The
/// accepted page is borrowed and remains untouched on every failure.
pub fn merge(previous: &Value, mut incoming: Value, cursor: &str, working: bool) -> Result<Value> {
    crate::git_page::validate_continuation(previous, &incoming, cursor)?;
    decode(&mut incoming, working, false)?;
    let mut files = previous["entries"]
        .as_array()
        .ok_or_else(|| invalid("previous files"))?
        .clone();
    let Value::Array(fragments) = incoming["entries"].take() else {
        unreachable!()
    };
    if fragments.is_empty() {
        return Err(invalid("empty continuation"));
    }
    for mut fragment in fragments {
        let tail = files
            .last_mut()
            .ok_or_else(|| invalid("missing previous file"))?;
        let old_index = count(&tail["fileIndex"])?;
        let new_index = count(&fragment["fileIndex"])?;
        if new_index != old_index {
            if new_index < old_index || (working && new_index != old_index + 1) {
                return Err(invalid("file continuation order"));
            }
            if let Some(hunk) = tail["hunks"].as_array().and_then(|h| h.last()) {
                finished_hunk(hunk, working)?;
            }
            if let Some(hunk) = fragment["hunks"].as_array().and_then(|h| h.first()) {
                if count(&hunk["index"])? != 0 {
                    return Err(invalid("file hunk start"));
                }
                starts_hunk(hunk, working)?;
            }
            files.push(fragment);
            continue;
        }
        for field in [
            "fileIndex",
            "oldPath",
            "newPath",
            "oldOid",
            "newOid",
            "oldMode",
            "newMode",
            "status",
            "binary",
            "omissionReason",
            "additions",
            "deletions",
        ] {
            if tail[field] != fragment[field] {
                return Err(invalid("file continuation header"));
            }
        }
        let previous_hunks = tail
            .get_mut("hunks")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| invalid("previous hunks"))?;
        let Value::Array(hunks) = fragment["hunks"].take() else {
            unreachable!()
        };
        if previous_hunks.is_empty() || hunks.is_empty() {
            return Err(invalid("repeated empty file"));
        }
        for mut hunk in hunks {
            let last = previous_hunks.last_mut().expect("nonempty hunks");
            let old_index = count(&last["index"])?;
            let new_index = count(&hunk["index"])?;
            if new_index != old_index {
                if new_index != old_index + 1 {
                    return Err(invalid("hunk continuation order"));
                }
                finished_hunk(last, working)?;
                starts_hunk(&hunk, working)?;
                previous_hunks.push(hunk);
                continue;
            }
            for field in ["index", "oldStart", "oldLines", "newStart", "newLines"] {
                if last[field] != hunk[field] {
                    return Err(invalid("hunk continuation header"));
                }
            }
            if working && (last["id"] != hunk["id"] || last["totalLines"] != hunk["totalLines"]) {
                return Err(invalid("hunk continuation identity"));
            }
            let first = hunk["lines"]
                .as_array()
                .and_then(|l| l.first())
                .ok_or_else(|| invalid("empty hunk"))?;
            let before = last["lines"]
                .as_array()
                .and_then(|l| l.last())
                .ok_or_else(|| invalid("empty previous hunk"))?;
            Piece::read(first, working)?.follows(Piece::read(before, working)?)?;
            let Value::Array(mut pieces) = hunk["lines"].take() else {
                unreachable!()
            };
            last["lines"]
                .as_array_mut()
                .expect("validated previous lines")
                .append(&mut pieces);
        }
    }
    let loaded: u64 = files.iter().try_fold(0u64, |units, file| {
        let hunks = file["hunks"]
            .as_array()
            .ok_or_else(|| invalid("assembled hunks"))?;
        let count = if hunks.is_empty() {
            1
        } else {
            hunks.iter().try_fold(0u64, |n, h| {
                h["lines"]
                    .as_array()
                    .map(|l| n + l.len() as u64)
                    .ok_or_else(|| invalid("assembled lines"))
            })?
        };
        Ok::<_, String>(units + count)
    })?;
    let total = count(&incoming["metadata"]["totalUnits"])?;
    let more = incoming["nextCursor"].is_string();
    if loaded > total || (more && loaded >= total) || (!more && loaded != total) {
        return Err(invalid("cumulative diff units"));
    }
    if working {
        let total_files = count(&incoming["metadata"]["totalFiles"])?;
        if files.len() as u64 > total_files || (!more && files.len() as u64 != total_files) {
            return Err(invalid("cumulative diff files"));
        }
    }
    incoming["entries"] = Value::Array(files);
    Ok(incoming)
}

fn starts_hunk(hunk: &Value, working: bool) -> Result<()> {
    let first = hunk["lines"]
        .as_array()
        .and_then(|l| l.first())
        .ok_or_else(|| invalid("empty hunk"))?;
    let piece = Piece::read(first, working)?;
    if piece.index != 0 || piece.offset != 0 {
        return Err(invalid("hunk start"));
    }
    Ok(())
}

fn finished_hunk(hunk: &Value, working: bool) -> Result<()> {
    let last = hunk["lines"]
        .as_array()
        .and_then(|l| l.last())
        .ok_or_else(|| invalid("empty hunk"))?;
    let piece = Piece::read(last, working)?;
    if !piece.complete || (working && piece.index + 1 != count(&hunk["totalLines"])?) {
        return Err(invalid("unfinished hunk"));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn page(working: bool) -> Value {
        let path = json!({"display":"file","bytesB64":"ZmlsZQ=="});
        let oid = json!({"algorithm":"sha1","hex":"a".repeat(40)});
        let meta = if working {
            json!({"readOnly":false,"sourceSnapshot":"status","entryId":"file-id","side":"index_to_worktree","contextLines":3,"hasOmissions":false,"totalUnits":1,"totalFiles":1})
        } else {
            json!({"readOnly":true,"commitOid":oid,"parentOid":null,"parentIndex":null,"parents":[],"selectedPath":path,"contextLines":3,"hasOmissions":false,"totalUnits":1})
        };
        json!({"snapshot":"snapshot","nextCursor":null,"metadata":meta,"entries":[{
            "fileIndex":0,"oldPath":path,"newPath":path,"oldOid":null,"newOid":oid,
            "oldMode":33188,"newMode":33188,"status":"Modified","binary":false,"omissionReason":null,"additions":1,"deletions":0,
            "hunks":[{"id":if working {json!("b".repeat(64))} else {Value::Null},"index":0,"oldStart":0,"oldLines":0,"newStart":1,"newLines":1,"totalLines":1,
                "lines":[{"id":if working {json!("c".repeat(64))} else {Value::Null},"lineIndex":0,"byteOffset":0,"lineComplete":true,"origin":"+","oldLine":null,"newLine":1,"contentBytesB64":"eAo="}]}]}]})
    }

    #[test]
    fn object_and_tuple_pages_have_one_canonical_form() {
        for working in [true, false] {
            let mut object = page(working);
            decode(&mut object, working, true).unwrap();
            let mut tuple = object.clone();
            let line = &mut tuple["entries"][0]["hunks"][0]["lines"][0];
            let mut fields = vec![
                json!(0),
                json!(0),
                json!(true),
                json!("+"),
                Value::Null,
                json!(1),
                json!("eAo="),
            ];
            if working {
                fields.push(line["id"].clone());
            }
            *line = json!(fields);
            decode(&mut tuple, working, true).unwrap();
            if !working {
                object["entries"][0]["hunks"][0]["lines"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("id");
            }
            assert_eq!(object, tuple);
        }
    }

    #[test]
    fn malformed_shapes_and_missing_fields_never_become_empty_content() {
        for pointer in [
            "/entries/0",
            "/entries/0/hunks",
            "/entries/0/hunks/0",
            "/entries/0/hunks/0/lines",
            "/entries/0/hunks/0/lines/0",
            "/metadata",
        ] {
            for bad in [
                Value::Null,
                json!(false),
                json!(17),
                json!("bad"),
                json!([]),
                json!({}),
            ] {
                // A file with no text hunks can be a valid metadata-only delta.
                if pointer == "/entries/0/hunks" && bad == json!([]) {
                    continue;
                }
                let mut response = page(true);
                *response.pointer_mut(pointer).unwrap() = bad;
                assert!(decode(&mut response, true, true).is_err(), "{pointer}");
            }
        }
        for pointer in [
            "/metadata",
            "/entries/0",
            "/entries/0/hunks/0",
            "/entries/0/hunks/0/lines/0",
        ] {
            let keys: Vec<_> = page(true)
                .pointer(pointer)
                .unwrap()
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            for key in keys {
                let mut response = page(true);
                response
                    .pointer_mut(pointer)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(&key);
                assert!(
                    decode(&mut response, true, true).is_err(),
                    "missing {pointer}/{key}"
                );
            }
        }
    }

    #[test]
    fn wrong_mode_ids_counts_and_parent_comparisons_are_rejected() {
        for (pointer, bad) in [
            ("/metadata/readOnly", json!(true)),
            ("/metadata/totalUnits", json!(2)),
            ("/metadata/totalFiles", json!(2)),
            ("/metadata/contextLines", json!(101)),
            ("/metadata/side", json!("unknown")),
            ("/entries/0/fileIndex", json!(1)),
            ("/entries/0/hunks/0/index", json!(1)),
            ("/entries/0/hunks/0/totalLines", json!(2)),
            ("/entries/0/hunks/0/id", json!("bad")),
            ("/entries/0/hunks/0/lines/0/id", Value::Null),
            ("/entries/0/hunks/0/lines/0/contentBytesB64", json!("Zh==")),
            (
                "/entries/0/hunks/0/lines/0/lineIndex",
                json!(9007199254740992u64),
            ),
        ] {
            let mut response = page(true);
            *response.pointer_mut(pointer).unwrap() = bad;
            assert!(decode(&mut response, true, true).is_err(), "{pointer}");
        }
        let mut historical = page(false);
        historical["metadata"]["parents"] = json!([historical["metadata"]["commitOid"]]);
        assert!(decode(&mut historical, false, true).is_err());
        historical["metadata"]["parentIndex"] = json!(0);
        historical["metadata"]["parentOid"] = historical["metadata"]["commitOid"].clone();
        decode(&mut historical, false, true).unwrap();
    }

    #[test]
    fn partial_page_can_start_mid_line_but_initial_and_final_boundaries_are_checked() {
        let mut first = page(true);
        first["nextCursor"] = json!("next");
        first["metadata"]["totalUnits"] = json!(2);
        let line = &mut first["entries"][0]["hunks"][0]["lines"][0];
        line["contentBytesB64"] = json!(STANDARD.encode(vec![b'x'; 4096]));
        line["lineComplete"] = json!(false);
        line["id"] = Value::Null;
        decode(&mut first, true, true).unwrap();
        let mut last = page(true);
        last["metadata"]["totalUnits"] = json!(2);
        last["entries"][0]["hunks"][0]["lines"][0]["byteOffset"] = json!(4096);
        decode(&mut last, true, false).unwrap();
        assert!(decode(&mut last, true, true).is_err());
        first["nextCursor"] = Value::Null;
        assert!(decode(&mut first, true, true).is_err());
    }

    #[test]
    fn omissions_and_empty_comparisons_have_explicit_consistent_metadata() {
        let mut omitted = page(true);
        omitted["metadata"]["hasOmissions"] = json!(true);
        omitted["entries"][0]["hunks"] = json!([]);
        omitted["entries"][0]["omissionReason"] = json!("binary");
        omitted["entries"][0]["binary"] = json!(true);
        decode(&mut omitted, true, true).unwrap();
        omitted["metadata"]["hasOmissions"] = json!(false);
        assert!(decode(&mut omitted, true, true).is_err());
        let mut empty = page(true);
        empty["entries"] = json!([]);
        empty["metadata"]["totalUnits"] = json!(0);
        empty["metadata"]["totalFiles"] = json!(0);
        decode(&mut empty, true, true).unwrap();
        empty["nextCursor"] = json!("not-empty");
        assert!(decode(&mut empty, true, true).is_err());
    }

    #[test]
    fn duplicates_gaps_and_unfinished_lines_cannot_hide_between_hunks() {
        for index in [0, 2] {
            let mut response = page(true);
            response["metadata"]["totalUnits"] = json!(2);
            response["entries"][0]["hunks"][0]["totalLines"] = json!(3);
            let mut next = response["entries"][0]["hunks"][0]["lines"][0].clone();
            next["lineIndex"] = json!(index);
            response["entries"][0]["hunks"][0]["lines"]
                .as_array_mut()
                .unwrap()
                .push(next);
            assert!(decode(&mut response, true, true).is_err());
        }
        let mut response = page(true);
        response["metadata"]["totalUnits"] = json!(2);
        response["metadata"]["totalFiles"] = json!(2);
        let mut empty_file = response["entries"][0].clone();
        empty_file["fileIndex"] = json!(1);
        empty_file["hunks"] = json!([]);
        response["entries"][0]["hunks"][0]["totalLines"] = json!(2);
        response["entries"].as_array_mut().unwrap().push(empty_file);
        assert!(decode(&mut response, true, true).is_err());
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::{decode, merge, tests::page};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};

    fn pair(working: bool) -> (Value, Value) {
        let mut first = page(working);
        first["metadata"]["totalUnits"] = json!(2);
        first["entries"][0]["hunks"][0]["totalLines"] = json!(2);
        first["nextCursor"] = json!("next");
        let mut last = first.clone();
        last["nextCursor"] = Value::Null;
        last["entries"][0]["hunks"][0]["lines"][0]["lineIndex"] = json!(1);
        last["entries"][0]["hunks"][0]["lines"][0]["newLine"] = json!(2);
        (first, last)
    }

    #[test]
    fn repeated_file_and_hunk_headers_join_without_duplicate_rows() {
        for working in [true, false] {
            let (mut first, last) = pair(working);
            decode(&mut first, working, true).unwrap();
            let result = merge(&first, last, "next", working).unwrap();
            assert_eq!(result["entries"].as_array().unwrap().len(), 1);
            assert_eq!(result["entries"][0]["hunks"].as_array().unwrap().len(), 1);
            assert_eq!(
                result["entries"][0]["hunks"][0]["lines"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(
                first["entries"][0]["hunks"][0]["lines"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn mismatched_headers_and_discontinuous_pieces_preserve_the_previous_page() {
        for working in [true, false] {
            let (first, last) = pair(working);
            let original = first.clone();
            for (pointer, value) in [
                ("/entries/0/oldMode", json!(33261)),
                ("/entries/0/oldPath/display", json!("other")),
                ("/entries/0/additions", json!(8)),
                ("/entries/0/hunks/0/oldStart", json!(10)),
                ("/entries/0/hunks/0/lines/0/lineIndex", json!(0)),
                ("/entries/0/hunks/0/lines/0/byteOffset", json!(1)),
            ] {
                let mut changed = last.clone();
                *changed.pointer_mut(pointer).unwrap() = value;
                assert!(
                    merge(&first, changed, "next", working).is_err(),
                    "{pointer}"
                );
                assert_eq!(first, original);
            }
            assert!(merge(&first, last.clone(), "wrong", working).is_err());
            let mut changed = last;
            changed["metadata"]["contextLines"] = json!(10);
            assert!(merge(&first, changed, "next", working).is_err());
        }
    }

    #[test]
    fn final_piece_can_supply_an_id_but_not_change_the_continued_line() {
        let (mut first, mut last) = pair(true);
        for p in [&mut first, &mut last] {
            p["entries"][0]["hunks"][0]["totalLines"] = json!(1);
        }
        let before = &mut first["entries"][0]["hunks"][0]["lines"][0];
        before["id"] = Value::Null;
        before["lineComplete"] = json!(false);
        before["contentBytesB64"] = json!(STANDARD.encode(vec![b'x'; 4096]));
        let after = &mut last["entries"][0]["hunks"][0]["lines"][0];
        after["lineIndex"] = json!(0);
        after["newLine"] = json!(1);
        after["byteOffset"] = json!(4096);
        decode(&mut first, true, true).unwrap();
        let result = merge(&first, last.clone(), "next", true).unwrap();
        assert!(result["entries"][0]["hunks"][0]["lines"][1]["id"].is_string());
        for (field, value) in [
            ("origin", json!("-")),
            ("newLine", json!(3)),
            ("byteOffset", json!(8192)),
        ] {
            let mut changed = last.clone();
            changed["entries"][0]["hunks"][0]["lines"][0][field] = value;
            assert!(merge(&first, changed, "next", true).is_err());
        }
    }

    #[test]
    fn new_hunks_and_files_must_follow_finished_predecessors() {
        let (mut first, mut last) = pair(true);
        first["entries"][0]["hunks"][0]["totalLines"] = json!(1);
        last["entries"][0]["hunks"][0]["totalLines"] = json!(1);
        last["entries"][0]["hunks"][0]["index"] = json!(1);
        last["entries"][0]["hunks"][0]["id"] = json!("d".repeat(64));
        last["entries"][0]["hunks"][0]["lines"][0]["lineIndex"] = json!(0);
        assert_eq!(
            merge(&first, last.clone(), "next", true).unwrap()["entries"][0]["hunks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        first["entries"][0]["hunks"][0]["totalLines"] = json!(2);
        assert!(merge(&first, last.clone(), "next", true).is_err());
        first["entries"][0]["hunks"][0]["totalLines"] = json!(1);
        for p in [&mut first, &mut last] {
            p["metadata"]["totalFiles"] = json!(2);
        }
        last["entries"][0]["fileIndex"] = json!(1);
        // The next file cannot start at the second hunk.
        assert!(merge(&first, last.clone(), "next", true).is_err());
        last["entries"][0]["hunks"][0]["index"] = json!(0);
        assert_eq!(
            merge(&first, last.clone(), "next", true).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        last["entries"][0]["fileIndex"] = json!(2);
        assert!(merge(&first, last, "next", true).is_err());
    }

    #[test]
    fn cumulative_counts_reject_early_ends_and_spurious_continuations() {
        for working in [true, false] {
            let (first, mut last) = pair(working);
            last["nextCursor"] = json!("more");
            assert!(merge(&first, last, "next", working).is_err());
            let (mut first, mut last) = pair(working);
            for p in [&mut first, &mut last] {
                p["metadata"]["totalUnits"] = json!(3);
            }
            assert!(merge(&first, last.clone(), "next", working).is_err());
            last["entries"] = json!([]);
            assert!(merge(&first, last, "next", working).is_err());
        }
    }

    #[test]
    fn many_pages_can_accumulate_more_than_wire_limits() {
        let mut first = page(true);
        first["metadata"]["totalUnits"] = json!(400);
        first["metadata"]["totalFiles"] = json!(400);
        first["nextCursor"] = json!("next");
        let mut file = first["entries"][0].clone();
        file["hunks"] = json!([]);
        let files = |start| {
            Value::Array(
                (start..start + 200)
                    .map(|i| {
                        let mut f = file.clone();
                        f["fileIndex"] = json!(i);
                        f
                    })
                    .collect(),
            )
        };
        first["entries"] = files(0);
        let mut last = first.clone();
        last["nextCursor"] = Value::Null;
        last["entries"] = files(200);
        decode(&mut first, true, true).unwrap();
        assert_eq!(
            merge(&first, last, "next", true).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            400
        );
    }
}
