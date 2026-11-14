//! Reassemble byte-oriented Git diff lines before decoding text or selecting it.
use newport_protocol::git::LineFragment;

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
        piece: &LineFragment,
    ) -> Result<Option<Line>, String> {
        let index = number(piece.line_index)?;
        let offset = number(piece.byte_offset)?;
        let complete = piece.line_complete;
        let origin = piece.origin.as_str();
        let old = piece.old_line.map(number).transpose()?;
        let new = piece.new_line.map(number).transpose()?;
        let bytes = piece.content_bytes_b64.0.clone();
        if bytes.len() > 4096
            || (!complete && bytes.len() != 4096)
            || ![" ", "+", "-", "=", ">", "<"].contains(&origin)
        {
            return Err(INVALID.into());
        }
        let id = match piece.id.as_deref() {
            None => None,
            Some(id) if !id.is_empty() => Some(id),
            Some(_) => return Err(INVALID.into()),
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

fn number(value: u64) -> Result<u64, String> {
    (value <= 9_007_199_254_740_991)
        .then_some(value)
        .ok_or_else(|| INVALID.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git::Binary;

    fn piece(bytes: &[u8], offset: u64, complete: bool) -> LineFragment {
        LineFragment {
            line_index: 0,
            byte_offset: offset,
            line_complete: complete,
            origin: "+".into(),
            old_line: None,
            new_line: Some(1),
            content_bytes_b64: Binary(bytes.to_vec()),
            id: complete.then(|| "line-id".into()),
        }
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
                .push(0, 0, Some("hunk"), &piece(&[b'x'; 4096], 0, false))
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
        let first = piece(&[b'x'; 4096], 0, false);
        let edits: [fn(&mut LineFragment); 5] = [
            |p| p.line_index = 1,
            |p| p.byte_offset = 4095,
            |p| p.origin = "-".into(),
            |p| p.old_line = Some(1),
            |p| p.new_line = Some(2),
        ];
        for edit in edits {
            let mut lines = Lines::default();
            lines.push(0, 0, Some("hunk"), &first).unwrap();
            let mut last = piece(b"tail", 4096, true);
            edit(&mut last);
            assert!(lines.push(0, 0, Some("hunk"), &last).is_err());
        }
        let mut lines = Lines::default();
        lines.push(0, 0, Some("hunk"), &first).unwrap();
        assert!(
            lines
                .push(0, 0, Some("different"), &piece(b"tail", 4096, true))
                .is_err()
        );
        let mut early = first;
        early.id = Some("too-early".into());
        assert!(Lines::default().push(0, 0, Some("hunk"), &early).is_err());
    }

    #[test]
    fn repeated_and_skipped_complete_lines_are_not_rendered_as_valid() {
        for next in [0, 2] {
            let mut lines = Lines::default();
            let mut row = piece(b"line", 0, true);
            lines.push(0, 0, None, &row).unwrap();
            row.line_index = next;
            assert!(lines.push(0, 0, None, &row).is_err());
        }
    }
}
