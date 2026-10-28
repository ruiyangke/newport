//! Selection groups follow displayed rows, including split-pane spacer boundaries.
use super::git_diff::Row;
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct Segment {
    pub(super) lines: Rc<Vec<String>>,
    pub(super) first: bool,
    pub(super) last: bool,
}

pub(super) fn group<'a>(rows: impl IntoIterator<Item = Option<&'a Row>>) -> Vec<Option<Segment>> {
    let rows: Vec<_> = rows.into_iter().collect();
    let mut result = vec![None; rows.len()];
    let mut start = 0;
    while start < rows.len() {
        let Some((hunk, _)) = rows[start].and_then(|r| r.selection.as_ref()) else {
            start += 1;
            continue;
        };
        let mut end = start;
        let mut lines = Vec::new();
        while let Some(Some(row)) = rows.get(end) {
            let Some((next_hunk, line)) = &row.selection else {
                break;
            };
            if next_hunk != hunk {
                break;
            }
            lines.push(line.clone());
            end += 1;
        }
        let lines = Rc::new(lines);
        for (index, entry) in result.iter_mut().enumerate().take(end).skip(start) {
            *entry = Some(Segment {
                lines: lines.clone(),
                first: index == start,
                last: index + 1 == end,
            });
        }
        start = end;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runs_stop_at_context_hunks_and_split_spacers() {
        let changed = |h: &str, l: &str| Row {
            selection: Some((h.into(), l.into())),
            ..Default::default()
        };
        let a = changed("h1", "remove");
        let b = changed("h1", "add");
        let c = changed("h2", "other");
        let context = Row::default();
        let unified = group([Some(&a), Some(&b), Some(&context), Some(&a), Some(&c)]);
        assert_eq!(*unified[0].as_ref().unwrap().lines, ["remove", "add"]);
        assert!(unified[0].as_ref().unwrap().first);
        assert!(!unified[0].as_ref().unwrap().last);
        assert!(unified[1].as_ref().unwrap().last);
        assert!(unified[2].is_none());
        assert_eq!(unified[3].as_ref().unwrap().lines.len(), 1);
        assert_eq!(unified[4].as_ref().unwrap().lines.len(), 1);
        let split = group([Some(&a), None, Some(&b)]);
        assert_eq!(*split[0].as_ref().unwrap().lines, ["remove"]);
        assert_eq!(*split[2].as_ref().unwrap().lines, ["add"]);
    }
}
