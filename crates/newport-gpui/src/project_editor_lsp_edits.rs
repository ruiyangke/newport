//! Validate edits against the exact server snapshot before changing editor text.
use super::{Encoding, Rope, RopeExt};
use lsp_types::{Position, TextEdit};
use std::ops::Range;

#[derive(Debug)]
pub(super) struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// Unlike navigation, edits must never silently clamp a malformed position.
pub(super) fn offset(encoding: Encoding, text: &Rope, position: Position) -> Result<usize, String> {
    let row = position.line as usize;
    if row >= text.lines_len() {
        return Err("The server returned an invalid edit line.".into());
    }
    let start = text.line_start_offset(row);
    let line = text.slice_line(row).to_string();
    let mut units = 0;
    for (byte, ch) in line.char_indices() {
        if units == position.character {
            return Ok(start + byte);
        }
        units += match encoding {
            Encoding::Utf16 => ch.len_utf16() as u32,
            Encoding::Utf32 => 1,
        };
        if units > position.character {
            return Err("The server returned an edit inside a Unicode character.".into());
        }
    }
    if units == position.character {
        Ok(start + line.len())
    } else {
        Err("The server returned an invalid edit column.".into())
    }
}

pub(super) fn validate(
    encoding: Encoding,
    text: &Rope,
    edits: Vec<TextEdit>,
) -> Result<Vec<Edit>, String> {
    let mut edits = edits
        .into_iter()
        .map(|edit| {
            let start = offset(encoding, text, edit.range.start)?;
            let end = offset(encoding, text, edit.range.end)?;
            if start > end {
                return Err("The server returned a reversed edit.".into());
            }
            Ok(Edit {
                range: start..end,
                text: edit.new_text.replace("\r\n", "\n"),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    // Stable order matters for multiple insertions at the same position.
    edits.sort_by_key(|edit| edit.range.start);
    for pair in edits.windows(2) {
        if pair[0].range.end > pair[1].range.start {
            return Err("The server returned overlapping edits.".into());
        }
    }
    Ok(edits)
}

pub(super) fn apply(text: &Rope, edits: &[Edit]) -> Result<String, String> {
    let size = edits
        .iter()
        .try_fold(text.len(), |size, edit| {
            size.checked_sub(edit.range.len())
                .and_then(|size| size.checked_add(edit.text.len()))
        })
        .ok_or("The edits exceed the file size limit.")?;
    if size > newport_domain::text_file::EDIT_LIMIT {
        return Err("The edits would exceed the 16 MiB file limit.".into());
    }
    let mut result = String::with_capacity(size);
    let mut cursor = 0;
    for edit in edits {
        result.push_str(&text.slice(cursor..edit.range.start).to_string());
        result.push_str(&edit.text);
        cursor = edit.range.end;
    }
    result.push_str(&text.slice(cursor..).to_string());
    newport_domain::text_file::validate(&result).map_err(str::to_owned)?;
    Ok(result)
}

pub(super) fn map_offset(offset: usize, edits: &[Edit]) -> usize {
    let mut shift = 0isize;
    for edit in edits {
        if offset < edit.range.start {
            break;
        }
        if offset <= edit.range.end {
            return (edit.range.start as isize + shift) as usize + edit.text.len();
        }
        shift += edit.text.len() as isize - edit.range.len() as isize;
    }
    (offset as isize + shift) as usize
}

/// GPUI Kit accepts one completion edit. Coalesce disjoint edits atomically;
/// preserve the intended caret separately when an import follows the completion.
pub(super) fn completion(
    text: &Rope,
    primary: TextEdit,
    additional: Vec<TextEdit>,
) -> Result<(TextEdit, Rope, Position), String> {
    let caret = offset(Encoding::Utf32, text, primary.range.end)?;
    let mut all = additional;
    all.push(primary);
    let edits = validate(Encoding::Utf32, text, all)?;
    if edits
        .windows(2)
        .any(|pair| pair[0].range.start == pair[1].range.start)
    {
        return Err("The completion returned overlapping edits.".into());
    }
    let result = Rope::from(apply(text, &edits)?);
    let caret = result.offset_to_position(map_offset(caret, &edits));
    let first = edits
        .first()
        .ok_or("The completion has no edit.")?
        .range
        .start;
    let last = edits.last().unwrap().range.end;
    let tail = text.len() - last;
    let merged = TextEdit::new(
        lsp_types::Range::new(
            text.offset_to_position(first),
            text.offset_to_position(last),
        ),
        result.slice(first..result.len() - tail).to_string(),
    );
    Ok((merged, result, caret))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edit(start: u32, end: u32, value: &str) -> TextEdit {
        TextEdit::new(
            lsp_types::Range::new(Position::new(0, start), Position::new(0, end)),
            value.into(),
        )
    }
    #[test]
    fn edits_validate_unicode_overlap_and_original_coordinates() {
        let text = Rope::from("😀foo bar");
        assert!(validate(Encoding::Utf16, &text, vec![edit(1, 2, "x")]).is_err());
        assert!(
            validate(
                Encoding::Utf16,
                &text,
                vec![edit(2, 5, "x"), edit(4, 6, "y")]
            )
            .is_err()
        );
        assert!(validate(Encoding::Utf16, &text, vec![edit(99, 99, "x")]).is_err());
        let edits = validate(
            Encoding::Utf16,
            &text,
            vec![edit(6, 9, "longer"), edit(2, 5, "x")],
        )
        .unwrap();
        assert_eq!(apply(&text, &edits).unwrap(), "😀x longer");
    }
    #[test]
    fn coincident_inserts_keep_server_order_before_a_replacement() {
        let text = Rope::from("abc");
        let edits = validate(
            Encoding::Utf32,
            &text,
            vec![edit(0, 0, "one"), edit(0, 0, "two"), edit(0, 1, "three")],
        )
        .unwrap();
        assert_eq!(apply(&text, &edits).unwrap(), "onetwothreebc");
        assert!(
            validate(
                Encoding::Utf32,
                &text,
                vec![edit(0, 1, "x"), edit(0, 0, "y")]
            )
            .is_err()
        );
        assert!(completion(&text, edit(0, 0, "x"), vec![edit(0, 0, "y")]).is_err());
    }

    #[test]
    fn completion_imports_on_both_sides_preserve_the_primary_caret() {
        let text = Rope::from("pri\n");
        let import = TextEdit::new(
            lsp_types::Range::new(Position::new(1, 0), Position::new(1, 0)),
            "import x\n".into(),
        );
        let (merged, result, caret) = completion(&text, edit(0, 3, "print"), vec![import]).unwrap();
        assert_eq!(result.to_string(), "print\nimport x\n");
        assert_eq!(caret, Position::new(0, 5));
        assert_eq!(merged.new_text, "print\nimport x\n");
        let text = Rope::from("\npri");
        let main = TextEdit::new(
            lsp_types::Range::new(Position::new(1, 0), Position::new(1, 3)),
            "print".into(),
        );
        let (_, result, caret) = completion(&text, main, vec![edit(0, 0, "import x\n")]).unwrap();
        assert_eq!(result.to_string(), "import x\n\nprint");
        assert_eq!(caret, Position::new(2, 5));
    }
}
