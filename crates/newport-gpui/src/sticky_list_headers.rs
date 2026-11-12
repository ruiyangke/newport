//! Pinned group headings for a virtualized uniform list. Header rows in the list
//! are spacers; this decoration renders each heading exactly once, including the
//! preceding group when its original row has scrolled out of the visible range.
use gpui_kit::{prelude::*, *};
use std::ops::Range;

type RenderHeader = Box<dyn Fn(usize, &mut Window, &mut App) -> AnyElement>;
pub struct StickyListHeaders {
    indices: Vec<usize>,
    render: RenderHeader,
}
impl StickyListHeaders {
    pub fn new(
        indices: Vec<usize>,
        render: impl Fn(usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        debug_assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
        Self {
            indices,
            render: Box::new(render),
        }
    }
}
fn positions(
    indices: &[usize],
    height: Pixels,
    offset: Pixels,
    viewport: Pixels,
    count: usize,
) -> Vec<(usize, Pixels)> {
    indices
        .iter()
        .enumerate()
        .filter_map(|(at, &index)| {
            let next = indices.get(at + 1).copied().unwrap_or(count);
            let y = (height * index + offset)
                .max(px(0.))
                .min(height * next + offset - height);
            (y + height > px(0.) && y < viewport).then_some((index, y))
        })
        .collect()
}
impl UniformListDecoration for StickyListHeaders {
    fn compute(
        &self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        offset: Point<Pixels>,
        height: Pixels,
        count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        div()
            .relative()
            .w(bounds.size.width)
            .h(bounds.size.height)
            .children(
                positions(&self.indices, height, offset.y, bounds.size.height, count)
                    .into_iter()
                    .map(|(index, y)| {
                        div()
                            .id(("sticky-list-heading", index))
                            .tab_group()
                            .tab_index(index as isize)
                            .tab_stop(false)
                            .absolute()
                            .top(y - offset.y)
                            .left(-offset.x)
                            .w_full()
                            .h(height)
                            .occlude()
                            .child((self.render)(index, window, cx))
                    })
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::positions;
    use gpui_kit::px;
    #[test]
    fn headings_pin_push_and_disappear_with_their_groups() {
        let at = |offset| positions(&[0, 4, 9], px(28.), px(offset), px(112.), 15);
        assert_eq!(at(0.), vec![(0, px(0.))]);
        assert_eq!(at(-42.), vec![(0, px(0.)), (4, px(70.))]);
        assert_eq!(at(-100.), vec![(0, px(-16.)), (4, px(12.))]);
        assert_eq!(at(-112.), vec![(4, px(0.))]);
        assert_eq!(at(-406.), vec![(9, px(-14.))]);
        assert!(at(-420.).is_empty());
        assert!(positions(&[], px(28.), px(0.), px(100.), 0).is_empty());
    }
}
