//! Released OpenCode adapters and their real dependencies, through Linux SSH.
use super::*;
use image::ImageEncoder;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

type Stream = russh::ChannelStream<russh::client::Msg>;
struct Probe(BufReader<Stream>);

impl Probe {
    async fn start(session: &ExecSession, version: &str, backend: &str) -> Self {
        let isolate = match (version, backend) {
            // Restrict PATH so v1 must use its xclip fallback, then xsel for text.
            ("v1", "x11") => format!(
                "probe_bin=$(mktemp -d); ln -s ~/.local/bin/xclip \"$probe_bin/xclip\"; \
                 ln -s ~/.local/bin/xsel \"$probe_bin/xsel\"; export PATH=\"$probe_bin\"; \
                 /usr/local/bin/bun /opt/opencode-probe/main.ts {version}; \
                 result=$?; /bin/rm -rf \"$probe_bin\"; exit $result"
            ),
            ("v1", "wayland") => {
                format!("exec /usr/local/bin/bun /opt/opencode-probe/main.ts {version}")
            }
            ("v2", "x11") => format!(
                "unset WAYLAND_DISPLAY WAYLAND_SOCKET; \
                 test -n \"$DISPLAY\" && test -f \"$XAUTHORITY\" || exit 1; \
                 exec /usr/local/bin/bun /opt/opencode-probe/main.ts {version}"
            ),
            ("v2", "wayland") => format!(
                "unset DISPLAY XAUTHORITY WAYLAND_SOCKET; test -S \"$WAYLAND_DISPLAY\" || exit 1; \
                 exec /usr/local/bin/bun /opt/opencode-probe/main.ts {version}"
            ),
            _ => panic!("unknown OpenCode probe mode"),
        };
        let stream = session
            .stream(&format!(
                "eval \"$(~/.local/bin/newport-agent env)\"; {isolate}"
            ))
            .await
            .unwrap();
        let mut probe = Self(BufReader::new(stream));
        let ready = probe.next().await;
        assert_eq!(ready["ready"], true, "{version}/{backend}: {ready}");
        assert_eq!(ready["version"], version);
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
            .expect("OpenCode probe timed out")
            .unwrap();
        assert!(size > 0, "OpenCode probe exited before returning a result");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("invalid probe result {line:?}: {e}"))
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

async fn offer(agent: &mut Stream, revision: i64, formats: &[&str]) {
    frame(
        agent,
        b'M',
        &rmp_serde::to_vec(&(revision, formats)).unwrap(),
    )
    .await;
    // Read immediately after the acknowledgment. A delay here would conceal
    // stale Wayland offers that OpenTUI's per-read sync barrier cannot repair.
    receive(agent, b'A').await;
}

async fn fetch(
    probe: &mut Probe,
    agent: &mut Stream,
    revision: i64,
    format: &str,
    data: &[u8],
) -> Value {
    probe.command("read").await;
    let result = probe.next();
    tokio::pin!(result);
    let request = tokio::select! {
        early = &mut result => panic!("OpenCode returned before fetching revision {revision}: {early}"),
        request = receive(agent, b'C') => request,
    };
    let (id, actual_revision, actual_format): (u64, i64, String) =
        rmp_serde::from_slice(&request).unwrap();
    assert_eq!(
        (actual_revision, actual_format.as_str()),
        (revision, format)
    );
    let chunks: Vec<_> = data.chunks(48 * 1024).collect();
    for (index, bytes) in chunks.iter().enumerate() {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        frame(
            agent,
            b'D',
            &rmp_serde::to_vec(&(
                id,
                revision,
                2u8,
                index + 1 == chunks.len(),
                serde_bytes::Bytes::new(&encoder.finish().unwrap()),
            ))
            .unwrap(),
        )
        .await;
    }
    result.await
}

fn picture() -> Vec<u8> {
    let mut seed = 123456789u32;
    let pixels: Vec<u8> = (0..384 * 256 * 4)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, 384, 256, image::ExtendedColorType::Rgba8)
        .unwrap();
    assert!(png.len() > 256 * 1024);
    png
}

async fn sequence(version: &str, backend: &str) {
    let session = ExecSession::connect(&server()).await.unwrap();
    crate::agent::install(&session).await.unwrap();
    let mut agent = session
        .stream(&format!(
            "exec ~/.local/bin/newport-agent serve {} --clipboard",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut agent, b'R').await, b"newport-agent/1");
    offer(&mut agent, 1, &["text/plain"]).await;
    let mut probe = Probe::start(&session, version, backend).await;
    let text = "OpenCode 世界 — 🦀\nsecond line\n";
    assert_eq!(
        fetch(&mut probe, &mut agent, 1, "text/plain", text.as_bytes()).await,
        json!({"mime":"text/plain", "text":text}),
        "{version}/{backend}"
    );
    let png = picture();
    // Both representations are available: OpenCode must prefer the screenshot.
    offer(&mut agent, 2, &["text/plain", "image/png"]).await;
    let hash: String = Sha256::digest(&png)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let expected = json!({"mime":"image/png", "sha256":hash});
    assert_eq!(
        fetch(&mut probe, &mut agent, 2, "image/png", &png).await,
        expected
    );
    probe.command("read").await;
    assert_eq!(probe.next().await, expected);
    // A cached read must not emit another fetch to the desktop.
    frame(&mut agent, b'H', &[]).await;
    receive(&mut agent, b'A').await;
    offer(&mut agent, 3, &[]).await;
    probe.command("read").await;
    assert_eq!(probe.next().await, json!({"empty":true}));
    offer(&mut agent, 4, &["text/plain"]).await;
    assert_eq!(
        fetch(&mut probe, &mut agent, 4, "text/plain", b"new text").await,
        json!({"mime":"text/plain", "text":"new text"})
    );
    probe.stop().await;
    frame(&mut agent, b'Q', &[]).await;
    timeout(Duration::from_secs(10), agent.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn v1_wl_paste_images_and_clipboardy_text() {
    sequence("v1", "wayland").await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn v1_xclip_fallback_images_and_clipboardy_text() {
    sequence("v1", "x11").await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn v2_native_x11_text_and_images() {
    sequence("v2", "x11").await;
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn v2_native_wayland_text_and_images() {
    sequence("v2", "wayland").await;
}
