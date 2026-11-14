use super::*;
use crate::encoding::protobuf::generated::newport::desktop::stub as pb;
#[cfg(test)]
pub(crate) fn enum_maps() -> Vec<crate::desktop::wire::wire_tables::EnumMap> {
    use crate::desktop::wire::wire_tables::enum_map;
    vec![
        enum_map(
            "newport.desktop.stub.LaunchReason",
            |value| launch_reason_from(value).ok(),
            |value| launch_reason_to(*value),
        ),
        enum_map(
            "newport.desktop.stub.WindowKind",
            |value| window_kind_from(value).ok(),
            |value| window_kind_to(*value),
        ),
        enum_map(
            "newport.desktop.stub.Decoration",
            |value| decoration_from(value).ok(),
            |value| decoration_to(*value),
        ),
        enum_map(
            "newport.desktop.stub.Activate",
            |value| activate_from(value).ok(),
            |value| activate_to(*value),
        ),
    ]
}
fn required<T>(value: Option<T>) -> Result<T, Error> {
    value.ok_or_else(|| Invalid("required field").into())
}
fn launch_reason_to(value: LaunchReason) -> i32 {
    match value {
        LaunchReason::User => pb::LaunchReason::User as i32,
        LaunchReason::Host => pb::LaunchReason::Host as i32,
    }
}
fn launch_reason_from(value: i32) -> Result<LaunchReason, Error> {
    match pb::LaunchReason::try_from(value).map_err(|_| Invalid("enum"))? {
        pb::LaunchReason::User => Ok(LaunchReason::User),
        pb::LaunchReason::Host => Ok(LaunchReason::Host),
        _ => Err(Invalid("enum").into()),
    }
}
fn window_kind_to(value: WindowKind) -> i32 {
    match value {
        WindowKind::Normal => pb::WindowKind::Normal as i32,
        WindowKind::Dialog => pb::WindowKind::Dialog as i32,
        WindowKind::Menu => pb::WindowKind::Menu as i32,
        WindowKind::Tooltip => pb::WindowKind::Tooltip as i32,
    }
}
fn window_kind_from(value: i32) -> Result<WindowKind, Error> {
    match pb::WindowKind::try_from(value).map_err(|_| Invalid("enum"))? {
        pb::WindowKind::Normal => Ok(WindowKind::Normal),
        pb::WindowKind::Dialog => Ok(WindowKind::Dialog),
        pb::WindowKind::Menu => Ok(WindowKind::Menu),
        pb::WindowKind::Tooltip => Ok(WindowKind::Tooltip),
        _ => Err(Invalid("enum").into()),
    }
}
fn decoration_to(value: Decoration) -> i32 {
    match value {
        Decoration::Server => pb::Decoration::Server as i32,
        Decoration::Client => pb::Decoration::Client as i32,
        Decoration::None => pb::Decoration::None as i32,
    }
}
fn decoration_from(value: i32) -> Result<Decoration, Error> {
    match pb::Decoration::try_from(value).map_err(|_| Invalid("enum"))? {
        pb::Decoration::Server => Ok(Decoration::Server),
        pb::Decoration::Client => Ok(Decoration::Client),
        pb::Decoration::None => Ok(Decoration::None),
        _ => Err(Invalid("enum").into()),
    }
}
fn activate_to(value: Activate) -> i32 {
    match value {
        Activate::No => pb::Activate::No as i32,
        Activate::Yes => pb::Activate::Yes as i32,
        Activate::Attention => pb::Activate::Attention as i32,
    }
}
fn activate_from(value: i32) -> Result<Activate, Error> {
    match pb::Activate::try_from(value).map_err(|_| Invalid("enum"))? {
        pb::Activate::No => Ok(Activate::No),
        pb::Activate::Yes => Ok(Activate::Yes),
        pb::Activate::Attention => Ok(Activate::Attention),
        _ => Err(Invalid("enum").into()),
    }
}
fn actions_to(value: &Actions) -> pb::Actions {
    pb::Actions {
        close: Some(value.close),
        minimize: Some(value.minimize),
        maximize: Some(value.maximize),
        resize: Some(value.resize),
        fullscreen: Some(value.fullscreen),
    }
}
fn actions_from(value: pb::Actions) -> Result<Actions, Error> {
    let close = required(value.close)?;
    let minimize = required(value.minimize)?;
    let maximize = required(value.maximize)?;
    let resize = required(value.resize)?;
    let fullscreen = required(value.fullscreen)?;
    Ok(Actions {
        close,
        minimize,
        maximize,
        resize,
        fullscreen,
    })
}
fn window_state_to(value: &WindowState) -> pb::WindowState {
    pb::WindowState {
        window: Some(value.window),
        generation: Some(value.generation),
        kind: Some(window_kind_to(value.kind)),
        parent: Some(value.parent),
        modal: Some(value.modal),
        position: Some(pb::Point {
            x: Some(value.position[0]),
            y: Some(value.position[1]),
        }),
        logical: Some(pb::Size {
            width: Some(value.logical[0]),
            height: Some(value.logical[1]),
        }),
        pixels: Some(pb::Size {
            width: Some(value.pixels[0]),
            height: Some(value.pixels[1]),
        }),
        apply_size: Some(value.apply_size),
        decoration: Some(decoration_to(value.decoration)),
        actions: Some(actions_to(&value.actions)),
        min: value.min.map(|value| pb::Size {
            width: Some(value[0]),
            height: Some(value[1]),
        }),
        max: value.max.map(|value| pb::Size {
            width: Some(value[0]),
            height: Some(value[1]),
        }),
        title: Some(value.title.clone()),
        activate: Some(activate_to(value.activate)),
    }
}
fn window_state_from(value: pb::WindowState) -> Result<WindowState, Error> {
    let window = required(value.window)?;
    let generation = required(value.generation)?;
    let kind = required(value.kind)?;
    let parent = required(value.parent)?;
    let modal = required(value.modal)?;
    let position = required(value.position)?;
    let logical = required(value.logical)?;
    let pixels = required(value.pixels)?;
    let apply_size = required(value.apply_size)?;
    let decoration = required(value.decoration)?;
    let actions = required(value.actions)?;
    let min = value.min;
    let max = value.max;
    let title = required(value.title)?;
    let activate = required(value.activate)?;
    Ok(WindowState {
        window,
        generation,
        kind: window_kind_from(kind)?,
        parent,
        modal,
        position: [required(position.x)?, required(position.y)?],
        logical: [required(logical.width)?, required(logical.height)?],
        pixels: [required(pixels.width)?, required(pixels.height)?],
        apply_size,
        decoration: decoration_from(decoration)?,
        actions: actions_from(actions)?,
        min: min
            .map(|value| -> Result<_, Error> {
                Ok([required(value.width)?, required(value.height)?])
            })
            .transpose()?,
        max: max
            .map(|value| -> Result<_, Error> {
                Ok([required(value.width)?, required(value.height)?])
            })
            .transpose()?,
        title,
        activate: activate_from(activate)?,
    })
}
pub(super) fn to_host_to(value: &ToHost) -> pb::HostFrame {
    pb::HostFrame {
        version: Some(VERSION),
        payload: Some(match value {
            ToHost::Hello {
                version,
                reason,
                bundle_id,
            } => pb::host_frame::Payload::Hello(Box::new(pb::HostHello {
                version: Some(*version),
                reason: Some(launch_reason_to(*reason)),
                bundle_id: Some((bundle_id).clone()),
            })),
            ToHost::Pointer {
                window,
                generation,
                x,
                y,
                modifiers,
            } => pb::host_frame::Payload::Pointer(Box::new(pb::HostPointer {
                window: Some(*window),
                generation: Some(*generation),
                x: Some(*x),
                y: Some(*y),
                modifiers: Some(*modifiers),
            })),
            ToHost::Button {
                window,
                generation,
                x,
                y,
                button,
                down,
                modifiers,
            } => pb::host_frame::Payload::Button(Box::new(pb::HostButton {
                window: Some(*window),
                generation: Some(*generation),
                x: Some(*x),
                y: Some(*y),
                button: Some(u32::from(*button)),
                down: Some(*down),
                modifiers: Some(*modifiers),
            })),
            ToHost::Scroll {
                window,
                generation,
                x,
                y,
                dx,
                dy,
                precise,
                phase,
                momentum,
                modifiers,
            } => pb::host_frame::Payload::Scroll(Box::new(pb::HostScroll {
                window: Some(*window),
                generation: Some(*generation),
                x: Some(*x),
                y: Some(*y),
                dx: Some(*dx),
                dy: Some(*dy),
                precise: Some(*precise),
                phase: Some(*phase),
                momentum: Some(*momentum),
                modifiers: Some(*modifiers),
            })),
            ToHost::Key {
                window,
                generation,
                code,
                characters,
                unmodified,
                modifiers,
                repeat,
            } => pb::host_frame::Payload::Key(Box::new(pb::HostKey {
                window: Some(*window),
                generation: Some(*generation),
                code: Some(u32::from(*code)),
                characters: Some((characters).clone()),
                unmodified: Some((unmodified).clone()),
                modifiers: Some(*modifiers),
                repeat: Some(*repeat),
            })),
            ToHost::KeyUp {
                window,
                generation,
                code,
                modifiers,
            } => pb::host_frame::Payload::KeyUp(Box::new(pb::HostKeyUp {
                window: Some(*window),
                generation: Some(*generation),
                code: Some(u32::from(*code)),
                modifiers: Some(*modifiers),
            })),
            ToHost::Modifiers {
                window,
                generation,
                code,
                flags,
            } => pb::host_frame::Payload::Modifiers(Box::new(pb::HostModifiers {
                window: Some(*window),
                generation: Some(*generation),
                code: Some(u32::from(*code)),
                flags: Some(*flags),
            })),
            ToHost::Exit { window, generation } => {
                pb::host_frame::Payload::Exit(Box::new(pb::HostExit {
                    window: Some(*window),
                    generation: Some(*generation),
                }))
            }
            ToHost::Text {
                window,
                generation,
                text,
            } => pb::host_frame::Payload::Text(Box::new(pb::HostText {
                window: Some(*window),
                generation: Some(*generation),
                text: Some((text).clone()),
            })),
            ToHost::Preedit {
                window,
                generation,
                text,
                cursor,
            } => pb::host_frame::Payload::Preedit(Box::new(pb::HostPreedit {
                window: Some(*window),
                generation: Some(*generation),
                text: Some((text).clone()),
                cursor: Some(*cursor),
            })),
            ToHost::Focus { window, key } => {
                pb::host_frame::Payload::Focus(Box::new(pb::HostFocus {
                    window: Some(*window),
                    key: Some(*key),
                }))
            }
            ToHost::Resized { window, logical } => {
                pb::host_frame::Payload::Resized(Box::new(pb::HostResized {
                    window: Some(*window),
                    logical: Some(pb::Size {
                        width: Some((logical)[0]),
                        height: Some((logical)[1]),
                    }),
                }))
            }
            ToHost::CloseAsked { window } => {
                pb::host_frame::Payload::CloseAsked(Box::new(pb::HostCloseAsked {
                    window: Some(*window),
                }))
            }
            ToHost::Minimized { window, minimized } => {
                pb::host_frame::Payload::Minimized(Box::new(pb::HostMinimized {
                    window: Some(*window),
                    minimized: Some(*minimized),
                }))
            }
            ToHost::Fullscreen { window, enabled } => {
                pb::host_frame::Payload::Fullscreen(Box::new(pb::HostFullscreen {
                    window: Some(*window),
                    enabled: Some(*enabled),
                }))
            }
            ToHost::Presented {
                window,
                generation,
                index,
            } => pb::host_frame::Payload::Presented(Box::new(pb::HostPresented {
                window: Some(*window),
                generation: Some(*generation),
                index: Some(u32::from(*index)),
            })),
            ToHost::AppActive { active } => {
                pb::host_frame::Payload::AppActive(Box::new(pb::HostAppActive {
                    active: Some(*active),
                }))
            }
            ToHost::QuitAsked => pb::host_frame::Payload::QuitAsked(Box::new(pb::Empty {})),
            ToHost::Reopen => pb::host_frame::Payload::Reopen(Box::new(pb::Empty {})),
            ToHost::Display {
                window,
                scale,
                work_area,
                content,
            } => pb::host_frame::Payload::Display(Box::new(pb::HostDisplay {
                window: Some(*window),
                scale: Some(*scale),
                work_area: Some(pb::Area {
                    x: Some((work_area)[0]),
                    y: Some((work_area)[1]),
                    width: Some((work_area)[2]),
                    height: Some((work_area)[3]),
                }),
                content: Some(pb::Position {
                    x: Some((content)[0]),
                    y: Some((content)[1]),
                }),
            })),
            ToHost::Visible { window, visible } => {
                pb::host_frame::Payload::Visible(Box::new(pb::HostVisible {
                    window: Some(*window),
                    visible: Some(*visible),
                }))
            }
        }),
    }
}
pub(super) fn to_host_from(value: pb::HostFrame) -> Result<ToHost, Error> {
    if required(value.version)? != VERSION {
        return Err(Invalid("version").into());
    }
    Ok(match required(value.payload)? {
        pb::host_frame::Payload::Hello(value) => {
            let version = required(value.version)?;
            let reason = required(value.reason)?;
            let bundle_id = required(value.bundle_id)?;
            ToHost::Hello {
                version,
                reason: launch_reason_from(reason)?,
                bundle_id,
            }
        }
        pb::host_frame::Payload::Pointer(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let x = required(value.x)?;
            let y = required(value.y)?;
            let modifiers = required(value.modifiers)?;
            ToHost::Pointer {
                window,
                generation,
                x,
                y,
                modifiers,
            }
        }
        pb::host_frame::Payload::Button(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let x = required(value.x)?;
            let y = required(value.y)?;
            let button = required(value.button)?;
            let down = required(value.down)?;
            let modifiers = required(value.modifiers)?;
            ToHost::Button {
                window,
                generation,
                x,
                y,
                button: button.try_into().map_err(|_| Invalid("integer range"))?,
                down,
                modifiers,
            }
        }
        pb::host_frame::Payload::Scroll(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let x = required(value.x)?;
            let y = required(value.y)?;
            let dx = required(value.dx)?;
            let dy = required(value.dy)?;
            let precise = required(value.precise)?;
            let phase = required(value.phase)?;
            let momentum = required(value.momentum)?;
            let modifiers = required(value.modifiers)?;
            ToHost::Scroll {
                window,
                generation,
                x,
                y,
                dx,
                dy,
                precise,
                phase,
                momentum,
                modifiers,
            }
        }
        pb::host_frame::Payload::Key(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let code = required(value.code)?;
            let characters = required(value.characters)?;
            let unmodified = required(value.unmodified)?;
            let modifiers = required(value.modifiers)?;
            let repeat = required(value.repeat)?;
            ToHost::Key {
                window,
                generation,
                code: code.try_into().map_err(|_| Invalid("integer range"))?,
                characters,
                unmodified,
                modifiers,
                repeat,
            }
        }
        pb::host_frame::Payload::KeyUp(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let code = required(value.code)?;
            let modifiers = required(value.modifiers)?;
            ToHost::KeyUp {
                window,
                generation,
                code: code.try_into().map_err(|_| Invalid("integer range"))?,
                modifiers,
            }
        }
        pb::host_frame::Payload::Modifiers(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let code = required(value.code)?;
            let flags = required(value.flags)?;
            ToHost::Modifiers {
                window,
                generation,
                code: code.try_into().map_err(|_| Invalid("integer range"))?,
                flags,
            }
        }
        pb::host_frame::Payload::Exit(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            ToHost::Exit { window, generation }
        }
        pb::host_frame::Payload::Text(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let text = required(value.text)?;
            ToHost::Text {
                window,
                generation,
                text,
            }
        }
        pb::host_frame::Payload::Preedit(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let text = required(value.text)?;
            let cursor = required(value.cursor)?;
            ToHost::Preedit {
                window,
                generation,
                text,
                cursor,
            }
        }
        pb::host_frame::Payload::Focus(value) => {
            let window = required(value.window)?;
            let key = required(value.key)?;
            ToHost::Focus { window, key }
        }
        pb::host_frame::Payload::Resized(value) => {
            let window = required(value.window)?;
            let logical = required(value.logical)?;
            ToHost::Resized {
                window,
                logical: [required(logical.width)?, required(logical.height)?],
            }
        }
        pb::host_frame::Payload::CloseAsked(value) => {
            let window = required(value.window)?;
            ToHost::CloseAsked { window }
        }
        pb::host_frame::Payload::Minimized(value) => {
            let window = required(value.window)?;
            let minimized = required(value.minimized)?;
            ToHost::Minimized { window, minimized }
        }
        pb::host_frame::Payload::Fullscreen(value) => {
            let window = required(value.window)?;
            let enabled = required(value.enabled)?;
            ToHost::Fullscreen { window, enabled }
        }
        pb::host_frame::Payload::Presented(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let index = required(value.index)?;
            ToHost::Presented {
                window,
                generation,
                index: index.try_into().map_err(|_| Invalid("integer range"))?,
            }
        }
        pb::host_frame::Payload::AppActive(value) => {
            let active = required(value.active)?;
            ToHost::AppActive { active }
        }
        pb::host_frame::Payload::QuitAsked(_) => ToHost::QuitAsked,
        pb::host_frame::Payload::Reopen(_) => ToHost::Reopen,
        pb::host_frame::Payload::Display(value) => {
            let window = required(value.window)?;
            let scale = required(value.scale)?;
            let work_area = required(value.work_area)?;
            let content = required(value.content)?;
            ToHost::Display {
                window,
                scale,
                work_area: [
                    required(work_area.x)?,
                    required(work_area.y)?,
                    required(work_area.width)?,
                    required(work_area.height)?,
                ],
                content: [required(content.x)?, required(content.y)?],
            }
        }
        pb::host_frame::Payload::Visible(value) => {
            let window = required(value.window)?;
            let visible = required(value.visible)?;
            ToHost::Visible { window, visible }
        }
    })
}
pub(super) fn to_stub_to(value: &ToStub) -> pb::StubFrame {
    pb::StubFrame {
        version: Some(VERSION),
        payload: Some(match value {
            ToStub::Welcome { name, server } => {
                pb::stub_frame::Payload::Welcome(Box::new(pb::StubWelcome {
                    name: Some((name).clone()),
                    server: Some((server).clone()),
                }))
            }
            ToStub::Window { state, surfaces } => {
                pb::stub_frame::Payload::Window(Box::new(pb::StubWindow {
                    state: Some(window_state_to(state)),
                    surfaces: Some(u32::from(*surfaces)),
                }))
            }
            ToStub::Surface {
                window,
                generation,
                index,
            } => pb::stub_frame::Payload::Surface(Box::new(pb::StubSurface {
                window: Some(*window),
                generation: Some(*generation),
                index: Some(u32::from(*index)),
            })),
            ToStub::Frame {
                window,
                generation,
                index,
            } => pb::stub_frame::Payload::Frame(Box::new(pb::StubPresentFrame {
                window: Some(*window),
                generation: Some(*generation),
                index: Some(u32::from(*index)),
            })),
            ToStub::Title { window, title } => {
                pb::stub_frame::Payload::Title(Box::new(pb::StubTitle {
                    window: Some(*window),
                    title: Some((title).clone()),
                }))
            }
            ToStub::Operation { window, operation } => {
                pb::stub_frame::Payload::Operation(Box::new(pb::StubOperation {
                    window: Some(*window),
                    operation: Some(operation_to(operation)),
                }))
            }
            ToStub::Destroy { window } => {
                pb::stub_frame::Payload::Destroy(Box::new(pb::StubDestroy {
                    window: Some(*window),
                }))
            }
            ToStub::QuitReply { allow } => {
                pb::stub_frame::Payload::QuitReply(Box::new(pb::StubQuitReply {
                    allow: Some(*allow),
                }))
            }
            ToStub::Alert { message, detail } => {
                pb::stub_frame::Payload::Alert(Box::new(pb::StubAlert {
                    message: Some((message).clone()),
                    detail: Some((detail).clone()),
                }))
            }
            ToStub::Quit => pb::stub_frame::Payload::Quit(Box::new(pb::Empty {})),
            ToStub::TextContext {
                window,
                editing,
                caret,
            } => pb::stub_frame::Payload::TextContext(Box::new(pb::StubTextContext {
                window: Some(*window),
                editing: Some(*editing),
                caret: (caret).map(|value| pb::Point {
                    x: Some(value[0]),
                    y: Some(value[1]),
                }),
            })),
            ToStub::Cursor { window, cursor } => {
                pb::stub_frame::Payload::Cursor(Box::new(pb::StubCursor {
                    window: Some(*window),
                    cursor: Some(u32::from(*cursor)),
                }))
            }
        }),
    }
}
pub(super) fn to_stub_from(value: pb::StubFrame) -> Result<ToStub, Error> {
    if required(value.version)? != VERSION {
        return Err(Invalid("version").into());
    }
    Ok(match required(value.payload)? {
        pb::stub_frame::Payload::Welcome(value) => {
            let name = required(value.name)?;
            let server = required(value.server)?;
            ToStub::Welcome { name, server }
        }
        pb::stub_frame::Payload::Window(value) => {
            let state = required(value.state)?;
            let surfaces = required(value.surfaces)?;
            ToStub::Window {
                state: window_state_from(state)?,
                surfaces: surfaces.try_into().map_err(|_| Invalid("integer range"))?,
            }
        }
        pb::stub_frame::Payload::Surface(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let index = required(value.index)?;
            ToStub::Surface {
                window,
                generation,
                index: index.try_into().map_err(|_| Invalid("integer range"))?,
            }
        }
        pb::stub_frame::Payload::Frame(value) => {
            let window = required(value.window)?;
            let generation = required(value.generation)?;
            let index = required(value.index)?;
            ToStub::Frame {
                window,
                generation,
                index: index.try_into().map_err(|_| Invalid("integer range"))?,
            }
        }
        pb::stub_frame::Payload::Title(value) => {
            let window = required(value.window)?;
            let title = required(value.title)?;
            ToStub::Title { window, title }
        }
        pb::stub_frame::Payload::Operation(value) => {
            let window = required(value.window)?;
            let operation = required(value.operation)?;
            ToStub::Operation {
                window,
                operation: operation_from(operation)?,
            }
        }
        pb::stub_frame::Payload::Destroy(value) => {
            let window = required(value.window)?;
            ToStub::Destroy { window }
        }
        pb::stub_frame::Payload::QuitReply(value) => {
            let allow = required(value.allow)?;
            ToStub::QuitReply { allow }
        }
        pb::stub_frame::Payload::Alert(value) => {
            let message = required(value.message)?;
            let detail = required(value.detail)?;
            ToStub::Alert { message, detail }
        }
        pb::stub_frame::Payload::Quit(_) => ToStub::Quit,
        pb::stub_frame::Payload::TextContext(value) => {
            let window = required(value.window)?;
            let editing = required(value.editing)?;
            let caret = value.caret;
            ToStub::TextContext {
                window,
                editing,
                caret: caret
                    .map(|value| -> Result<_, Error> {
                        Ok([required(value.x)?, required(value.y)?])
                    })
                    .transpose()?,
            }
        }
        pb::stub_frame::Payload::Cursor(value) => {
            let window = required(value.window)?;
            let cursor = required(value.cursor)?;
            ToStub::Cursor {
                window,
                cursor: cursor.try_into().map_err(|_| Invalid("integer range"))?,
            }
        }
    })
}
pub(super) fn operation_to(value: &Operation) -> pb::OperationValue {
    pb::OperationValue {
        payload: Some(match value {
            Operation::Move => pb::operation_value::Payload::Move(Box::new(pb::Empty {})),
            Operation::Minimize => pb::operation_value::Payload::Minimize(Box::new(pb::Empty {})),
            Operation::Zoom { maximized } => {
                pb::operation_value::Payload::Zoom(Box::new(pb::OperationZoom {
                    maximized: Some(*maximized),
                }))
            }
            Operation::Fullscreen { enabled } => {
                pb::operation_value::Payload::Fullscreen(Box::new(pb::OperationFullscreen {
                    enabled: Some(*enabled),
                }))
            }
            Operation::Activate => pb::operation_value::Payload::Activate(Box::new(pb::Empty {})),
        }),
    }
}
pub(super) fn operation_from(value: pb::OperationValue) -> Result<Operation, Error> {
    Ok(match required(value.payload)? {
        pb::operation_value::Payload::Move(_) => Operation::Move,
        pb::operation_value::Payload::Minimize(_) => Operation::Minimize,
        pb::operation_value::Payload::Zoom(value) => {
            let maximized = required(value.maximized)?;
            Operation::Zoom { maximized }
        }
        pb::operation_value::Payload::Fullscreen(value) => {
            let enabled = required(value.enabled)?;
            Operation::Fullscreen { enabled }
        }
        pb::operation_value::Payload::Activate(_) => Operation::Activate,
    })
}
