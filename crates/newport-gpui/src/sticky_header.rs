//! An in-flow header pinned to its nearest scroll viewport, without a second
//! scroller or duplicate controls. The body is clipped below it for both hit
//! testing and painting, while accessibility and Tab order stay header-first.
use gpui_kit::{prelude::*, *};

pub struct StickyHeader {
    header: AnyElement,
    body: AnyElement,
}

impl StickyHeader {
    pub fn new(header: impl IntoElement, body: impl IntoElement) -> Self {
        Self {
            header: header.into_any_element(),
            body: body.into_any_element(),
        }
    }
}

impl IntoElement for StickyHeader {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for StickyHeader {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ContentMask<Pixels>;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let header = self.header.request_layout(window, cx);
        let body = self.body.request_layout(window, cx);
        let style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.,
            size: size(relative(1.).into(), Length::Auto),
            ..Default::default()
        };
        (window.request_layout(style, [header, body], cx), header)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        header: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) -> ContentMask<Pixels> {
        let viewport = window.content_mask();
        let height = window.layout_bounds(*header).size.height;
        let offset = (viewport.bounds.top() - bounds.top())
            .max(px(0.))
            .min((bounds.size.height - height).max(px(0.)));
        window.with_element_offset(point(px(0.), offset), |window| {
            self.header.prepaint(window, cx);
        });
        let top = (bounds.top() + offset + height)
            .max(viewport.bounds.top())
            .min(viewport.bounds.bottom());
        let mask = ContentMask {
            bounds: Bounds::from_corners(
                point(viewport.bounds.left(), top),
                viewport.bounds.bottom_right(),
            ),
        };
        window.with_content_mask(Some(mask), |window| self.body.prepaint(window, cx));
        mask
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        mask: &mut ContentMask<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // GPUI registers tab stops during painting. Keep the natural order;
        // clipping already prevents the later body from covering the header.
        self.header.paint(window, cx);
        window.with_content_mask(Some(*mask), |window| self.body.paint(window, cx));
    }
}
