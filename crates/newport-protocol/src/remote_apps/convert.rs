//! The messages as their generated wire types and back. Going to the wire
//! can't fail; coming from it fails for a missing field, a value out of its
//! Rust type's range or an unknown enumeration value. The messages' own
//! `validate` checks the rest.
use crate::encoding::protobuf::Present;
use crate::encoding::protobuf::generated::newport::remote::{apps as pb, display as pd};
use crate::remote_apps::{
    audio, display, drag, features, file_tree, file_upload, performance, reverse_drag,
    reverse_files, session_activity, session_application_lifecycle as lifecycle,
    session_applications, session_launch, sizing, text_input, tile, v1, window_application,
};

type Result<T> = std::result::Result<T, String>;

fn required<T>(value: Option<T>, what: &str) -> Result<T> {
    value.ok_or_else(|| format!("Missing {what}"))
}
fn narrow<T: TryFrom<u32>>(value: u32, what: &str) -> Result<T> {
    T::try_from(value).map_err(|_| format!("Invalid {what}"))
}
fn enumeration<T: TryFrom<i32>>(value: i32, what: &str) -> Result<T> {
    T::try_from(value).map_err(|_| format!("Unknown {what}"))
}
fn size(value: [u32; 2]) -> pb::Size {
    pb::Size {
        width: value[0],
        height: value[1],
    }
}
fn from_size(value: pb::Size) -> [u32; 2] {
    [value.width, value.height]
}
fn point(value: [i32; 2]) -> pb::Point {
    pb::Point {
        x: value[0],
        y: value[1],
    }
}
fn from_point(value: pb::Point) -> [i32; 2] {
    [value.x, value.y]
}
fn location(value: [u32; 2]) -> pb::Location {
    pb::Location {
        x: value[0],
        y: value[1],
    }
}
fn from_location(value: Option<pb::Location>) -> Result<[u32; 2]> {
    let value = required(value, "point")?;
    Ok([value.x, value.y])
}
fn rect(value: [u32; 4]) -> pb::Rect {
    pb::Rect {
        x: value[0],
        y: value[1],
        width: value[2],
        height: value[3],
    }
}
fn from_rect(value: pb::Rect) -> [u32; 4] {
    [value.x, value.y, value.width, value.height]
}
fn window(value: Option<pb::WindowId>) -> Result<v1::WindowId> {
    required(value, "window").map(Into::into)
}

impl From<v1::WindowId> for pb::WindowId {
    fn from(value: v1::WindowId) -> Self {
        Self {
            id: value.id,
            generation: value.generation,
        }
    }
}
impl From<pb::WindowId> for v1::WindowId {
    fn from(value: pb::WindowId) -> Self {
        Self {
            id: value.id,
            generation: value.generation,
        }
    }
}
impl From<v1::Scale> for pb::Scale {
    fn from(value: v1::Scale) -> Self {
        Self {
            numerator: value.numerator.into(),
            denominator: value.denominator.into(),
        }
    }
}
impl TryFrom<Option<pb::Scale>> for v1::Scale {
    type Error = String;
    fn try_from(value: Option<pb::Scale>) -> Result<Self> {
        let value = required(value, "scale")?;
        Ok(Self {
            numerator: narrow(value.numerator, "scale")?,
            denominator: narrow(value.denominator, "scale")?,
        })
    }
}
impl From<v1::Surface> for pb::Surface {
    fn from(value: v1::Surface) -> Self {
        Self {
            generation: value.generation,
            logical_width: value.logical_width,
            logical_height: value.logical_height,
            pixel_width: value.pixel_width,
            pixel_height: value.pixel_height,
            scale: Some(value.scale.into()),
        }
    }
}
impl TryFrom<Option<pb::Surface>> for v1::Surface {
    type Error = String;
    fn try_from(value: Option<pb::Surface>) -> Result<Self> {
        let value = required(value, "surface")?;
        Ok(Self {
            generation: value.generation,
            logical_width: value.logical_width,
            logical_height: value.logical_height,
            pixel_width: value.pixel_width,
            pixel_height: value.pixel_height,
            scale: value.scale.try_into()?,
        })
    }
}
impl From<v1::MediaTime> for pb::MediaTime {
    fn from(value: v1::MediaTime) -> Self {
        Self {
            earliest_us: value.earliest_us,
            latest_us: value.latest_us,
        }
    }
}
impl From<pb::MediaTime> for v1::MediaTime {
    fn from(value: pb::MediaTime) -> Self {
        Self {
            earliest_us: value.earliest_us,
            latest_us: value.latest_us,
        }
    }
}
impl From<&v1::FrameDiagnostics> for pb::FrameDiagnostics {
    fn from(value: &v1::FrameDiagnostics) -> Self {
        Self {
            target_fps: value.target_fps,
            fast_codec: value.fast_codec,
            ack_ms: value.ack_ms,
            work_ms: value.work_ms,
            capture_ms: value.capture_ms,
            dispatch_ms: value.dispatch_ms,
        }
    }
}
impl From<pb::FrameDiagnostics> for v1::FrameDiagnostics {
    fn from(value: pb::FrameDiagnostics) -> Self {
        Self {
            target_fps: value.target_fps,
            fast_codec: value.fast_codec,
            ack_ms: value.ack_ms,
            work_ms: value.work_ms,
            capture_ms: value.capture_ms,
            dispatch_ms: value.dispatch_ms,
        }
    }
}

impl From<v1::WindowKind> for pb::WindowKind {
    fn from(value: v1::WindowKind) -> Self {
        match value {
            v1::WindowKind::Normal => Self::Normal,
            v1::WindowKind::Dialog => Self::Dialog,
            v1::WindowKind::Menu => Self::Menu,
            v1::WindowKind::Tooltip => Self::Tooltip,
        }
    }
}
impl From<pb::WindowKind> for v1::WindowKind {
    fn from(value: pb::WindowKind) -> Self {
        match value {
            pb::WindowKind::Normal => Self::Normal,
            pb::WindowKind::Dialog => Self::Dialog,
            pb::WindowKind::Menu => Self::Menu,
            pb::WindowKind::Tooltip => Self::Tooltip,
        }
    }
}
impl From<v1::Decoration> for pb::Decoration {
    fn from(value: v1::Decoration) -> Self {
        match value {
            v1::Decoration::Client => Self::Client,
            v1::Decoration::Server => Self::Server,
            v1::Decoration::None => Self::None,
        }
    }
}
impl From<pb::Decoration> for v1::Decoration {
    fn from(value: pb::Decoration) -> Self {
        match value {
            pb::Decoration::Client => Self::Client,
            pb::Decoration::Server => Self::Server,
            pb::Decoration::None => Self::None,
        }
    }
}
impl From<v1::Actions> for pb::Actions {
    fn from(value: v1::Actions) -> Self {
        Self {
            close: value.close,
            minimize: value.minimize,
            maximize: value.maximize,
            resize: value.resize,
            fullscreen: value.fullscreen,
        }
    }
}
impl From<pb::Actions> for v1::Actions {
    fn from(value: pb::Actions) -> Self {
        Self {
            close: value.close,
            minimize: value.minimize,
            maximize: value.maximize,
            resize: value.resize,
            fullscreen: value.fullscreen,
        }
    }
}
impl From<sizing::PixelConstraints> for pb::PixelConstraints {
    fn from(value: sizing::PixelConstraints) -> Self {
        Self {
            min: value.min.map(size),
            max: value.max.map(size),
            base: value.base.map(size),
            increment: value.increment.map(size),
            aspect: value.aspect.map(|[min, max]| pb::AspectRange {
                min: Some(size(min)),
                max: Some(size(max)),
            }),
        }
    }
}
impl TryFrom<pb::PixelConstraints> for sizing::PixelConstraints {
    type Error = String;
    fn try_from(value: pb::PixelConstraints) -> Result<Self> {
        Ok(Self {
            min: value.min.map(from_size),
            max: value.max.map(from_size),
            base: value.base.map(from_size),
            increment: value.increment.map(from_size),
            aspect: value
                .aspect
                .map(|aspect| {
                    Ok::<_, String>([
                        from_size(required(aspect.min, "aspect")?),
                        from_size(required(aspect.max, "aspect")?),
                    ])
                })
                .transpose()?,
        })
    }
}
impl From<v1::Constraints> for pb::Constraints {
    fn from(value: v1::Constraints) -> Self {
        Self {
            physical: value.physical.map(Into::into),
            min: value.min.map(size),
            max: value.max.map(size),
        }
    }
}
impl TryFrom<pb::Constraints> for v1::Constraints {
    type Error = String;
    fn try_from(value: pb::Constraints) -> Result<Self> {
        Ok(Self {
            physical: value.physical.map(TryInto::try_into).transpose()?,
            min: value.min.map(from_size),
            max: value.max.map(from_size),
        })
    }
}
impl From<&window_application::WindowApplication> for pb::WindowApplication {
    fn from(value: &window_application::WindowApplication) -> Self {
        Self {
            app_id: value.app_id.clone(),
            desktop_id: value.desktop_id.clone(),
            request: value.request,
        }
    }
}
impl From<&v1::Window> for pb::Window {
    fn from(value: &v1::Window) -> Self {
        Self {
            key: Some(value.key.into()),
            parent: value.parent.map(Into::into),
            kind: pb::WindowKind::from(value.kind).into(),
            modal: value.modal,
            position: Some(point(value.position)),
            title: value.title.clone(),
            decoration: pb::Decoration::from(value.decoration).into(),
            actions: Some(value.actions.into()),
            constraints: Some(value.constraints.into()),
            minimized: value.minimized,
            maximized: value.maximized,
            fullscreen: value.fullscreen,
            application: value.application.as_ref().map(Into::into),
        }
    }
}
impl TryFrom<Option<pb::Window>> for v1::Window {
    type Error = String;
    fn try_from(value: Option<pb::Window>) -> Result<Self> {
        let value = required(value, "window")?;
        Ok(Self {
            key: required(value.key, "window identity")?.into(),
            parent: value.parent.map(Into::into),
            kind: enumeration::<pb::WindowKind>(value.kind, "window kind")?.into(),
            modal: value.modal,
            position: from_point(value.position.unwrap_or_default()),
            title: value.title,
            decoration: enumeration::<pb::Decoration>(value.decoration, "window decoration")?
                .into(),
            actions: value.actions.unwrap_or_default().into(),
            constraints: value.constraints.unwrap_or_default().try_into()?,
            minimized: value.minimized,
            maximized: value.maximized,
            fullscreen: value.fullscreen,
            // A hint: an owner with nothing known is no owner.
            application: value.application.and_then(|application| {
                window_application::WindowApplication::new(
                    application.app_id,
                    application.desktop_id,
                    application.request,
                )
            }),
        })
    }
}

impl From<tile::Codec> for pb::Codec {
    fn from(value: tile::Codec) -> Self {
        match value {
            tile::Codec::Solid => Self::Solid,
            tile::Codec::RawBgra => Self::RawBgra,
            tile::Codec::Lz4 => Self::Lz4,
            tile::Codec::Zlib => Self::Zlib,
            tile::Codec::Png => Self::Png,
            tile::Codec::Jpeg => Self::Jpeg,
            tile::Codec::Copy => Self::Copy,
            tile::Codec::Av1 => Self::Av1,
        }
    }
}
impl From<pb::Codec> for tile::Codec {
    fn from(value: pb::Codec) -> Self {
        match value {
            pb::Codec::Solid => Self::Solid,
            pb::Codec::RawBgra => Self::RawBgra,
            pb::Codec::Lz4 => Self::Lz4,
            pb::Codec::Zlib => Self::Zlib,
            pb::Codec::Png => Self::Png,
            pb::Codec::Jpeg => Self::Jpeg,
            pb::Codec::Copy => Self::Copy,
            pb::Codec::Av1 => Self::Av1,
        }
    }
}
impl From<&tile::Tile> for pb::Tile {
    fn from(value: &tile::Tile) -> Self {
        Self {
            id: value.id,
            codec: pb::Codec::from(value.codec).into(),
            data: value.data.clone(),
            rect: value.rect.map(|r| rect(r.map(u32::from))),
            copy: value.copy.map(|copy| pb::TileCopy {
                source: Some(match copy.source {
                    tile::CopySource::Surface { source_y } => {
                        pb::tile_copy::Source::SourceY(source_y)
                    }
                    tile::CopySource::Tile { sy } => pb::tile_copy::Source::Sy(sy.into()),
                }),
                dy: copy.dy.into(),
                h: copy.h.into(),
            }),
            video: value.video.map(|video| pb::VideoGroup {
                rect: Some(rect(video.rect)),
                stream: video.stream,
                sequence: video.sequence,
                keyframe: video.keyframe,
            }),
        }
    }
}
impl TryFrom<pb::Tile> for tile::Tile {
    type Error = String;
    fn try_from(value: pb::Tile) -> Result<Self> {
        let patch = |value: u32| narrow::<u16>(value, "tile patch");
        Ok(Self {
            id: value.id,
            codec: enumeration::<pb::Codec>(value.codec, "tile codec")?.into(),
            data: value.data,
            rect: value
                .rect
                .map(|r| {
                    Ok::<_, String>([patch(r.x)?, patch(r.y)?, patch(r.width)?, patch(r.height)?])
                })
                .transpose()?,
            copy: value
                .copy
                .map(|copy| {
                    Ok::<_, String>(tile::TileCopy {
                        source: match required(copy.source, "copy source")? {
                            pb::tile_copy::Source::SourceY(source_y) => {
                                tile::CopySource::Surface { source_y }
                            }
                            pb::tile_copy::Source::Sy(sy) => tile::CopySource::Tile {
                                sy: narrow(sy, "tile copy")?,
                            },
                        },
                        dy: narrow(copy.dy, "tile copy")?,
                        h: narrow(copy.h, "tile copy")?,
                    })
                })
                .transpose()?,
            video: value
                .video
                .map(|video| {
                    Ok::<_, String>(tile::VideoGroup {
                        rect: from_rect(required(video.rect, "video rectangle")?),
                        stream: video.stream,
                        sequence: video.sequence,
                        keyframe: video.keyframe,
                    })
                })
                .transpose()?,
        })
    }
}

impl From<v1::ResizeEdge> for pb::ResizeEdge {
    fn from(value: v1::ResizeEdge) -> Self {
        match value {
            v1::ResizeEdge::Top => Self::Top,
            v1::ResizeEdge::Bottom => Self::Bottom,
            v1::ResizeEdge::Left => Self::Left,
            v1::ResizeEdge::Right => Self::Right,
            v1::ResizeEdge::TopLeft => Self::TopLeft,
            v1::ResizeEdge::TopRight => Self::TopRight,
            v1::ResizeEdge::BottomLeft => Self::BottomLeft,
            v1::ResizeEdge::BottomRight => Self::BottomRight,
        }
    }
}
impl From<pb::ResizeEdge> for v1::ResizeEdge {
    fn from(value: pb::ResizeEdge) -> Self {
        match value {
            pb::ResizeEdge::Top => Self::Top,
            pb::ResizeEdge::Bottom => Self::Bottom,
            pb::ResizeEdge::Left => Self::Left,
            pb::ResizeEdge::Right => Self::Right,
            pb::ResizeEdge::TopLeft => Self::TopLeft,
            pb::ResizeEdge::TopRight => Self::TopRight,
            pb::ResizeEdge::BottomLeft => Self::BottomLeft,
            pb::ResizeEdge::BottomRight => Self::BottomRight,
        }
    }
}
impl From<display::DisplayProfile> for pb::DisplayProfile {
    fn from(value: display::DisplayProfile) -> Self {
        Self {
            scale: Some(value.scale.into()),
            work_area: Some(size(value.work_area)),
        }
    }
}
impl TryFrom<pb::DisplayProfile> for display::DisplayProfile {
    type Error = String;
    fn try_from(value: pb::DisplayProfile) -> Result<Self> {
        Ok(Self {
            scale: value.scale.try_into()?,
            work_area: from_size(required(value.work_area, "work area")?),
        })
    }
}
impl From<display::PopupBounds> for pb::PopupBounds {
    fn from(value: display::PopupBounds) -> Self {
        Self {
            origin: Some(point(value.origin)),
            size: Some(size(value.size)),
        }
    }
}
impl TryFrom<pb::PopupBounds> for display::PopupBounds {
    type Error = String;
    fn try_from(value: pb::PopupBounds) -> Result<Self> {
        Ok(Self {
            origin: from_point(required(value.origin, "popup origin")?),
            size: from_size(required(value.size, "popup size")?),
        })
    }
}
impl From<&v1::Operation> for pb::Operation {
    fn from(value: &v1::Operation) -> Self {
        use pb::operation::Action;
        let action = match *value {
            v1::Operation::Activate => Action::Activate(Present {}),
            v1::Operation::Move => Action::Move(Present {}),
            v1::Operation::Close => Action::Close(Present {}),
            v1::Operation::Minimize => Action::Minimize(Present {}),
            v1::Operation::Maximize => Action::Maximize(Present {}),
            v1::Operation::Restore => Action::Restore(Present {}),
            v1::Operation::Fullscreen { enabled } => Action::Fullscreen(enabled),
            v1::Operation::ShowWindowMenu { x, y } => Action::ShowWindowMenu(point([x, y])),
            v1::Operation::StartResize { edge } => {
                Action::StartResize(pb::ResizeEdge::from(edge).into())
            }
            v1::Operation::Resize {
                logical_width,
                logical_height,
                scale,
            } => Action::Resize(pb::operation::Resize {
                logical_width,
                logical_height,
                scale: Some(scale.into()),
            }),
            v1::Operation::SetDisplay { profile } => Action::SetDisplay(profile.into()),
            v1::Operation::SetPopupBounds { bounds } => Action::SetPopupBounds(bounds.into()),
        };
        Self {
            action: Some(action),
        }
    }
}
impl TryFrom<Option<pb::Operation>> for v1::Operation {
    type Error = String;
    fn try_from(value: Option<pb::Operation>) -> Result<Self> {
        use pb::operation::Action;
        Ok(
            match required(required(value, "operation")?.action, "operation")? {
                Action::Activate(_) => Self::Activate,
                Action::Move(_) => Self::Move,
                Action::Close(_) => Self::Close,
                Action::Minimize(_) => Self::Minimize,
                Action::Maximize(_) => Self::Maximize,
                Action::Restore(_) => Self::Restore,
                Action::Fullscreen(enabled) => Self::Fullscreen { enabled },
                Action::ShowWindowMenu(point) => Self::ShowWindowMenu {
                    x: point.x,
                    y: point.y,
                },
                Action::StartResize(edge) => Self::StartResize {
                    edge: enumeration::<pb::ResizeEdge>(edge, "resize edge")?.into(),
                },
                Action::Resize(resize) => Self::Resize {
                    logical_width: resize.logical_width,
                    logical_height: resize.logical_height,
                    scale: resize.scale.try_into()?,
                },
                Action::SetDisplay(profile) => Self::SetDisplay {
                    profile: profile.try_into()?,
                },
                Action::SetPopupBounds(bounds) => Self::SetPopupBounds {
                    bounds: bounds.try_into()?,
                },
            },
        )
    }
}

fn modifiers(value: v1::Modifiers) -> Option<pb::Modifiers> {
    (!value.is_empty()).then_some(pb::Modifiers {
        shift: value.shift,
        control: value.control,
        alt: value.alt,
        super_: value.logo,
        caps_lock: value.caps_lock,
    })
}
fn from_modifiers(value: Option<pb::Modifiers>) -> v1::Modifiers {
    let value = value.unwrap_or_default();
    v1::Modifiers {
        shift: value.shift,
        control: value.control,
        alt: value.alt,
        logo: value.super_,
        caps_lock: value.caps_lock,
    }
}
impl From<v1::Button> for pb::MouseButton {
    fn from(value: v1::Button) -> Self {
        match value {
            v1::Button::Left => Self::Left,
            v1::Button::Middle => Self::Middle,
            v1::Button::Right => Self::Right,
            v1::Button::Back => Self::Back,
            v1::Button::Forward => Self::Forward,
        }
    }
}
impl From<pb::MouseButton> for v1::Button {
    fn from(value: pb::MouseButton) -> Self {
        match value {
            pb::MouseButton::Left => Self::Left,
            pb::MouseButton::Middle => Self::Middle,
            pb::MouseButton::Right => Self::Right,
            pb::MouseButton::Back => Self::Back,
            pb::MouseButton::Forward => Self::Forward,
        }
    }
}
impl From<v1::ScrollPhase> for pb::ScrollPhase {
    fn from(value: v1::ScrollPhase) -> Self {
        match value {
            v1::ScrollPhase::Wheel => Self::Wheel,
            v1::ScrollPhase::Begin => Self::Begin,
            v1::ScrollPhase::Update => Self::Update,
            v1::ScrollPhase::End => Self::End,
            v1::ScrollPhase::Momentum => Self::Momentum,
            v1::ScrollPhase::MomentumEnd => Self::MomentumEnd,
        }
    }
}
impl From<pb::ScrollPhase> for v1::ScrollPhase {
    fn from(value: pb::ScrollPhase) -> Self {
        match value {
            pb::ScrollPhase::Wheel => Self::Wheel,
            pb::ScrollPhase::Begin => Self::Begin,
            pb::ScrollPhase::Update => Self::Update,
            pb::ScrollPhase::End => Self::End,
            pb::ScrollPhase::Momentum => Self::Momentum,
            pb::ScrollPhase::MomentumEnd => Self::MomentumEnd,
        }
    }
}
impl From<&v1::Input> for pb::Input {
    fn from(value: &v1::Input) -> Self {
        use pb::input::Kind;
        let kind = match value {
            v1::Input::Focus => Kind::Focus(Present {}),
            v1::Input::Blur => Kind::Blur(Present {}),
            v1::Input::Leave => Kind::Leave(Present {}),
            &v1::Input::Pointer { x, y, modifiers: m } => Kind::Pointer(pb::input::Pointer {
                x,
                y,
                modifiers: modifiers(m),
            }),
            &v1::Input::HeldPointer { x, y, modifiers: m } => {
                Kind::HeldPointer(pb::input::HeldPointer {
                    x,
                    y,
                    modifiers: modifiers(m),
                })
            }
            &v1::Input::Button {
                button,
                down,
                modifiers: m,
            } => Kind::Button(pb::input::Button {
                button: pb::MouseButton::from(button).into(),
                down,
                modifiers: modifiers(m),
            }),
            &v1::Input::Scroll {
                position,
                dx,
                dy,
                discrete,
                phase,
                modifiers: m,
            } => Kind::Scroll(pb::input::Scroll {
                position: Some(location(position)),
                dx,
                dy,
                discrete: discrete.map(point),
                phase: pb::ScrollPhase::from(phase).into(),
                modifiers: modifiers(m),
            }),
            v1::Input::Key {
                key,
                code,
                down,
                repeat,
                modifiers: m,
            } => Kind::Key(pb::input::Key {
                key: key.clone(),
                code: (*code).into(),
                down: *down,
                repeat: *repeat,
                modifiers: modifiers(*m),
            }),
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::Input>> for v1::Input {
    type Error = String;
    fn try_from(value: Option<pb::Input>) -> Result<Self> {
        use pb::input::Kind;
        Ok(match required(required(value, "input")?.kind, "input")? {
            Kind::Focus(_) => Self::Focus,
            Kind::Blur(_) => Self::Blur,
            Kind::Leave(_) => Self::Leave,
            Kind::Pointer(pointer) => Self::Pointer {
                x: pointer.x,
                y: pointer.y,
                modifiers: from_modifiers(pointer.modifiers),
            },
            Kind::HeldPointer(pointer) => Self::HeldPointer {
                x: pointer.x,
                y: pointer.y,
                modifiers: from_modifiers(pointer.modifiers),
            },
            Kind::Button(button) => Self::Button {
                button: enumeration::<pb::MouseButton>(button.button, "button")?.into(),
                down: button.down,
                modifiers: from_modifiers(button.modifiers),
            },
            Kind::Scroll(scroll) => Self::Scroll {
                position: from_location(scroll.position)?,
                dx: scroll.dx,
                dy: scroll.dy,
                discrete: scroll.discrete.map(from_point),
                phase: enumeration::<pb::ScrollPhase>(scroll.phase, "scroll phase")?.into(),
                modifiers: from_modifiers(scroll.modifiers),
            },
            Kind::Key(key) => Self::Key {
                key: key.key,
                code: narrow(key.code, "key code")?,
                down: key.down,
                repeat: key.repeat,
                modifiers: from_modifiers(key.modifiers),
            },
        })
    }
}

/// The cursor shapes in the order of both enumerations.
const CURSOR_SHAPES: [(v1::CursorShape, pb::CursorShape); 34] = {
    use pb::CursorShape as P;
    use v1::CursorShape as D;
    [
        (D::Default, P::Default),
        (D::ContextMenu, P::ContextMenu),
        (D::Help, P::Help),
        (D::Pointer, P::Pointer),
        (D::Progress, P::Progress),
        (D::Wait, P::Wait),
        (D::Cell, P::Cell),
        (D::Crosshair, P::Crosshair),
        (D::Text, P::Text),
        (D::VerticalText, P::VerticalText),
        (D::Alias, P::Alias),
        (D::Copy, P::Copy),
        (D::Move, P::Move),
        (D::NoDrop, P::NoDrop),
        (D::NotAllowed, P::NotAllowed),
        (D::Grab, P::Grab),
        (D::Grabbing, P::Grabbing),
        (D::EResize, P::EResize),
        (D::NResize, P::NResize),
        (D::NeResize, P::NeResize),
        (D::NwResize, P::NwResize),
        (D::SResize, P::SResize),
        (D::SeResize, P::SeResize),
        (D::SwResize, P::SwResize),
        (D::WResize, P::WResize),
        (D::EwResize, P::EwResize),
        (D::NsResize, P::NsResize),
        (D::NeswResize, P::NeswResize),
        (D::NwseResize, P::NwseResize),
        (D::ColResize, P::ColResize),
        (D::RowResize, P::RowResize),
        (D::AllScroll, P::AllScroll),
        (D::ZoomIn, P::ZoomIn),
        (D::ZoomOut, P::ZoomOut),
    ]
};
impl From<v1::Cursor> for pb::Cursor {
    fn from(value: v1::Cursor) -> Self {
        use pb::cursor::Kind;
        let kind = match value {
            v1::Cursor::Hidden => Kind::Hidden(Present {}),
            v1::Cursor::Shape { shape } => {
                let wire = CURSOR_SHAPES
                    .iter()
                    .find(|(domain, _)| *domain == shape)
                    .map_or(pb::CursorShape::Default, |(_, wire)| *wire);
                Kind::Shape(wire.into())
            }
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::Cursor>> for v1::Cursor {
    type Error = String;
    fn try_from(value: Option<pb::Cursor>) -> Result<Self> {
        use pb::cursor::Kind;
        Ok(match required(required(value, "cursor")?.kind, "cursor")? {
            Kind::Hidden(_) => Self::Hidden,
            Kind::Shape(shape) => {
                let shape = enumeration::<pb::CursorShape>(shape, "cursor shape")?;
                Self::Shape {
                    shape: CURSOR_SHAPES
                        .iter()
                        .find(|(_, wire)| *wire == shape)
                        .map(|(domain, _)| *domain)
                        .ok_or("Unknown cursor shape")?,
                }
            }
        })
    }
}

impl From<performance::CodecMode> for pb::CodecMode {
    fn from(value: performance::CodecMode) -> Self {
        match value {
            performance::CodecMode::Auto => Self::Auto,
            performance::CodecMode::Adaptive => Self::Adaptive,
            performance::CodecMode::Fast => Self::Fast,
            performance::CodecMode::Lossless => Self::Lossless,
        }
    }
}
impl From<pb::CodecMode> for performance::CodecMode {
    fn from(value: pb::CodecMode) -> Self {
        match value {
            pb::CodecMode::Auto => Self::Auto,
            pb::CodecMode::Adaptive => Self::Adaptive,
            pb::CodecMode::Fast => Self::Fast,
            pb::CodecMode::Lossless => Self::Lossless,
        }
    }
}
impl From<performance::Settings> for pb::PerformanceSettings {
    fn from(value: performance::Settings) -> Self {
        Self {
            codec_mode: pb::CodecMode::from(value.codec_mode).into(),
            max_fps: value.max_fps,
            jpeg_quality: value.jpeg_quality.into(),
            fast_trial: value.fast_trial,
        }
    }
}
impl TryFrom<pb::PerformanceSettings> for performance::Settings {
    type Error = String;
    fn try_from(value: pb::PerformanceSettings) -> Result<Self> {
        Ok(Self {
            codec_mode: enumeration::<pb::CodecMode>(value.codec_mode, "codec mode")?.into(),
            max_fps: value.max_fps,
            jpeg_quality: narrow(value.jpeg_quality, "jpeg_quality")?,
            fast_trial: value.fast_trial,
        })
    }
}

impl From<&features::Features> for pb::Features {
    fn from(value: &features::Features) -> Self {
        let present = |feature: Option<features::Present>| feature.map(|_| Present {});
        Self {
            text_input: present(value.text_input),
            upload: present(value.upload),
            drag_out: value.drag_out.as_ref().map(Into::into),
            audio: present(value.audio),
        }
    }
}
impl From<pb::Features> for features::Features {
    fn from(value: pb::Features) -> Self {
        let present = |feature: Option<Present>| feature.map(|_| features::Present {});
        Self {
            text_input: present(value.text_input),
            upload: present(value.upload),
            drag_out: value.drag_out.map(Into::into),
            audio: present(value.audio),
        }
    }
}
impl From<&features::DragOut> for pb::DragOut {
    fn from(value: &features::DragOut) -> Self {
        Self {
            files: value.files.map(|_| Present {}),
        }
    }
}
impl From<pb::DragOut> for features::DragOut {
    fn from(value: pb::DragOut) -> Self {
        Self {
            files: value.files.map(|_| features::Present {}),
        }
    }
}

impl From<&text_input::Context> for pb::text_input::Context {
    fn from(value: &text_input::Context) -> Self {
        Self {
            window: Some(value.window.into()),
            revision: value.revision,
            focus_epoch: value.focus_epoch,
            preedit: value.preedit,
            caret: value.caret.map(point),
        }
    }
}
impl TryFrom<Option<pb::text_input::Context>> for text_input::Context {
    type Error = String;
    fn try_from(value: Option<pb::text_input::Context>) -> Result<Self> {
        let value = required(value, "text input context")?;
        Ok(Self {
            window: window(value.window)?,
            revision: value.revision,
            focus_epoch: value.focus_epoch,
            preedit: value.preedit,
            caret: value.caret.map(from_point),
        })
    }
}
impl From<&text_input::Client> for pb::text_input::Client {
    fn from(value: &text_input::Client) -> Self {
        use pb::text_input::client::Kind;
        let kind = match value {
            text_input::Client::Preedit {
                request,
                context,
                text,
                cursor,
            } => Kind::Preedit(pb::text_input::Preedit {
                request: *request,
                context: Some(context.into()),
                text: text.clone(),
                cursor: *cursor,
            }),
            text_input::Client::Commit {
                request,
                context,
                text,
            } => Kind::Commit(pb::text_input::Commit {
                request: *request,
                context: Some(context.into()),
                text: text.clone(),
            }),
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::text_input::Client>> for text_input::Client {
    type Error = String;
    fn try_from(value: Option<pb::text_input::Client>) -> Result<Self> {
        use pb::text_input::client::Kind;
        Ok(
            match required(required(value, "text input")?.kind, "text input")? {
                Kind::Preedit(preedit) => Self::Preedit {
                    request: preedit.request,
                    context: preedit.context.try_into()?,
                    text: preedit.text,
                    cursor: preedit.cursor,
                },
                Kind::Commit(commit) => Self::Commit {
                    request: commit.request,
                    context: commit.context.try_into()?,
                    text: commit.text,
                },
            },
        )
    }
}
impl From<&text_input::Server> for pb::text_input::Server {
    fn from(value: &text_input::Server) -> Self {
        use pb::text_input::server::Kind;
        let kind = match value {
            text_input::Server::Focus { context } => Kind::Focus(pb::text_input::Focus {
                context: context.as_ref().map(Into::into),
            }),
            &text_input::Server::Result { request, applied } => {
                Kind::Result(pb::text_input::Result { request, applied })
            }
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::text_input::Server>> for text_input::Server {
    type Error = String;
    fn try_from(value: Option<pb::text_input::Server>) -> Result<Self> {
        use pb::text_input::server::Kind;
        Ok(
            match required(required(value, "text input")?.kind, "text input")? {
                Kind::Focus(focus) => Self::Focus {
                    context: focus
                        .context
                        .map(|context| Some(context).try_into())
                        .transpose()?,
                },
                Kind::Result(result) => Self::Result {
                    request: result.request,
                    applied: result.applied,
                },
            },
        )
    }
}

impl From<&file_tree::Entry> for pb::FileTreeEntry {
    fn from(value: &file_tree::Entry) -> Self {
        use pb::file_tree_entry::Contents;
        Self {
            parent: value.parent.map(u32::from),
            name: value.name.clone(),
            contents: Some(match value.contents {
                file_tree::Contents::Directory {} => Contents::Directory(Present {}),
                file_tree::Contents::File { bytes } => Contents::File(bytes),
            }),
        }
    }
}
impl TryFrom<pb::FileTreeEntry> for file_tree::Entry {
    type Error = String;
    fn try_from(value: pb::FileTreeEntry) -> Result<Self> {
        use pb::file_tree_entry::Contents;
        Ok(Self {
            parent: value
                .parent
                .map(|parent| narrow(parent, "tree parent"))
                .transpose()?,
            name: value.name,
            contents: match required(value.contents, "tree entry contents")? {
                Contents::Directory(_) => file_tree::Contents::Directory {},
                Contents::File(bytes) => file_tree::Contents::File { bytes },
            },
        })
    }
}
impl From<&file_upload::Client> for pb::upload::Client {
    fn from(value: &file_upload::Client) -> Self {
        use pb::upload::{Transfer, client::Kind};
        let kind = match value {
            file_upload::Client::Begin { transfer, files } => Kind::Begin(pb::upload::Begin {
                transfer: *transfer,
                files: files
                    .iter()
                    .map(|file| pb::upload::File {
                        name: file.name.clone(),
                        bytes: file.bytes,
                    })
                    .collect(),
            }),
            file_upload::Client::BeginTree { transfer, entries } => {
                Kind::BeginTree(pb::upload::BeginTree {
                    transfer: *transfer,
                    entries: entries.iter().map(Into::into).collect(),
                })
            }
            file_upload::Client::Chunk {
                transfer,
                file,
                offset,
                data,
            } => Kind::Chunk(pb::upload::Chunk {
                transfer: *transfer,
                file: (*file).into(),
                offset: *offset,
                data: data.clone(),
            }),
            &file_upload::Client::Finish { transfer } => Kind::Finish(Transfer { transfer }),
            &file_upload::Client::Cancel { transfer } => Kind::Cancel(Transfer { transfer }),
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::upload::Client>> for file_upload::Client {
    type Error = String;
    fn try_from(value: Option<pb::upload::Client>) -> Result<Self> {
        use pb::upload::client::Kind;
        Ok(match required(required(value, "upload")?.kind, "upload")? {
            Kind::Begin(begin) => Self::Begin {
                transfer: begin.transfer,
                files: begin
                    .files
                    .into_iter()
                    .map(|file| file_upload::FileSpec {
                        name: file.name,
                        bytes: file.bytes,
                    })
                    .collect(),
            },
            Kind::BeginTree(begin) => Self::BeginTree {
                transfer: begin.transfer,
                entries: begin
                    .entries
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_>>()?,
            },
            Kind::Chunk(chunk) => Self::Chunk {
                transfer: chunk.transfer,
                file: narrow(chunk.file, "upload file")?,
                offset: chunk.offset,
                data: chunk.data,
            },
            Kind::Finish(finish) => Self::Finish {
                transfer: finish.transfer,
            },
            Kind::Cancel(cancel) => Self::Cancel {
                transfer: cancel.transfer,
            },
        })
    }
}
impl From<&file_upload::Server> for pb::upload::Server {
    fn from(value: &file_upload::Server) -> Self {
        use pb::upload::{Transfer, server::Kind};
        let kind = match value {
            &file_upload::Server::Ack {
                transfer,
                file,
                offset,
            } => Kind::Ack(pb::upload::Ack {
                transfer,
                file: file.into(),
                offset,
            }),
            &file_upload::Server::Ready { transfer } => Kind::Ready(Transfer { transfer }),
            &file_upload::Server::Cancelled { transfer } => Kind::Cancelled(Transfer { transfer }),
            file_upload::Server::Rejected { transfer, reason } => {
                Kind::Rejected(pb::upload::Rejected {
                    transfer: *transfer,
                    reason: reason.clone(),
                })
            }
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::upload::Server>> for file_upload::Server {
    type Error = String;
    fn try_from(value: Option<pb::upload::Server>) -> Result<Self> {
        use pb::upload::server::Kind;
        Ok(match required(required(value, "upload")?.kind, "upload")? {
            Kind::Ack(ack) => Self::Ack {
                transfer: ack.transfer,
                file: narrow(ack.file, "upload file")?,
                offset: ack.offset,
            },
            Kind::Ready(ready) => Self::Ready {
                transfer: ready.transfer,
            },
            Kind::Cancelled(cancelled) => Self::Cancelled {
                transfer: cancelled.transfer,
            },
            Kind::Rejected(rejected) => Self::Rejected {
                transfer: rejected.transfer,
                reason: rejected.reason,
            },
        })
    }
}

impl From<&drag::Client> for pb::drag::Client {
    fn from(value: &drag::Client) -> Self {
        use pb::drag::{Ref, client::Kind};
        let kind = match *value {
            drag::Client::Begin {
                drag,
                transfer,
                window,
                surface,
                point,
            } => Kind::Begin(pb::drag::Begin {
                drag,
                transfer,
                window: Some(window.into()),
                surface,
                point: Some(location(point)),
            }),
            drag::Client::Motion { drag, point } => Kind::Motion(pb::drag::Motion {
                drag,
                point: Some(location(point)),
            }),
            drag::Client::Drop { drag } => Kind::Drop(Ref { drag }),
            drag::Client::Cancel { drag } => Kind::Cancel(Ref { drag }),
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::drag::Client>> for drag::Client {
    type Error = String;
    fn try_from(value: Option<pb::drag::Client>) -> Result<Self> {
        use pb::drag::client::Kind;
        Ok(match required(required(value, "drag")?.kind, "drag")? {
            Kind::Begin(begin) => Self::Begin {
                drag: begin.drag,
                transfer: begin.transfer,
                window: window(begin.window)?,
                surface: begin.surface,
                point: from_location(begin.point)?,
            },
            Kind::Motion(motion) => Self::Motion {
                drag: motion.drag,
                point: from_location(motion.point)?,
            },
            Kind::Drop(drop) => Self::Drop { drag: drop.drag },
            Kind::Cancel(cancel) => Self::Cancel { drag: cancel.drag },
        })
    }
}
impl From<&drag::Server> for pb::drag::Server {
    fn from(value: &drag::Server) -> Self {
        use pb::drag::server::Kind;
        let kind = match value {
            &drag::Server::Status {
                drag,
                revision,
                accepted,
            } => Kind::Status(pb::drag::Status {
                drag,
                revision,
                accepted,
            }),
            drag::Server::Finished {
                drag,
                copied,
                reason,
            } => Kind::Finished(pb::drag::Finished {
                drag: *drag,
                copied: *copied,
                reason: reason.clone(),
            }),
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::drag::Server>> for drag::Server {
    type Error = String;
    fn try_from(value: Option<pb::drag::Server>) -> Result<Self> {
        use pb::drag::server::Kind;
        Ok(match required(required(value, "drag")?.kind, "drag")? {
            Kind::Status(status) => Self::Status {
                drag: status.drag,
                revision: status.revision,
                accepted: status.accepted,
            },
            Kind::Finished(finished) => Self::Finished {
                drag: finished.drag,
                copied: finished.copied,
                reason: finished.reason,
            },
        })
    }
}

impl From<&reverse_drag::Client> for pb::reverse_drag::Client {
    fn from(value: &reverse_drag::Client) -> Self {
        use pb::reverse_drag::{Ref, client::Kind};
        let kind = match *value {
            reverse_drag::Client::Read { drag, offset } => {
                Kind::Read(pb::reverse_drag::Read { drag, offset })
            }
            reverse_drag::Client::Ready { drag } => Kind::Ready(Ref { drag }),
            reverse_drag::Client::Complete { drag, copied } => {
                Kind::Complete(pb::reverse_drag::Complete { drag, copied })
            }
            reverse_drag::Client::Cancel { drag } => Kind::Cancel(Ref { drag }),
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::reverse_drag::Client>> for reverse_drag::Client {
    type Error = String;
    fn try_from(value: Option<pb::reverse_drag::Client>) -> Result<Self> {
        use pb::reverse_drag::client::Kind;
        Ok(
            match required(required(value, "reverse drag")?.kind, "reverse drag")? {
                Kind::Read(read) => Self::Read {
                    drag: read.drag,
                    offset: read.offset,
                },
                Kind::Ready(ready) => Self::Ready { drag: ready.drag },
                Kind::Complete(complete) => Self::Complete {
                    drag: complete.drag,
                    copied: complete.copied,
                },
                Kind::Cancel(cancel) => Self::Cancel { drag: cancel.drag },
            },
        )
    }
}
impl From<&reverse_drag::Server> for pb::reverse_drag::Server {
    fn from(value: &reverse_drag::Server) -> Self {
        use pb::reverse_drag::{Ref, server::Kind};
        let kind = match value {
            reverse_drag::Server::Offer {
                drag,
                gesture,
                window,
                surface,
                mime,
                bytes,
            } => Kind::Offer(pb::reverse_drag::Offer {
                drag: *drag,
                gesture: *gesture,
                window: Some((*window).into()),
                surface: *surface,
                mime: mime.clone(),
                bytes: *bytes,
            }),
            reverse_drag::Server::Chunk { drag, offset, data } => {
                Kind::Chunk(pb::reverse_drag::Chunk {
                    drag: *drag,
                    offset: *offset,
                    data: data.clone(),
                })
            }
            &reverse_drag::Server::Drop { drag } => Kind::Drop(Ref { drag }),
            &reverse_drag::Server::Cancel { drag } => Kind::Cancel(Ref { drag }),
            &reverse_drag::Server::ExitRefused { gesture } => {
                Kind::ExitRefused(pb::reverse_drag::ExitRefused { gesture })
            }
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::reverse_drag::Server>> for reverse_drag::Server {
    type Error = String;
    fn try_from(value: Option<pb::reverse_drag::Server>) -> Result<Self> {
        use pb::reverse_drag::server::Kind;
        Ok(
            match required(required(value, "reverse drag")?.kind, "reverse drag")? {
                Kind::Offer(offer) => Self::Offer {
                    drag: offer.drag,
                    gesture: offer.gesture,
                    window: window(offer.window)?,
                    surface: offer.surface,
                    mime: offer.mime,
                    bytes: offer.bytes,
                },
                Kind::Chunk(chunk) => Self::Chunk {
                    drag: chunk.drag,
                    offset: chunk.offset,
                    data: chunk.data,
                },
                Kind::Drop(drop) => Self::Drop { drag: drop.drag },
                Kind::Cancel(cancel) => Self::Cancel { drag: cancel.drag },
                Kind::ExitRefused(refused) => Self::ExitRefused {
                    gesture: refused.gesture,
                },
            },
        )
    }
}

impl From<&reverse_files::Context> for pb::reverse_files::Context {
    fn from(value: &reverse_files::Context) -> Self {
        Self {
            drag: value.drag,
            gesture: value.gesture,
            window: Some(value.window.into()),
            surface: value.surface,
        }
    }
}
impl TryFrom<Option<pb::reverse_files::Context>> for reverse_files::Context {
    type Error = String;
    fn try_from(value: Option<pb::reverse_files::Context>) -> Result<Self> {
        let value = required(value, "reverse file context")?;
        Ok(Self {
            drag: value.drag,
            gesture: value.gesture,
            window: window(value.window)?,
            surface: value.surface,
        })
    }
}
impl From<&reverse_files::Server> for pb::reverse_files::Server {
    fn from(value: &reverse_files::Server) -> Self {
        Self {
            context: Some((&value.context).into()),
            message: Some((&value.message).into()),
        }
    }
}
impl TryFrom<Option<pb::reverse_files::Server>> for reverse_files::Server {
    type Error = String;
    fn try_from(value: Option<pb::reverse_files::Server>) -> Result<Self> {
        let value = required(value, "reverse files")?;
        Ok(Self {
            context: value.context.try_into()?,
            message: value.message.try_into()?,
        })
    }
}
impl From<&reverse_files::Client> for pb::reverse_files::Client {
    fn from(value: &reverse_files::Client) -> Self {
        Self {
            context: Some((&value.context).into()),
            message: Some((&value.message).into()),
        }
    }
}
impl TryFrom<Option<pb::reverse_files::Client>> for reverse_files::Client {
    type Error = String;
    fn try_from(value: Option<pb::reverse_files::Client>) -> Result<Self> {
        let value = required(value, "reverse files")?;
        Ok(Self {
            context: value.context.try_into()?,
            message: value.message.try_into()?,
        })
    }
}

impl From<&session_launch::Target> for pb::LaunchTarget {
    fn from(value: &session_launch::Target) -> Self {
        use pb::launch_target::Kind;
        let kind = match value {
            session_launch::Target::Command { argv } => {
                Kind::Command(pb::launch_target::Command { argv: argv.clone() })
            }
            session_launch::Target::Desktop { id, uris, action } => {
                Kind::Desktop(pb::launch_target::Desktop {
                    id: id.clone(),
                    uris: uris.clone(),
                    action: action.clone(),
                })
            }
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::LaunchTarget>> for session_launch::Target {
    type Error = String;
    fn try_from(value: Option<pb::LaunchTarget>) -> Result<Self> {
        use pb::launch_target::Kind;
        Ok(
            match required(required(value, "launch target")?.kind, "launch target")? {
                Kind::Command(command) => Self::Command { argv: command.argv },
                Kind::Desktop(desktop) => Self::Desktop {
                    id: desktop.id,
                    uris: desktop.uris,
                    action: desktop.action,
                },
            },
        )
    }
}
impl From<&session_launch::Outcome> for pb::LaunchOutcome {
    fn from(value: &session_launch::Outcome) -> Self {
        use pb::launch_outcome::Status;
        let status = match value {
            session_launch::Outcome::Accepted => Status::Accepted(Present {}),
            session_launch::Outcome::Failed { message } => {
                Status::Failed(pb::launch_outcome::Failed {
                    message: message.clone(),
                })
            }
        };
        Self {
            status: Some(status),
        }
    }
}
impl TryFrom<Option<pb::LaunchOutcome>> for session_launch::Outcome {
    type Error = String;
    fn try_from(value: Option<pb::LaunchOutcome>) -> Result<Self> {
        use pb::launch_outcome::Status;
        Ok(
            match required(required(value, "launch outcome")?.status, "launch outcome")? {
                Status::Accepted(_) => Self::Accepted,
                Status::Failed(failed) => Self::Failed {
                    message: failed.message,
                },
            },
        )
    }
}
impl From<&session_applications::Snapshot> for pb::Applications {
    fn from(value: &session_applications::Snapshot) -> Self {
        use pb::applications::{Application, application::State};
        Self {
            revision: value.revision,
            applications: value
                .applications
                .iter()
                .map(|app| Application {
                    request: app.request,
                    state: Some(match app.state {
                        session_applications::State::Opening => State::Opening(Present {}),
                        session_applications::State::Window { window } => {
                            State::Window(window.into())
                        }
                        session_applications::State::Stopping => State::Stopping(Present {}),
                    }),
                })
                .collect(),
        }
    }
}
impl TryFrom<Option<pb::Applications>> for session_applications::Snapshot {
    type Error = String;
    fn try_from(value: Option<pb::Applications>) -> Result<Self> {
        use pb::applications::application::State;
        let value = required(value, "application inventory")?;
        Ok(Self {
            revision: value.revision,
            applications: value
                .applications
                .into_iter()
                .map(|app| {
                    Ok(session_applications::Application {
                        request: app.request,
                        state: match required(app.state, "application state")? {
                            State::Opening(_) => session_applications::State::Opening,
                            State::Window(window) => session_applications::State::Window {
                                window: window.into(),
                            },
                            State::Stopping(_) => session_applications::State::Stopping,
                        },
                    })
                })
                .collect::<Result<_>>()?,
        })
    }
}
impl From<session_activity::Snapshot> for pb::Activity {
    fn from(value: session_activity::Snapshot) -> Self {
        Self {
            revision: value.revision,
            applications: value.applications,
        }
    }
}
impl TryFrom<Option<pb::Activity>> for session_activity::Snapshot {
    type Error = String;
    fn try_from(value: Option<pb::Activity>) -> Result<Self> {
        let value = required(value, "application activity")?;
        Ok(Self {
            revision: value.revision,
            applications: value.applications,
        })
    }
}

impl From<lifecycle::Stage> for pb::lifecycle::Stage {
    fn from(value: lifecycle::Stage) -> Self {
        match value {
            lifecycle::Stage::Preparing => Self::Preparing,
            lifecycle::Stage::ServiceLookup => Self::ServiceLookup,
            lifecycle::Stage::ServiceRegistration => Self::ServiceRegistration,
            lifecycle::Stage::ActivationCall => Self::ActivationCall,
            lifecycle::Stage::Command => Self::Command,
            lifecycle::Stage::AwaitingWindow => Self::AwaitingWindow,
            lifecycle::Stage::AwaitingPresentation => Self::AwaitingPresentation,
        }
    }
}
#[cfg(test)]
pub(crate) fn enum_maps() -> Vec<crate::desktop::wire::wire_tables::EnumMap> {
    vec![crate::desktop::wire::wire_tables::enum_map(
        "newport.remote.apps.Lifecycle.Stage",
        |value| from_stage(value).ok(),
        |value| stage(*value),
    )]
}
fn stage(value: lifecycle::Stage) -> i32 {
    pb::lifecycle::Stage::from(value).into()
}
fn from_stage(value: i32) -> Result<lifecycle::Stage> {
    use pb::lifecycle::Stage;
    Ok(match enumeration::<Stage>(value, "startup stage")? {
        Stage::Preparing => lifecycle::Stage::Preparing,
        Stage::ServiceLookup => lifecycle::Stage::ServiceLookup,
        Stage::ServiceRegistration => lifecycle::Stage::ServiceRegistration,
        Stage::ActivationCall => lifecycle::Stage::ActivationCall,
        Stage::Command => lifecycle::Stage::Command,
        Stage::AwaitingWindow => lifecycle::Stage::AwaitingWindow,
        Stage::AwaitingPresentation => lifecycle::Stage::AwaitingPresentation,
    })
}
impl From<lifecycle::Window> for pb::lifecycle::Window {
    fn from(value: lifecycle::Window) -> Self {
        Self {
            window: Some(value.window.into()),
            surface: value.surface,
            existing_instance: value.existing_instance,
        }
    }
}
impl TryFrom<pb::lifecycle::Window> for lifecycle::Window {
    type Error = String;
    fn try_from(value: pb::lifecycle::Window) -> Result<Self> {
        Ok(Self {
            window: window(value.window)?,
            surface: value.surface,
            existing_instance: value.existing_instance,
        })
    }
}
impl From<&lifecycle::Outcome> for pb::lifecycle::Outcome {
    fn from(value: &lifecycle::Outcome) -> Self {
        use pb::lifecycle::outcome::{self as wire, Kind};
        let kind = match value {
            lifecycle::Outcome::Presented(window) => Kind::Presented((*window).into()),
            lifecycle::Outcome::CompletedWithoutPresentation { output } => {
                Kind::CompletedWithoutPresentation(wire::CompletedWithoutPresentation {
                    output: output.clone(),
                })
            }
            lifecycle::Outcome::HandedOff {} => Kind::HandedOff(Present {}),
            lifecycle::Outcome::Failed {
                stage: s,
                message,
                outcome_unknown,
            } => Kind::Failed(wire::Failed {
                stage: stage(*s),
                message: message.clone(),
                outcome_unknown: *outcome_unknown,
            }),
            &lifecycle::Outcome::TimedOut {
                stage: s,
                outcome_unknown,
            } => Kind::TimedOut(wire::TimedOut {
                stage: stage(s),
                outcome_unknown,
            }),
            &lifecycle::Outcome::Cancelled { outcome_unknown } => {
                Kind::Cancelled(wire::Cancelled { outcome_unknown })
            }
        };
        Self { kind: Some(kind) }
    }
}
impl TryFrom<Option<pb::lifecycle::Outcome>> for lifecycle::Outcome {
    type Error = String;
    fn try_from(value: Option<pb::lifecycle::Outcome>) -> Result<Self> {
        use pb::lifecycle::outcome::Kind;
        Ok(
            match required(required(value, "startup outcome")?.kind, "startup outcome")? {
                Kind::Presented(window) => Self::Presented(window.try_into()?),
                Kind::CompletedWithoutPresentation(completed) => {
                    Self::CompletedWithoutPresentation {
                        output: completed.output,
                    }
                }
                Kind::HandedOff(_) => Self::HandedOff {},
                Kind::Failed(failed) => Self::Failed {
                    stage: from_stage(failed.stage)?,
                    message: failed.message,
                    outcome_unknown: failed.outcome_unknown,
                },
                Kind::TimedOut(timed_out) => Self::TimedOut {
                    stage: from_stage(timed_out.stage)?,
                    outcome_unknown: timed_out.outcome_unknown,
                },
                Kind::Cancelled(cancelled) => Self::Cancelled {
                    outcome_unknown: cancelled.outcome_unknown,
                },
            },
        )
    }
}
impl From<&lifecycle::Update> for pb::lifecycle::Update {
    fn from(value: &lifecycle::Update) -> Self {
        use pb::lifecycle::state::Kind;
        let kind = match &value.state {
            lifecycle::State::Starting { stage: s } => Kind::Starting(stage(*s)),
            lifecycle::State::Background { waited_ms } => Kind::Background(*waited_ms),
            lifecycle::State::Window(window) => Kind::Window((*window).into()),
            lifecycle::State::Finished { outcome } => Kind::Finished(outcome.into()),
        };
        Self {
            session: value.session,
            request: value.request,
            revision: value.revision,
            state: Some(pb::lifecycle::State { kind: Some(kind) }),
        }
    }
}
impl TryFrom<pb::lifecycle::Update> for lifecycle::Update {
    type Error = String;
    fn try_from(value: pb::lifecycle::Update) -> Result<Self> {
        use pb::lifecycle::state::Kind;
        let state = required(
            required(value.state, "startup state")?.kind,
            "startup state",
        )?;
        Ok(Self {
            session: value.session,
            request: value.request,
            revision: value.revision,
            state: match state {
                Kind::Starting(s) => lifecycle::State::Starting {
                    stage: from_stage(s)?,
                },
                Kind::Background(waited_ms) => lifecycle::State::Background { waited_ms },
                Kind::Window(window) => lifecycle::State::Window(window.try_into()?),
                Kind::Finished(outcome) => lifecycle::State::Finished {
                    outcome: Some(outcome).try_into()?,
                },
            },
        })
    }
}
impl From<lifecycle::Presented> for pb::lifecycle::Presented {
    fn from(value: lifecycle::Presented) -> Self {
        Self {
            session: value.session,
            request: value.request,
            window: Some(value.window.into()),
            surface: value.surface,
        }
    }
}
impl TryFrom<pb::lifecycle::Presented> for lifecycle::Presented {
    type Error = String;
    fn try_from(value: pb::lifecycle::Presented) -> Result<Self> {
        Ok(Self {
            session: value.session,
            request: value.request,
            window: window(value.window)?,
            surface: value.surface,
        })
    }
}

impl From<&crate::display::attachment::DisplayIdentity> for pd::DisplayIdentity {
    fn from(value: &crate::display::attachment::DisplayIdentity) -> Self {
        Self {
            revision: value.revision,
            build: value.build.clone(),
        }
    }
}
impl From<pd::DisplayIdentity> for crate::display::attachment::DisplayIdentity {
    fn from(value: pd::DisplayIdentity) -> Self {
        Self {
            revision: value.revision,
            build: value.build,
        }
    }
}
fn license(value: &v1::LicenseToken) -> crate::encoding::protobuf::Secret {
    crate::encoding::protobuf::Secret {
        value: value.0.clone().into_bytes().into(),
    }
}
fn from_license(value: crate::encoding::protobuf::Secret) -> Result<v1::LicenseToken> {
    String::from_utf8(value.value.to_vec())
        .map(v1::LicenseToken)
        .map_err(|_| "Invalid license token".to_owned())
}
fn error(code: &str, message: &str) -> crate::encoding::protobuf::Error {
    crate::encoding::protobuf::Error {
        code: code.to_owned(),
        message: message.to_owned(),
        retry: crate::encoding::protobuf::Retry::Unknown.into(),
    }
}

impl From<&v1::ServerMessage> for pb::ServerFrame {
    fn from(value: &v1::ServerMessage) -> Self {
        use pb::server_frame::{self as wire, Message};
        use v1::ServerMessage as M;
        let message = match value {
            M::ApplicationLifecycle(update) => Message::ApplicationLifecycle(update.into()),
            M::SessionApplications { session, snapshot } => {
                Message::SessionApplications(wire::SessionApplications {
                    session: *session,
                    snapshot: Some(snapshot.into()),
                })
            }
            &M::SessionActivity { session, activity } => {
                Message::SessionActivity(wire::SessionActivity {
                    session,
                    activity: Some(activity.into()),
                })
            }
            M::LaunchResult {
                session,
                request,
                outcome,
            } => Message::LaunchResult(wire::LaunchResult {
                session: *session,
                request: *request,
                outcome: Some(outcome.into()),
            }),
            M::Audio {
                session,
                sequence,
                sample_time,
                captured,
                hole,
                epoch,
                data,
            } => Message::Audio(wire::Audio {
                session: *session,
                sequence: *sequence,
                sample_time: *sample_time,
                data: data.clone(),
                captured: captured.map(Into::into),
                hole: *hole,
                epoch: *epoch,
            }),
            M::AudioState { session, state } => Message::AudioState(wire::AudioState {
                session: *session,
                state: Some(match state {
                    audio::State::Waiting { reason } => {
                        wire::audio_state::State::Waiting(wire::audio_state::Waiting {
                            reason: reason.clone(),
                        })
                    }
                    audio::State::Playing => wire::audio_state::State::Playing(Present {}),
                }),
            }),
            M::ReverseFiles { session, message } => Message::ReverseFiles(wire::ReverseFiles {
                session: *session,
                message: Some(message.into()),
            }),
            M::ReverseDrag { session, message } => Message::ReverseDrag(wire::ReverseDrag {
                session: *session,
                message: Some(message.into()),
            }),
            M::Drag { session, message } => Message::Drag(wire::Drag {
                session: *session,
                message: Some(message.into()),
            }),
            M::FileUpload { session, message } => Message::FileUpload(wire::FileUpload {
                session: *session,
                message: Some(message.into()),
            }),
            M::TextInput { session, message } => Message::TextInput(wire::TextInput {
                session: *session,
                message: Some(message.into()),
            }),
            M::Welcome {
                session,
                version,
                features,
                performance,
            } => Message::Welcome(wire::Welcome {
                session: *session,
                version: (*version).into(),
                features: Some(features.into()),
                performance: performance.map(Into::into),
            }),
            M::WindowCreated {
                session,
                window,
                surface,
                moved,
            } => Message::WindowCreated(wire::WindowCreated {
                session: *session,
                window: Some(window.into()),
                surface: Some((*surface).into()),
                moved: *moved,
            }),
            M::WindowUpdated { session, window } => Message::WindowUpdated(wire::WindowUpdated {
                session: *session,
                window: Some(window.into()),
            }),
            &M::SurfaceChanged {
                session,
                window,
                surface,
            } => Message::SurfaceChanged(wire::SurfaceChanged {
                session,
                window: Some(window.into()),
                surface: Some(surface.into()),
            }),
            M::Frame {
                session,
                window,
                surface,
                sequence,
                keyframe,
                tiles,
                captured,
                diagnostics,
            } => Message::Frame(wire::Frame {
                session: *session,
                window: Some((*window).into()),
                surface: *surface,
                sequence: *sequence,
                keyframe: *keyframe,
                tiles: tiles.iter().map(Into::into).collect(),
                captured: captured.map(Into::into),
                diagnostics: diagnostics.as_ref().map(Into::into),
            }),
            M::WindowAction {
                session,
                window,
                action,
            } => Message::WindowAction(wire::WindowAction {
                session: *session,
                window: Some((*window).into()),
                action: Some(action.into()),
            }),
            &M::Cursor {
                session,
                window,
                cursor,
            } => Message::Cursor(wire::Cursor {
                session,
                window: Some(window.into()),
                cursor: Some(cursor.into()),
            }),
            M::Error { session, message } => Message::Error(wire::Error {
                session: *session,
                error: Some(error("failed", message)),
            }),
            &M::WindowDestroyed { session, window } => {
                Message::WindowDestroyed(wire::WindowDestroyed {
                    session,
                    window: Some(window.into()),
                })
            }
            M::OperationResult {
                session,
                request,
                window,
                applied,
                message,
            } => Message::OperationResult(wire::OperationResult {
                session: *session,
                request: *request,
                window: Some((*window).into()),
                applied: *applied,
                message: message.clone(),
            }),
            &M::Closed { session } => Message::Closed(wire::Closed { session }),
            M::Rejected {
                reason,
                message,
                display,
            } => Message::Rejected(wire::Rejected {
                error: Some(error(reason, message)),
                display: display.as_ref().map(Into::into),
            }),
            M::Ended {
                session,
                reason,
                message,
                computer,
                computer_session,
            } => Message::Ended(wire::Ended {
                session: *session,
                reason: reason.clone(),
                message: message.clone(),
                computer: computer.clone(),
                computer_session: computer_session.clone(),
            }),
        };
        Self {
            message: Some(message),
        }
    }
}
impl TryFrom<pb::server_frame::Message> for v1::ServerMessage {
    type Error = String;
    fn try_from(value: pb::server_frame::Message) -> Result<Self> {
        use pb::server_frame::Message;
        Ok(match value {
            Message::WorkerControl(_) => {
                return Err("Unexpected worker control at service endpoint".into());
            }
            Message::ApplicationLifecycle(update) => Self::ApplicationLifecycle(update.try_into()?),
            Message::SessionApplications(m) => Self::SessionApplications {
                session: m.session,
                snapshot: m.snapshot.try_into()?,
            },
            Message::SessionActivity(m) => Self::SessionActivity {
                session: m.session,
                activity: m.activity.try_into()?,
            },
            Message::LaunchResult(m) => Self::LaunchResult {
                session: m.session,
                request: m.request,
                outcome: m.outcome.try_into()?,
            },
            Message::Audio(m) => Self::Audio {
                session: m.session,
                sequence: m.sequence,
                sample_time: m.sample_time,
                captured: m.captured.map(Into::into),
                hole: m.hole,
                epoch: m.epoch,
                data: m.data,
            },
            Message::AudioState(m) => Self::AudioState {
                session: m.session,
                state: match required(m.state, "audio state")? {
                    pb::server_frame::audio_state::State::Waiting(waiting) => {
                        audio::State::Waiting {
                            reason: waiting.reason,
                        }
                    }
                    pb::server_frame::audio_state::State::Playing(_) => audio::State::Playing,
                },
            },
            Message::ReverseFiles(m) => Self::ReverseFiles {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::ReverseDrag(m) => Self::ReverseDrag {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::Drag(m) => Self::Drag {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::FileUpload(m) => Self::FileUpload {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::TextInput(m) => Self::TextInput {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::Welcome(m) => Self::Welcome {
                session: m.session,
                version: narrow(m.version, "protocol version")?,
                features: m.features.unwrap_or_default().into(),
                performance: m.performance.map(TryInto::try_into).transpose()?,
            },
            Message::WindowCreated(m) => Self::WindowCreated {
                session: m.session,
                window: m.window.try_into()?,
                surface: m.surface.try_into()?,
                moved: m.moved,
            },
            Message::WindowUpdated(m) => Self::WindowUpdated {
                session: m.session,
                window: m.window.try_into()?,
            },
            Message::SurfaceChanged(m) => Self::SurfaceChanged {
                session: m.session,
                window: window(m.window)?,
                surface: m.surface.try_into()?,
            },
            Message::Frame(m) => Self::Frame {
                session: m.session,
                window: window(m.window)?,
                surface: m.surface,
                sequence: m.sequence,
                keyframe: m.keyframe,
                tiles: m
                    .tiles
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_>>()?,
                captured: m.captured.map(Into::into),
                diagnostics: m.diagnostics.map(Into::into),
            },
            Message::WindowAction(m) => Self::WindowAction {
                session: m.session,
                window: window(m.window)?,
                action: m.action.try_into()?,
            },
            Message::Cursor(m) => Self::Cursor {
                session: m.session,
                window: window(m.window)?,
                cursor: m.cursor.try_into()?,
            },
            Message::Error(m) => Self::Error {
                session: m.session,
                message: required(m.error, "error")?.message,
            },
            Message::WindowDestroyed(m) => Self::WindowDestroyed {
                session: m.session,
                window: window(m.window)?,
            },
            Message::OperationResult(m) => Self::OperationResult {
                session: m.session,
                request: m.request,
                window: window(m.window)?,
                applied: m.applied,
                message: m.message,
            },
            Message::Closed(m) => Self::Closed { session: m.session },
            Message::Rejected(m) => {
                let error = required(m.error, "rejection")?;
                Self::Rejected {
                    reason: error.code,
                    message: error.message,
                    display: m.display.map(Into::into),
                }
            }
            Message::Ended(m) => Self::Ended {
                session: m.session,
                reason: m.reason,
                message: m.message,
                computer: m.computer,
                computer_session: m.computer_session,
            },
        })
    }
}

impl From<&v1::ClientMessage> for pb::ClientFrame {
    fn from(value: &v1::ClientMessage) -> Self {
        use pb::client_frame::{self as wire, Message};
        use v1::ClientMessage as M;
        let message = match value {
            &M::ApplicationPresented(presented) => Message::ApplicationPresented(presented.into()),
            &M::StopApplication { session, request } => {
                Message::StopApplication(wire::StopApplication { session, request })
            }
            M::Launch {
                session,
                request,
                target,
            } => Message::Launch(wire::Launch {
                session: *session,
                request: *request,
                target: Some(target.into()),
            }),
            &M::Heartbeat { session } => Message::Heartbeat(wire::Heartbeat { session }),
            M::ReverseFiles { session, message } => Message::ReverseFiles(wire::ReverseFiles {
                session: *session,
                message: Some(message.into()),
            }),
            &M::ReverseDragExit {
                session,
                gesture,
                window,
                surface,
            } => Message::ReverseDragExit(wire::ReverseDragExit {
                session,
                gesture,
                window: Some(window.into()),
                surface,
            }),
            M::ReverseDrag { session, message } => Message::ReverseDrag(wire::ReverseDrag {
                session: *session,
                message: Some(message.into()),
            }),
            M::Drag { session, message } => Message::Drag(wire::Drag {
                session: *session,
                message: Some(message.into()),
            }),
            M::FileUpload { session, message } => Message::FileUpload(wire::FileUpload {
                session: *session,
                message: Some(message.into()),
            }),
            M::TextInput { session, message } => Message::TextInput(wire::TextInput {
                session: *session,
                message: Some(message.into()),
            }),
            M::Hello {
                versions,
                features,
                performance,
                display,
                license: token,
            } => Message::Hello(wire::Hello {
                versions: versions.iter().map(|&v| v.into()).collect(),
                features: Some(features.into()),
                performance: performance.map(Into::into),
                display: display.map(Into::into),
                license: token.as_ref().map(license),
            }),
            M::License { session, token } => Message::License(wire::License {
                session: *session,
                token: Some(license(token)),
            }),
            M::Request {
                session,
                request,
                window,
                operation,
            } => Message::Request(wire::Request {
                session: *session,
                request: *request,
                window: Some((*window).into()),
                operation: Some(operation.into()),
            }),
            &M::Ack {
                session,
                window,
                surface,
                sequence,
            } => Message::Ack(wire::Ack {
                session,
                window: Some(window.into()),
                surface,
                sequence,
            }),
            M::Input {
                session,
                window,
                surface,
                input,
            } => Message::Input(wire::Input {
                session: *session,
                window: Some((*window).into()),
                surface: *surface,
                input: Some(input.into()),
            }),
            &M::Quit { session } => Message::Quit(wire::Quit { session }),
        };
        Self {
            message: Some(message),
        }
    }
}
impl TryFrom<pb::client_frame::Message> for v1::ClientMessage {
    type Error = String;
    fn try_from(value: pb::client_frame::Message) -> Result<Self> {
        use pb::client_frame::Message;
        Ok(match value {
            Message::WorkerControl(_) => {
                return Err("Unexpected worker control at service endpoint".into());
            }
            Message::ApplicationPresented(m) => Self::ApplicationPresented(m.try_into()?),
            Message::StopApplication(m) => Self::StopApplication {
                session: m.session,
                request: m.request,
            },
            Message::Launch(m) => Self::Launch {
                session: m.session,
                request: m.request,
                target: m.target.try_into()?,
            },
            Message::Heartbeat(m) => Self::Heartbeat { session: m.session },
            Message::ReverseFiles(m) => Self::ReverseFiles {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::ReverseDragExit(m) => Self::ReverseDragExit {
                session: m.session,
                gesture: m.gesture,
                window: window(m.window)?,
                surface: m.surface,
            },
            Message::ReverseDrag(m) => Self::ReverseDrag {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::Drag(m) => Self::Drag {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::FileUpload(m) => Self::FileUpload {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::TextInput(m) => Self::TextInput {
                session: m.session,
                message: m.message.try_into()?,
            },
            Message::Hello(m) => Self::Hello {
                versions: m
                    .versions
                    .into_iter()
                    .map(|v| narrow(v, "protocol version"))
                    .collect::<Result<_>>()?,
                features: m.features.unwrap_or_default().into(),
                performance: m.performance.map(TryInto::try_into).transpose()?,
                display: m.display.map(TryInto::try_into).transpose()?,
                license: m.license.map(from_license).transpose()?,
            },
            Message::License(m) => Self::License {
                session: m.session,
                token: from_license(required(m.token, "license token")?)?,
            },
            Message::Request(m) => Self::Request {
                session: m.session,
                request: m.request,
                window: window(m.window)?,
                operation: m.operation.try_into()?,
            },
            Message::Ack(m) => Self::Ack {
                session: m.session,
                window: window(m.window)?,
                surface: m.surface,
                sequence: m.sequence,
            },
            Message::Input(m) => Self::Input {
                session: m.session,
                window: window(m.window)?,
                surface: m.surface,
                input: m.input.try_into()?,
            },
            Message::Quit(m) => Self::Quit { session: m.session },
        })
    }
}
