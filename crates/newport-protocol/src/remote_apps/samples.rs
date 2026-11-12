//! One sample of every message, both directions, for the round trip and the
//! revision check.
use crate::display::attachment as envelope;
use crate::remote_apps::{
    audio, display, drag, features, file_tree, file_upload, performance, reverse_drag,
    reverse_files, session_activity, session_application_lifecycle as lifecycle,
    session_applications, session_launch, sizing, text_input, tile, v1::*, window_application,
};

const SESSION: u64 = 0x9e37_79b9_7f4a_7c15;

fn window_id() -> WindowId {
    WindowId {
        id: 3,
        generation: 2,
    }
}
fn scale() -> Scale {
    Scale {
        numerator: 3,
        denominator: 2,
    }
}
fn surface() -> Surface {
    Surface {
        generation: 4,
        logical_width: 800,
        logical_height: 600,
        pixel_width: 1200,
        pixel_height: 900,
        scale: scale(),
    }
}
fn window() -> Window {
    Window {
        key: window_id(),
        parent: Some(WindowId {
            id: 1,
            generation: 1,
        }),
        kind: WindowKind::Dialog,
        modal: true,
        position: [-12, 40],
        title: "Save As".into(),
        decoration: Decoration::Server,
        actions: Actions {
            close: true,
            minimize: false,
            maximize: true,
            resize: true,
            fullscreen: false,
        },
        constraints: Constraints {
            physical: Some(sizing::PixelConstraints {
                min: Some([100, 80]),
                max: Some([4000, 3000]),
                base: Some([2, 4]),
                increment: Some([8, 16]),
                aspect: Some([[1, 2], [2, 1]]),
            }),
            min: Some([50, 40]),
            max: None,
        },
        minimized: false,
        maximized: true,
        fullscreen: false,
        application: window_application::WindowApplication::new(
            Some("org.gnome.TextEditor".into()),
            Some("org.gnome.TextEditor.desktop".into()),
            Some(5),
        ),
    }
}
fn media_time() -> MediaTime {
    MediaTime {
        earliest_us: -20,
        latest_us: 1_000_000,
    }
}
fn profile() -> display::DisplayProfile {
    display::DisplayProfile {
        scale: scale(),
        work_area: [1512, 945],
    }
}
fn context() -> text_input::Context {
    text_input::Context {
        window: window_id(),
        revision: 9,
        focus_epoch: 7,
        preedit: true,
        caret: Some([-4, 18]),
    }
}
fn files_context() -> reverse_files::Context {
    reverse_files::Context {
        drag: 8,
        gesture: 6,
        window: window_id(),
        surface: 4,
    }
}
fn upload_chunk() -> file_upload::Client {
    file_upload::Client::Chunk {
        transfer: 8,
        file: 1,
        offset: 4096,
        data: vec![7; 64].into(),
    }
}
fn tree() -> Vec<file_tree::Entry> {
    vec![
        file_tree::Entry {
            parent: None,
            name: "photos".into(),
            contents: file_tree::Contents::Directory {},
        },
        file_tree::Entry {
            parent: Some(0),
            name: "a.jpg".into(),
            contents: file_tree::Contents::File { bytes: 12_345 },
        },
    ]
}
fn lifecycle_window() -> lifecycle::Window {
    lifecycle::Window {
        window: window_id(),
        surface: 4,
        existing_instance: true,
    }
}
fn update(revision: u64, state: lifecycle::State) -> ServerMessage {
    ServerMessage::ApplicationLifecycle(lifecycle::Update {
        session: SESSION,
        request: 5,
        revision,
        state,
    })
}

/// Every cursor shape.
pub fn cursor_shapes() -> Vec<CursorShape> {
    use CursorShape::*;
    vec![
        Default,
        ContextMenu,
        Help,
        Pointer,
        Progress,
        Wait,
        Cell,
        Crosshair,
        Text,
        VerticalText,
        Alias,
        Copy,
        Move,
        NoDrop,
        NotAllowed,
        Grab,
        Grabbing,
        EResize,
        NResize,
        NeResize,
        NwResize,
        SResize,
        SeResize,
        SwResize,
        WResize,
        EwResize,
        NsResize,
        NeswResize,
        NwseResize,
        ColResize,
        RowResize,
        AllScroll,
        ZoomIn,
        ZoomOut,
    ]
}

/// What the display sends, every message and every variant of what they hold.
pub fn server() -> Vec<ServerMessage> {
    let mut tiles = vec![
        tile::Tile::new(0, tile::Codec::Lz4, vec![1, 2, 3]),
        tile::Tile {
            rect: Some([0, 16, 256, 32]),
            copy: Some(tile::TileCopy {
                source: tile::CopySource::Surface { source_y: 300 },
                dy: 0,
                h: 16,
            }),
            ..tile::Tile::new(1, tile::Codec::Copy, Vec::new())
        },
        tile::Tile {
            copy: Some(tile::TileCopy {
                source: tile::CopySource::Tile { sy: 4 },
                dy: 2,
                h: 8,
            }),
            ..tile::Tile::new(2, tile::Codec::Copy, Vec::new())
        },
        tile::Tile {
            video: Some(tile::VideoGroup {
                rect: [0, 256, 512, 256],
                stream: 2,
                sequence: 1,
                keyframe: true,
            }),
            ..tile::Tile::new(3, tile::Codec::Av1, vec![9; 32])
        },
    ];
    tiles.extend(
        tile::Codec::ALL
            .into_iter()
            .enumerate()
            .map(|(id, codec)| tile::Tile::new(4 + id, codec, vec![id as u8])),
    );
    let mut messages = vec![
        ServerMessage::Welcome {
            session: SESSION,
            version: VERSION,
            features: features::Features::all(),
            performance: Some(performance::Settings {
                codec_mode: performance::CodecMode::Lossless,
                max_fps: 120,
                jpeg_quality: 80,
                fast_trial: true,
            }),
        },
        ServerMessage::Welcome {
            session: SESSION,
            version: VERSION,
            features: features::Features::default(),
            performance: None,
        },
        ServerMessage::Rejected {
            reason: "expired".into(),
            message: "The license ended.".into(),
            display: None,
        },
        ServerMessage::Rejected {
            reason: envelope::DISPLAY_REVISION.into(),
            message: "Restart the server's apps.".into(),
            display: Some(envelope::DisplayIdentity {
                revision: 1,
                build: "a1b2".into(),
            }),
        },
        ServerMessage::WindowCreated {
            session: SESSION,
            window: window(),
            surface: surface(),
            moved: true,
        },
        ServerMessage::WindowUpdated {
            session: SESSION,
            window: Window {
                parent: None,
                application: None,
                kind: WindowKind::Normal,
                modal: false,
                ..window()
            },
        },
        ServerMessage::SurfaceChanged {
            session: SESSION,
            window: window_id(),
            surface: surface(),
        },
        ServerMessage::Frame {
            session: SESSION,
            window: window_id(),
            surface: 4,
            sequence: 77,
            keyframe: false,
            tiles,
            captured: Some(media_time()),
            diagnostics: Some(FrameDiagnostics {
                target_fps: 60,
                fast_codec: true,
                ack_ms: 1.5,
                work_ms: 2.25,
                capture_ms: 0.5,
                dispatch_ms: 0.125,
            }),
        },
        ServerMessage::Audio {
            session: SESSION,
            sequence: 3,
            sample_time: 1920,
            captured: Some(media_time()),
            hole: true,
            epoch: 2,
            data: vec![0xfc; 120].into(),
        },
        ServerMessage::AudioState {
            session: SESSION,
            state: audio::State::Waiting {
                reason: "No sound server".into(),
            },
        },
        ServerMessage::AudioState {
            session: SESSION,
            state: audio::State::Playing,
        },
        ServerMessage::Cursor {
            session: SESSION,
            window: window_id(),
            cursor: Cursor::Hidden,
        },
        ServerMessage::Error {
            session: SESSION,
            message: "Invalid window".into(),
        },
        ServerMessage::WindowDestroyed {
            session: SESSION,
            window: window_id(),
        },
        ServerMessage::OperationResult {
            session: SESSION,
            request: 12,
            window: window_id(),
            applied: false,
            message: Some("Resize violates window constraints".into()),
        },
        ServerMessage::OperationResult {
            session: SESSION,
            request: 13,
            window: window_id(),
            applied: true,
            message: None,
        },
        ServerMessage::Closed { session: SESSION },
        ServerMessage::Ended {
            session: SESSION,
            reason: ENDED_TAKEN_OVER.into(),
            message: "Shown on studio-mac.".into(),
            computer: Some("studio-mac".into()),
            computer_session: Some("s-2".into()),
        },
        ServerMessage::TextInput {
            session: SESSION,
            message: text_input::Server::Focus {
                context: Some(context()),
            },
        },
        ServerMessage::TextInput {
            session: SESSION,
            message: text_input::Server::Focus { context: None },
        },
        ServerMessage::TextInput {
            session: SESSION,
            message: text_input::Server::Result {
                request: 4,
                applied: true,
            },
        },
        ServerMessage::Drag {
            session: SESSION,
            message: drag::Server::Status {
                drag: 2,
                revision: 3,
                accepted: true,
            },
        },
        ServerMessage::Drag {
            session: SESSION,
            message: drag::Server::Finished {
                drag: 2,
                copied: true,
                reason: String::new(),
            },
        },
        ServerMessage::FileUpload {
            session: SESSION,
            message: file_upload::Server::Ack {
                transfer: 8,
                file: 1,
                offset: 8192,
            },
        },
        ServerMessage::FileUpload {
            session: SESSION,
            message: file_upload::Server::Ready { transfer: 8 },
        },
        ServerMessage::FileUpload {
            session: SESSION,
            message: file_upload::Server::Cancelled { transfer: 8 },
        },
        ServerMessage::FileUpload {
            session: SESSION,
            message: file_upload::Server::Rejected {
                transfer: 8,
                reason: "Disk full".into(),
            },
        },
        ServerMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Server::Offer {
                drag: 8,
                gesture: 6,
                window: window_id(),
                surface: 4,
                mime: "text/plain;charset=utf-8".into(),
                bytes: 5000,
            },
        },
        ServerMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Server::Chunk {
                drag: 8,
                offset: 4096,
                data: vec![1; 904].into(),
            },
        },
        ServerMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Server::Drop { drag: 8 },
        },
        ServerMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Server::Cancel { drag: 8 },
        },
        ServerMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Server::ExitRefused { gesture: 6 },
        },
        ServerMessage::ReverseFiles {
            session: SESSION,
            message: reverse_files::Server {
                context: files_context(),
                message: file_upload::Client::BeginTree {
                    transfer: 8,
                    entries: tree(),
                },
            },
        },
        ServerMessage::SessionApplications {
            session: SESSION,
            snapshot: session_applications::Snapshot {
                revision: 4,
                applications: vec![
                    session_applications::Application {
                        request: 1,
                        state: session_applications::State::Opening,
                    },
                    session_applications::Application {
                        request: 2,
                        state: session_applications::State::Window {
                            window: window_id(),
                        },
                    },
                    session_applications::Application {
                        request: 3,
                        state: session_applications::State::Stopping,
                    },
                ],
            },
        },
        ServerMessage::SessionActivity {
            session: SESSION,
            activity: session_activity::Snapshot {
                revision: 4,
                applications: 2,
            },
        },
        ServerMessage::LaunchResult {
            session: SESSION,
            request: 5,
            outcome: session_launch::Outcome::Accepted,
        },
        ServerMessage::LaunchResult {
            session: SESSION,
            request: 6,
            outcome: session_launch::Outcome::Failed {
                message: "No such application".into(),
            },
        },
        update(
            1,
            lifecycle::State::Starting {
                stage: lifecycle::Stage::ActivationCall,
            },
        ),
        update(2, lifecycle::State::Background { waited_ms: 5000 }),
        update(3, lifecycle::State::Window(lifecycle_window())),
    ];
    for (revision, outcome) in [
        lifecycle::Outcome::Presented(lifecycle_window()),
        lifecycle::Outcome::CompletedWithoutPresentation {
            output: Some("done".into()),
        },
        lifecycle::Outcome::CompletedWithoutPresentation { output: None },
        lifecycle::Outcome::HandedOff {},
        lifecycle::Outcome::Failed {
            stage: lifecycle::Stage::Command,
            message: "exit status 1".into(),
            outcome_unknown: true,
        },
        lifecycle::Outcome::TimedOut {
            stage: lifecycle::Stage::AwaitingPresentation,
            outcome_unknown: false,
        },
        lifecycle::Outcome::Cancelled {
            outcome_unknown: true,
        },
    ]
    .into_iter()
    .enumerate()
    {
        messages.push(update(
            4 + revision as u64,
            lifecycle::State::Finished { outcome },
        ));
    }
    for stage in [
        lifecycle::Stage::Preparing,
        lifecycle::Stage::ServiceLookup,
        lifecycle::Stage::ServiceRegistration,
        lifecycle::Stage::Command,
        lifecycle::Stage::AwaitingWindow,
    ] {
        messages.push(update(20, lifecycle::State::Starting { stage }));
    }
    for shape in cursor_shapes() {
        messages.push(ServerMessage::Cursor {
            session: SESSION,
            window: window_id(),
            cursor: Cursor::Shape { shape },
        });
    }
    for kind in [WindowKind::Menu, WindowKind::Tooltip] {
        messages.push(ServerMessage::WindowUpdated {
            session: SESSION,
            window: Window {
                kind,
                decoration: Decoration::None,
                modal: false,
                ..window()
            },
        });
    }
    for action in operations() {
        messages.push(ServerMessage::WindowAction {
            session: SESSION,
            window: window_id(),
            action,
        });
    }
    messages
}

fn operations() -> Vec<Operation> {
    let mut operations = vec![
        Operation::Activate,
        Operation::Move,
        Operation::Close,
        Operation::Minimize,
        Operation::Maximize,
        Operation::Restore,
        Operation::Fullscreen { enabled: true },
        Operation::ShowWindowMenu { x: -3, y: 9 },
        Operation::Resize {
            logical_width: 640,
            logical_height: 480,
            scale: scale(),
        },
        Operation::SetDisplay { profile: profile() },
        Operation::SetPopupBounds {
            bounds: display::PopupBounds {
                origin: [-400, -20],
                size: [1600, 900],
            },
        },
    ];
    operations.extend(
        [
            ResizeEdge::Top,
            ResizeEdge::Bottom,
            ResizeEdge::Left,
            ResizeEdge::Right,
            ResizeEdge::TopLeft,
            ResizeEdge::TopRight,
            ResizeEdge::BottomLeft,
            ResizeEdge::BottomRight,
        ]
        .map(|edge| Operation::StartResize { edge }),
    );
    operations
}

/// What a client sends, every message and every variant of what they hold.
pub fn client() -> Vec<ClientMessage> {
    let modifiers = Modifiers {
        shift: true,
        control: true,
        alt: true,
        logo: true,
        caps_lock: true,
    };
    let mut inputs = vec![
        Input::Focus,
        Input::Blur,
        Input::Leave,
        Input::Pointer {
            x: 640,
            y: 360,
            modifiers: Modifiers::default(),
        },
        Input::HeldPointer {
            x: -20,
            y: 4000,
            modifiers,
        },
        Input::Key {
            key: "é".into(),
            code: 18,
            down: true,
            repeat: true,
            modifiers,
        },
    ];
    for button in [
        Button::Left,
        Button::Middle,
        Button::Right,
        Button::Back,
        Button::Forward,
    ] {
        inputs.push(Input::Button {
            button,
            down: true,
            modifiers,
        });
    }
    for phase in [
        ScrollPhase::Wheel,
        ScrollPhase::Begin,
        ScrollPhase::Update,
        ScrollPhase::End,
        ScrollPhase::Momentum,
        ScrollPhase::MomentumEnd,
    ] {
        inputs.push(Input::Scroll {
            position: [21, 31],
            dx: -0.5,
            dy: 12.25,
            discrete: Some([0, -120]),
            phase,
            modifiers: Modifiers::default(),
        });
    }
    let mut messages = vec![
        ClientMessage::Hello {
            versions: vec![VERSION],
            features: features::Features::all(),
            performance: Some(performance::Settings::default()),
            display: Some(profile()),
            license: Some(LicenseToken("v4.public.sample".into())),
        },
        ClientMessage::Hello {
            versions: vec![VERSION, 2],
            features: features::Features::default(),
            performance: None,
            display: None,
            license: None,
        },
        ClientMessage::License {
            session: SESSION,
            token: LicenseToken("v4.public.renewed".into()),
        },
        ClientMessage::Heartbeat { session: SESSION },
        ClientMessage::Quit { session: SESSION },
        ClientMessage::Ack {
            session: SESSION,
            window: window_id(),
            surface: 4,
            sequence: 77,
        },
        ClientMessage::ReverseDragExit {
            session: SESSION,
            gesture: 6,
            window: window_id(),
            surface: 4,
        },
        ClientMessage::Launch {
            session: SESSION,
            request: 5,
            target: session_launch::Target::Command {
                argv: vec!["gnome-text-editor".into(), "--new-window".into()],
            },
        },
        ClientMessage::Launch {
            session: SESSION,
            request: 6,
            target: session_launch::Target::Desktop {
                id: "org.gnome.TextEditor.desktop".into(),
                uris: vec!["file:///home/a.txt".into()],
                action: Some("new-window".into()),
            },
        },
        ClientMessage::StopApplication {
            session: SESSION,
            request: 5,
        },
        ClientMessage::ApplicationPresented(lifecycle::Presented {
            session: SESSION,
            request: 5,
            window: window_id(),
            surface: 4,
        }),
        ClientMessage::TextInput {
            session: SESSION,
            message: text_input::Client::Preedit {
                request: 3,
                context: context(),
                text: "にほ".into(),
                cursor: 1,
            },
        },
        ClientMessage::TextInput {
            session: SESSION,
            message: text_input::Client::Commit {
                request: 4,
                context: text_input::Context {
                    caret: None,
                    preedit: false,
                    ..context()
                },
                text: "日本".into(),
            },
        },
        ClientMessage::Drag {
            session: SESSION,
            message: drag::Client::Begin {
                drag: 2,
                transfer: 8,
                window: window_id(),
                surface: 4,
                point: [10, 20],
            },
        },
        ClientMessage::Drag {
            session: SESSION,
            message: drag::Client::Motion {
                drag: 2,
                point: [11, 21],
            },
        },
        ClientMessage::Drag {
            session: SESSION,
            message: drag::Client::Drop { drag: 2 },
        },
        ClientMessage::Drag {
            session: SESSION,
            message: drag::Client::Cancel { drag: 2 },
        },
        ClientMessage::FileUpload {
            session: SESSION,
            message: file_upload::Client::Begin {
                transfer: 8,
                files: vec![file_upload::FileSpec {
                    name: "notes.txt".into(),
                    bytes: 100,
                }],
            },
        },
        ClientMessage::FileUpload {
            session: SESSION,
            message: file_upload::Client::BeginTree {
                transfer: 9,
                entries: tree(),
            },
        },
        ClientMessage::FileUpload {
            session: SESSION,
            message: upload_chunk(),
        },
        ClientMessage::FileUpload {
            session: SESSION,
            message: file_upload::Client::Finish { transfer: 8 },
        },
        ClientMessage::FileUpload {
            session: SESSION,
            message: file_upload::Client::Cancel { transfer: 8 },
        },
        ClientMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Client::Read {
                drag: 8,
                offset: 4096,
            },
        },
        ClientMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Client::Ready { drag: 8 },
        },
        ClientMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Client::Complete {
                drag: 8,
                copied: true,
            },
        },
        ClientMessage::ReverseDrag {
            session: SESSION,
            message: reverse_drag::Client::Cancel { drag: 8 },
        },
        ClientMessage::ReverseFiles {
            session: SESSION,
            message: reverse_files::Client {
                context: files_context(),
                message: file_upload::Server::Ack {
                    transfer: 8,
                    file: 1,
                    offset: 0,
                },
            },
        },
    ];
    for input in inputs {
        messages.push(ClientMessage::Input {
            session: SESSION,
            window: window_id(),
            surface: 4,
            input,
        });
    }
    for (request, operation) in operations().into_iter().enumerate() {
        messages.push(ClientMessage::Request {
            session: SESSION,
            request: request as u64 + 1,
            window: window_id(),
            operation,
        });
    }
    messages
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn json(value: &impl serde::Serialize) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }
    fn variant(value: &serde_json::Value) -> String {
        value
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("application_lifecycle")
            .to_owned()
    }

    #[test]
    fn every_message_round_trips() {
        let mut variants = BTreeSet::new();
        for message in server() {
            let wire = crate::remote_apps::wire::encode_server(&message).unwrap();
            let (read, size) = crate::remote_apps::wire::read_server(&mut wire.as_slice()).unwrap();
            assert_eq!(size, wire.len());
            assert_eq!(json(&read), json(&message), "{message:?}");
            variants.insert(variant(&json(&message)));
        }
        assert_eq!(variants.len(), 24, "{variants:?}");
        let mut variants = BTreeSet::new();
        for message in client() {
            let wire = crate::remote_apps::wire::encode_client(&message).unwrap();
            let (read, size) = crate::remote_apps::wire::read_client(&mut wire.as_slice()).unwrap();
            assert_eq!(size, wire.len());
            assert_eq!(json(&read), json(&message), "{message:?}");
            variants.insert(variant(&json(&message)));
        }
        assert_eq!(variants.len(), 16, "{variants:?}");
    }

    #[test]
    fn the_samples_are_valid() {
        for message in server() {
            let mut bytes = Vec::new();
            write_server(&mut bytes, &message).unwrap();
        }
    }

    /// The samples' bytes, by FNV-1a: stable across Rust releases.
    fn fingerprint() -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let server = server()
            .into_iter()
            .map(|m| crate::remote_apps::wire::encode_server(&m));
        let client = client()
            .into_iter()
            .map(|m| crate::remote_apps::wire::encode_client(&m));
        for bytes in server.chain(client) {
            for byte in bytes.unwrap() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        hash
    }

    /// Reviewed wire samples for the current version-one contract.
    const WIRE_FINGERPRINT: u64 = 0x02e7_8eae_2424_7821;

    #[test]
    fn the_wire_matches_the_reviewed_contract() {
        assert_eq!(crate::remote_apps::REVISION, 1);
        assert_eq!(
            fingerprint(),
            WIRE_FINGERPRINT,
            "Review the wire change and update WIRE_FINGERPRINT. \
             Before Newport 1.0, protocol changes stay at version 1.",
        );
    }
}
