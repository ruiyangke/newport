//! Pointer/keyboard range selection in display order, scoped to one diff.
use std::{
    cell::OnceCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
};

pub(super) type DisplayRows = Rc<Vec<(usize, String)>>;

#[derive(Clone, Copy)]
pub(super) enum Gesture {
    Begin { shift: bool },
    Extend,
    End,
    Activate { shift: bool },
}

#[derive(Default)]
pub(crate) struct Selection {
    lanes: Vec<Vec<String>>,
    indices: Vec<HashMap<String, usize>>,
    anchors: [Option<usize>; 3],
    drag: Option<Drag>,
    pub(super) display_rows: [OnceCell<DisplayRows>; 3],
    drag_epoch: u64,
    pending_frame: Option<u64>,
}
struct Drag {
    lane: usize,
    anchor: usize,
    end: usize,
    select: bool,
    before: BTreeSet<String>,
}
impl Selection {
    pub(super) fn active_lane(&self) -> Option<usize> {
        self.drag.as_ref().map(|drag| drag.lane)
    }
    pub(super) fn anchor_index(&self) -> Option<usize> {
        self.drag.as_ref().map(|drag| drag.anchor)
    }
    pub(super) fn reserve_frame(&mut self) -> Option<u64> {
        if self.drag.is_none() || self.pending_frame.is_some() {
            return None;
        }
        self.pending_frame = Some(self.drag_epoch);
        self.pending_frame
    }
    pub(super) fn take_frame(&mut self, epoch: u64) -> bool {
        if self.drag.is_none() || self.pending_frame != Some(epoch) || self.drag_epoch != epoch {
            return false;
        }
        self.pending_frame = None;
        true
    }
    fn invalidate_frame(&mut self) {
        self.drag_epoch = self.drag_epoch.wrapping_add(1);
        self.pending_frame = None;
    }
    pub(super) fn prepare(&mut self, rows: impl Iterator<Item = (String, bool)>) {
        if !self.lanes.is_empty() {
            return;
        }
        self.lanes = vec![vec![], vec![], vec![]];
        for (id, added) in rows {
            self.lanes[0].push(id.clone());
            self.lanes[if added { 2 } else { 1 }].push(id);
        }
        self.indices = self
            .lanes
            .iter()
            .map(|lane| {
                lane.iter()
                    .enumerate()
                    .map(|(i, id)| (id.clone(), i))
                    .collect()
            })
            .collect();
    }
    // Freeze toolbar geometry until pointer-up so the rows never jump under a drag.
    pub(super) fn toolbar_selection(&self) -> Option<&BTreeSet<String>> {
        self.drag.as_ref().map(|drag| &drag.before)
    }
    pub(crate) fn reset(&mut self) {
        self.invalidate_frame();
        self.anchors = [None; 3];
        self.drag = None;
    }
    pub(super) fn apply(
        &mut self,
        lane: usize,
        id: &str,
        gesture: Gesture,
        selected: &mut BTreeSet<String>,
    ) {
        if matches!(gesture, Gesture::End) {
            self.invalidate_frame();
            self.drag = None;
            return;
        }
        let Some(&index) = self.indices.get(lane).and_then(|indices| indices.get(id)) else {
            return;
        };
        match gesture {
            Gesture::Begin { shift } | Gesture::Activate { shift } => {
                self.invalidate_frame();
                let anchor = if shift {
                    self.anchors[lane].unwrap_or(index)
                } else {
                    index
                };
                let select = if shift && self.anchors[lane].is_some() {
                    selected.contains(&self.lanes[lane][anchor])
                } else {
                    !selected.contains(id)
                };
                if !shift || self.anchors[lane].is_none() {
                    self.anchors[lane] = Some(index);
                }
                self.drag = Some(Drag {
                    lane,
                    anchor,
                    end: index,
                    select,
                    before: selected.clone(),
                });
            }
            Gesture::Extend => {
                let Some(drag) = self.drag.as_mut().filter(|drag| drag.lane == lane) else {
                    return;
                };
                if drag.end == index {
                    return;
                }
                // Restore the old span before applying the new one, so reversing
                // direction retracts the gesture without losing prior selections.
                for id in &self.lanes[lane][drag.anchor.min(drag.end)..=drag.anchor.max(drag.end)] {
                    if drag.before.contains(id) {
                        selected.insert(id.clone());
                    } else {
                        selected.remove(id);
                    }
                }
                drag.end = index;
            }
            Gesture::End => unreachable!(),
        }
        let drag = self.drag.as_ref().unwrap();
        for id in &self.lanes[lane][drag.anchor.min(index)..=drag.anchor.max(index)] {
            if drag.select {
                selected.insert(id.clone());
            } else {
                selected.remove(id);
            }
        }
        if matches!(gesture, Gesture::Activate { .. }) {
            self.drag = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Gesture, Selection};
    use std::collections::BTreeSet;
    fn fixture() -> Selection {
        let mut state = Selection::default();
        state.prepare(
            [
                ("z", false),
                ("a", true),
                ("q", true),
                ("b", false),
                ("x", true),
            ]
            .into_iter()
            .map(|(id, added)| (id.into(), added)),
        );
        state
    }
    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|id| (*id).into()).collect()
    }
    #[test]
    fn only_one_frame_is_pending_and_old_frames_cannot_follow_a_new_drag() {
        let mut state = fixture();
        let mut selected = set(&[]);
        state.apply(0, "a", Gesture::Begin { shift: false }, &mut selected);
        let old = state.reserve_frame().unwrap();
        assert_eq!(state.reserve_frame(), None);
        state.apply(0, "a", Gesture::End, &mut selected);
        state.apply(0, "b", Gesture::Begin { shift: false }, &mut selected);
        let current = state.reserve_frame().unwrap();
        assert!(!state.take_frame(old));
        assert!(state.take_frame(current));
        assert!(!state.take_frame(current));
        let pending = state.reserve_frame().unwrap();
        state.reset();
        assert!(!state.take_frame(pending));
    }
    #[test]
    fn dragging_retracts_in_both_directions_and_preserves_other_selections() {
        let mut state = fixture();
        let mut selected = set(&["x"]);
        state.apply(0, "a", Gesture::Begin { shift: false }, &mut selected);
        state.apply(0, "b", Gesture::Extend, &mut selected);
        assert_eq!(selected, set(&["a", "q", "b", "x"]));
        state.apply(0, "z", Gesture::Extend, &mut selected);
        assert_eq!(selected, set(&["z", "a", "x"]));
        state.apply(0, "a", Gesture::End, &mut selected);
        state.apply(0, "b", Gesture::Extend, &mut selected);
        assert_eq!(selected, set(&["z", "a", "x"]));
    }
    #[test]
    fn shift_extends_anchor_and_dragging_selected_lines_removes_a_range() {
        let mut state = fixture();
        let mut selected = set(&[]);
        state.apply(0, "z", Gesture::Activate { shift: false }, &mut selected);
        state.apply(0, "b", Gesture::Activate { shift: true }, &mut selected);
        assert_eq!(selected, set(&["z", "a", "q", "b"]));
        state.apply(0, "a", Gesture::Begin { shift: false }, &mut selected);
        state.apply(0, "b", Gesture::Extend, &mut selected);
        assert_eq!(selected, set(&["z"]));
        state.apply(0, "q", Gesture::Extend, &mut selected);
        assert_eq!(selected, set(&["z", "b"]));
    }
    #[test]
    fn split_ranges_stay_in_their_pane_and_reset_forgets_anchors() {
        let mut state = fixture();
        let mut selected = set(&[]);
        state.apply(2, "a", Gesture::Begin { shift: false }, &mut selected);
        state.apply(1, "b", Gesture::Extend, &mut selected);
        assert_eq!(selected, set(&["a"]));
        state.apply(2, "x", Gesture::Extend, &mut selected);
        assert_eq!(selected, set(&["a", "q", "x"]));
        state.reset();
        selected.clear();
        state.apply(2, "x", Gesture::Activate { shift: true }, &mut selected);
        assert_eq!(selected, set(&["x"]));
    }
}
