//! Unmodified arboard in Linux, driven through SSH and the real agent protocol.
use super::*;
use image::ImageEncoder;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};
type Stream = russh::ChannelStream<russh::client::Msg>;

#[derive(Clone, Copy, Debug)]
enum Backend {
    X11,
    Wayland,
}
struct Probe(BufReader<Stream>);
impl Probe {
    async fn start(session: &ExecSession, backend: Backend) -> Self {
        let isolate = match backend {
            Backend::X11 => "unset WAYLAND_DISPLAY WAYLAND_SOCKET; test -n \"$DISPLAY\" && test -f \"$XAUTHORITY\" || exit 1",
            Backend::Wayland => "unset DISPLAY XAUTHORITY WAYLAND_SOCKET; test -S \"$WAYLAND_DISPLAY\" || exit 1",
        };
        let stream = session.stream(&format!("eval \"$(~/.local/bin/newport-agent env)\"; {isolate}; exec arboard-probe read-sequence")).await.unwrap();
        let mut probe = Self(BufReader::new(stream));
        let ready = probe.next().await;
        assert_eq!(ready["ready"], true, "{backend:?}: {ready}");
        assert_eq!(ready["arboard"], "3.6.1");
        assert!(ready["pid"].as_u64().is_some());
        probe
    }
    async fn command(&mut self, command: &str) {
        self.0
            .get_mut()
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn next(&mut self) -> Value {
        let mut line = String::new();
        let size = timeout(Duration::from_secs(20), self.0.read_line(&mut line))
            .await
            .expect("arboard probe timed out")
            .unwrap();
        assert!(size > 0, "arboard probe exited before returning a result");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("invalid probe result {line:?}: {e}"))
    }
    async fn unavailable(&mut self, command: &str) {
        self.command(command).await;
        let result = self.next().await;
        assert_eq!(result["ok"], false, "unexpected clipboard data: {result}");
        assert!(result["error"].as_str().is_some_and(|s| !s.is_empty()));
    }
    async fn stop(mut self) {
        self.command("quit").await;
        let mut rest = String::new();
        timeout(Duration::from_secs(5), self.0.read_to_string(&mut rest))
            .await
            .unwrap()
            .unwrap();
        assert!(rest.is_empty(), "unexpected extra probe output: {rest}");
    }
}
async fn agent(session: &ExecSession) -> Stream {
    crate::agent::install(session).await.unwrap();
    let mut agent = session
        .stream(&format!(
            "exec ~/.local/bin/newport-agent serve {} --clipboard",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut agent, b'R').await, b"newport-agent/5");
    agent
}
async fn stop(agent: &mut Stream) {
    frame(agent, b'Q', &[]).await;
    timeout(Duration::from_secs(10), agent.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
}
async fn offer(agent: &mut Stream, revision: i64, format: &str) {
    frame(agent, b'M', format!("{revision}\n{format}").as_bytes()).await;
    receive(agent, b'A').await;
}
fn request_fields(request: &[u8], revision: i64, format: &str) -> u64 {
    let text = std::str::from_utf8(request).unwrap();
    let fields: Vec<_> = text.split('\n').collect();
    assert_eq!(fields[1], revision.to_string());
    assert_eq!(fields[2], format);
    fields[0].parse().unwrap()
}
async fn chunk(agent: &mut Stream, id: u64, revision: i64, data: &[u8], done: bool) {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data).unwrap();
    let mut bytes = Vec::new();
    bytes.extend(id.to_be_bytes());
    bytes.extend(revision.to_be_bytes());
    bytes.extend([2, u8::from(done)]);
    bytes.extend(encoder.finish().unwrap());
    frame(agent, b'D', &bytes).await;
}
async fn fetch(
    probe: &mut Probe,
    agent: &mut Stream,
    command: &str,
    revision: i64,
    format: &str,
    data: &[u8],
) -> Value {
    probe.command(command).await;
    let (result, ()) = tokio::join!(probe.next(), async {
        let request = receive(agent, b'C').await;
        let id = request_fields(&request, revision, format);
        // Leave space for zlib overhead even for incompressible pixel data.
        let chunks: Vec<_> = data.chunks(48 * 1024).collect();
        for (index, bytes) in chunks.iter().enumerate() {
            chunk(agent, id, revision, bytes, index + 1 == chunks.len()).await;
        }
    });
    result
}
struct Picture {
    png: Vec<u8>,
    expected: Value,
}
fn picture(width: u32, height: u32, mut seed: u32) -> Picture {
    let pixels: Vec<u8> = (0..width * height * 4)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .unwrap();
    let hash: String = Sha256::digest(&pixels)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Picture {
        png,
        expected: serde_json::json!({"ok":true, "width":width, "height":height, "sha256":hash}),
    }
}

async fn sequence(backend: Backend) {
    let session = ExecSession::connect(&server()).await.unwrap();
    let mut agent = agent(&session).await;
    offer(&mut agent, 1, "text/plain").await;
    let mut probe = Probe::start(&session, backend).await;
    let first = "Hello 世界 — 🦀\nsecond line";
    let result = fetch(
        &mut probe,
        &mut agent,
        "read-text",
        1,
        "text/plain",
        first.as_bytes(),
    )
    .await;
    assert_eq!(
        result,
        serde_json::json!({"ok":true,"text":first}),
        "{backend:?}"
    );
    // Same instance and same revision: this must complete without another request.
    probe.command("read-text").await;
    assert_eq!(probe.next().await, result);
    frame(&mut agent, b'H', &[]).await;
    receive(&mut agent, b'A').await;
    probe.unavailable("read-image").await;
    offer(&mut agent, 2, "text/plain").await;
    let result = fetch(
        &mut probe,
        &mut agent,
        "read-text",
        2,
        "text/plain",
        b"new text",
    )
    .await;
    assert_eq!(result["text"], "new text");
    for (revision, image) in [(3, picture(1, 1, 42)), (4, picture(384, 256, 123456789))] {
        if revision == 4 {
            assert!(image.png.len() > 256 * 1024);
        }
        offer(&mut agent, revision, "image/png").await;
        let result = fetch(
            &mut probe,
            &mut agent,
            "read-image",
            revision,
            "image/png",
            &image.png,
        )
        .await;
        assert_eq!(result, image.expected, "{backend:?} revision {revision}");
    }
    offer(&mut agent, 5, "image/png").await;
    let result = fetch(
        &mut probe,
        &mut agent,
        "read-image",
        5,
        "image/png",
        b"not a PNG",
    )
    .await;
    assert_eq!(result["ok"], false, "malformed image should fail: {result}");
    offer(&mut agent, 6, "").await;
    probe.unavailable("read-image").await;
    probe.unavailable("read-text").await;
    let image = picture(17, 9, 7);
    offer(&mut agent, 7, "image/png").await;
    assert_eq!(
        fetch(
            &mut probe,
            &mut agent,
            "read-image",
            7,
            "image/png",
            &image.png
        )
        .await,
        image.expected
    );
    probe.stop().await;
    stop(&mut agent).await;
}

async fn superseded_image(backend: Backend) {
    let session = ExecSession::connect(&server()).await.unwrap();
    let mut agent = agent(&session).await;
    offer(&mut agent, 1, "image/png").await;
    let mut probe = Probe::start(&session, backend).await;
    probe.command("read-image").await;
    let request = receive(&mut agent, b'C').await;
    let id = request_fields(&request, 1, "image/png");
    let old = picture(384, 256, 15);
    chunk(&mut agent, id, 1, &old.png[..48 * 1024], false).await;
    offer(&mut agent, 2, "image/png").await;
    // Late bytes for the superseded request cannot complete the old image.
    chunk(&mut agent, id, 1, b"late old bytes", true).await;
    assert_eq!(probe.next().await["ok"], false);
    let current = picture(23, 19, 16);
    assert_eq!(
        fetch(
            &mut probe,
            &mut agent,
            "read-image",
            2,
            "image/png",
            &current.png
        )
        .await,
        current.expected
    );
    probe.stop().await;
    stop(&mut agent).await;
}

async fn restart(backend: Backend) {
    let session = ExecSession::connect(&server()).await.unwrap();
    let mut running = agent(&session).await;
    offer(&mut running, 1, "image/png").await;
    let mut probe = Probe::start(&session, backend).await;
    let image = picture(7, 5, 123);
    assert_eq!(
        fetch(
            &mut probe,
            &mut running,
            "read-image",
            1,
            "image/png",
            &image.png
        )
        .await,
        image.expected
    );
    stop(&mut running).await;
    // A client connected to the old server must not return its cached pixels.
    probe.unavailable("read-image").await;
    probe.stop().await;
    let mut running = agent(&session).await;
    offer(&mut running, 2, "").await;
    // X11 connections are tied to a server lifetime; refresh env and create a
    // new Clipboard after restart, rather than promising automatic reconnection.
    let mut probe = Probe::start(&session, backend).await;
    probe.unavailable("read-image").await;
    offer(&mut running, 3, "image/png").await;
    let new = picture(11, 13, 124);
    assert_eq!(
        fetch(
            &mut probe,
            &mut running,
            "read-image",
            3,
            "image/png",
            &new.png
        )
        .await,
        new.expected
    );
    probe.stop().await;
    stop(&mut running).await;
}

#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn x11_arboard_sequence_text_images_empty_and_compressed_chunks() {
    sequence(Backend::X11).await;
}
#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn wayland_arboard_sequence_text_images_empty_and_compressed_chunks() {
    sequence(Backend::Wayland).await;
}
#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn x11_arboard_rejects_superseded_image() {
    superseded_image(Backend::X11).await;
}
#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn wayland_arboard_rejects_superseded_image() {
    superseded_image(Backend::Wayland).await;
}
#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn x11_arboard_restart_does_not_return_stale_pixels() {
    restart(Backend::X11).await;
}
#[tokio::test]
#[ignore = "Requires npm run test:remote"]
async fn wayland_arboard_restart_does_not_return_stale_pixels() {
    restart(Backend::Wayland).await;
}
