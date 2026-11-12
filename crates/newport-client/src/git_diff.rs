//! Reassemble byte-oriented Git diff lines before decoding text or selecting it.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

const INVALID: &str = "The agent returned an invalid diff piece.";
const CONTINUATION: &str = "Diff line continuation does not match.";

#[derive(Debug)]
pub struct Line {
    pub origin: String,
    pub old: Option<u64>,
    pub new: Option<u64>,
    pub bytes: Vec<u8>,
    pub selection: Option<(String, String)>,
}

struct Pending {
    key: (u64, u64, u64),
    hunk: Option<String>,
    line: Line,
}

/// Feed pieces in snapshot order, including across page/file fragments. An
/// incomplete line has no mutation ID; the agent supplies it on the final piece.
/// Discard the assembler after an error. Rendering and partial-line labels belong
/// to the consumer, not this transport decoder.
#[derive(Default)]
pub struct Lines {
    pending: Option<Pending>,
    last: Option<(u64, u64, u64)>,
}

impl Lines {
    pub fn push(
        &mut self,
        file: u64,
        hunk: u64,
        hunk_id: Option<&str>,
        piece: &Value,
    ) -> Result<Option<Line>, String> {
        let index = number(&piece["lineIndex"])?;
        let offset = number(&piece["byteOffset"])?;
        let complete = piece["lineComplete"].as_bool().ok_or(INVALID)?;
        let origin = piece["origin"].as_str().ok_or(INVALID)?;
        let old = nullable_number(piece.get("oldLine"))?;
        let new = nullable_number(piece.get("newLine"))?;
        let encoded = piece["contentBytesB64"]
            .as_str()
            .filter(|value| value.len() <= 5464)
            .ok_or(INVALID)?;
        let bytes = STANDARD.decode(encoded).map_err(|_| INVALID)?;
        if bytes.len() > 4096
            || (!complete && bytes.len() != 4096)
            || ![" ", "+", "-", "=", ">", "<"].contains(&origin)
        {
            return Err(INVALID.into());
        }
        let id = match piece.get("id") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if !id.is_empty() => Some(id.as_str()),
            _ => return Err(INVALID.into()),
        };
        if id.is_some() && (!complete || !matches!(origin, "+" | "-")) {
            return Err(INVALID.into());
        }
        let key = (file, hunk, index);
        let mut line = if let Some(mut pending) = self.pending.take() {
            if pending.key != key
                || pending.hunk.as_deref() != hunk_id
                || offset != pending.line.bytes.len() as u64
                || origin != pending.line.origin
                || old != pending.line.old
                || new != pending.line.new
            {
                return Err(CONTINUATION.into());
            }
            pending.line.bytes.extend(bytes);
            pending.line
        } else {
            if offset != 0 {
                return Err("Diff line starts in the middle of its content.".into());
            }
            if let Some((f, h, l)) = self.last {
                if file < f
                    || (file == f && hunk < h)
                    || (file == f && hunk == h && l.checked_add(1) != Some(index))
                    || ((file != f || hunk != h) && index != 0)
                {
                    return Err("Diff lines are out of order.".into());
                }
            } else if index != 0 {
                return Err("Diff starts in the middle of a hunk.".into());
            }
            Line {
                origin: origin.into(),
                old,
                new,
                bytes,
                selection: None,
            }
        };
        if complete {
            line.selection = hunk_id.zip(id).map(|(hunk, id)| (hunk.into(), id.into()));
            self.last = Some(key);
            Ok(Some(line))
        } else {
            self.pending = Some(Pending {
                key,
                hunk: hunk_id.map(str::to_owned),
                line,
            });
            Ok(None)
        }
    }

    /// Return a non-selectable partial line only if another page is available.
    pub fn finish(self, has_more: bool) -> Result<Option<Line>, String> {
        match self.pending {
            Some(pending) if has_more => Ok(Some(pending.line)),
            Some(_) => Err("Diff ended with an incomplete line.".into()),
            None => Ok(None),
        }
    }
}

fn number(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or_else(|| INVALID.into())
}

fn nullable_number(value: Option<&Value>) -> Result<Option<u64>, String> {
    match value {
        Some(Value::Null) => Ok(None),
        Some(value) => number(value).map(Some),
        None => Err(INVALID.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn piece(bytes: &[u8], offset: u64, complete: bool) -> Value {
        json!({"lineIndex":0,"byteOffset":offset,"lineComplete":complete,
            "origin":"+","oldLine":null,"newLine":1,
            "contentBytesB64":STANDARD.encode(bytes),
            "id":if complete { json!("line-id") } else { Value::Null }})
    }

    #[test]
    fn final_piece_supplies_selection_and_split_utf8_is_preserved() {
        let mut bytes = vec![b'x'; 4095];
        bytes.push(0xe2);
        let first = piece(&bytes, 0, false);
        let last = piece(&[0x82, 0xac, b'\n'], 4096, true);
        let mut lines = Lines::default();
        assert!(lines.push(0, 0, Some("hunk"), &first).unwrap().is_none());
        let line = lines.push(0, 0, Some("hunk"), &last).unwrap().unwrap();
        assert_eq!(
            String::from_utf8(line.bytes).unwrap(),
            format!("{}€\n", "x".repeat(4095))
        );
        assert_eq!(line.selection, Some(("hunk".into(), "line-id".into())));
        assert!(lines.finish(false).unwrap().is_none());
    }

    #[test]
    fn partial_lines_are_never_selectable_or_silently_complete() {
        for has_more in [false, true] {
            let mut lines = Lines::default();
            lines
                .push(0, 0, Some("hunk"), &piece(&vec![b'x'; 4096], 0, false))
                .unwrap();
            let result = lines.finish(has_more);
            if has_more {
                let line = result.unwrap().unwrap();
                assert_eq!(line.bytes.len(), 4096);
                assert!(line.selection.is_none());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[test]
    fn changed_fragment_identity_offsets_and_early_ids_are_rejected() {
        let first = piece(&vec![b'x'; 4096], 0, false);
        for (field, value) in [
            ("lineIndex", json!(1)),
            ("byteOffset", json!(4095)),
            ("origin", json!("-")),
            ("oldLine", json!(1)),
            ("newLine", json!(2)),
        ] {
            let mut lines = Lines::default();
            lines.push(0, 0, Some("hunk"), &first).unwrap();
            let mut last = piece(b"tail", 4096, true);
            last[field] = value;
            assert!(lines.push(0, 0, Some("hunk"), &last).is_err(), "{field}");
        }
        let mut lines = Lines::default();
        lines.push(0, 0, Some("hunk"), &first).unwrap();
        assert!(
            lines
                .push(0, 0, Some("different"), &piece(b"tail", 4096, true))
                .is_err()
        );
        let mut early = first;
        early["id"] = json!("too-early");
        assert!(Lines::default().push(0, 0, Some("hunk"), &early).is_err());
    }

    #[test]
    fn repeated_and_skipped_complete_lines_are_not_rendered_as_valid() {
        for next in [0, 2] {
            let mut lines = Lines::default();
            let mut row = piece(b"line", 0, true);
            lines.push(0, 0, None, &row).unwrap();
            row["lineIndex"] = json!(next);
            assert!(lines.push(0, 0, None, &row).is_err());
        }
    }
}
