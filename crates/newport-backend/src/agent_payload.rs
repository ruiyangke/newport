//! Compressed, file-backed Needle payloads. Only identity and size are embedded.
use flate2::read::MultiGzDecoder;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use tokio::{sync::mpsc, task::JoinHandle};

const CHUNK: usize = 32 * 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy)]
pub(crate) struct Payload {
    pub file: &'static str,
    pub size: usize,
    pub sha256: &'static str,
}
include!(concat!(env!("OUT_DIR"), "/needle-payloads.rs"));

pub(crate) fn payload(arch: &str) -> Option<Payload> {
    match arch {
        "x86_64" => Some(X86_64),
        "aarch64" | "arm64" => Some(AARCH64),
        _ => None,
    }
}

fn resource_directory(exe: &Path) -> Result<PathBuf, String> {
    let parent = exe.parent().ok_or("Cannot locate Needle resources")?;
    if parent.file_name().is_some_and(|name| name == "MacOS")
        && parent
            .parent()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "Contents"))
    {
        return Ok(parent.parent().unwrap().join("Resources/agents"));
    }
    // Cargo test and example executables share the profile's staged resources.
    if parent
        .file_name()
        .is_some_and(|name| name == "deps" || name == "examples")
    {
        return Ok(parent
            .parent()
            .ok_or("Cannot locate Cargo resources")?
            .join("agents"));
    }
    Ok(parent.join("agents"))
}

impl Payload {
    fn open(self) -> Result<File, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let path = resource_directory(&exe)?.join(self.file);
        File::open(&path).map_err(|e| {
            format!(
                "Cannot open bundled Needle resource {}: {e}",
                path.display()
            )
        })
    }

    /// Called on a blocking worker. At most two queued 32 KiB chunks are kept.
    fn decode(
        self,
        input: impl Read,
        mut consume: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut decoder = MultiGzDecoder::new(input);
        let mut buffer = [0u8; CHUNK];
        let mut size = 0usize;
        let mut hash = Sha256::new();
        loop {
            let count = decoder
                .read(&mut buffer)
                .map_err(|e| format!("Cannot decompress bundled Needle: {e}"))?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count)
                .ok_or("Needle payload is too large")?;
            if size > self.size {
                return Err("Needle payload exceeds its expected size".into());
            }
            hash.update(&buffer[..count]);
            consume(&buffer[..count])?;
        }
        if size != self.size || hex(&hash.finalize()) != self.sha256 {
            return Err("Bundled Needle checksum or size does not match this application".into());
        }
        Ok(())
    }

    pub fn stream(self) -> Result<Upload, String> {
        let file = self.open()?;
        let (sender, receiver) = mpsc::channel(2);
        let worker = tokio::task::spawn_blocking(move || {
            self.decode(file, |chunk| {
                sender
                    .blocking_send(chunk.to_vec())
                    .map_err(|_| "Needle upload cancelled".into())
            })
        });
        Ok(Upload {
            receiver,
            worker: Some(worker),
        })
    }
}

pub(crate) struct Upload {
    receiver: mpsc::Receiver<Vec<u8>>,
    worker: Option<JoinHandle<Result<(), String>>>,
}
impl Upload {
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>, String> {
        if let Some(chunk) = self.receiver.recv().await {
            return Ok(Some(chunk));
        }
        if let Some(worker) = self.worker.take() {
            worker
                .await
                .map_err(|e| format!("Needle resource reader failed: {e}"))??;
        }
        // SSH EOF is sent only after decoding, size and checksum verification succeed.
        Ok(None)
    }
}

pub(crate) fn check_resources() -> Result<(), String> {
    for payload in [X86_64, AARCH64] {
        payload.decode(payload.open()?, |_| Ok(()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn compressed(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn resource_locations_follow_the_executable_without_source_fallback() {
        for (exe, expected) in [
            (
                "/app/Newport.app/Contents/MacOS/newport",
                "/app/Newport.app/Contents/Resources/agents",
            ),
            ("/app/Newport/newport.exe", "/app/Newport/agents"),
            ("/build/debug/deps/test", "/build/debug/agents"),
            ("/build/release/examples/probe", "/build/release/agents"),
        ] {
            assert_eq!(
                resource_directory(Path::new(exe)).unwrap(),
                Path::new(expected)
            );
        }
    }

    #[test]
    fn corrupt_truncated_wrong_and_oversized_payloads_fail() {
        let data = b"verified payload";
        let hash = hex(&Sha256::digest(data));
        let descriptor = Payload {
            file: "fixture",
            size: data.len(),
            sha256: Box::leak(hash.into_boxed_str()),
        };
        let gzip = compressed(data);
        let mut decoded = Vec::new();
        descriptor
            .decode(gzip.as_slice(), |bytes| {
                decoded.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap();
        assert_eq!(decoded, data);
        assert!(
            descriptor
                .decode(&gzip[..gzip.len() - 1], |_| Ok(()))
                .is_err()
        );
        assert!(
            descriptor
                .decode(compressed(b"wrong contents!").as_slice(), |_| Ok(()))
                .is_err()
        );
        let mut trailing = gzip.clone();
        trailing.extend_from_slice(b"invalid trailer");
        assert!(descriptor.decode(trailing.as_slice(), |_| Ok(())).is_err());
        let mut delivered = 0;
        assert!(
            descriptor
                .decode(compressed(&vec![0; CHUNK * 10]).as_slice(), |bytes| {
                    delivered += bytes.len();
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(
            delivered, 0,
            "reject excessive decoded bytes before handing them to SSH"
        );
    }

    #[tokio::test]
    async fn cancellation_releases_the_decoder_and_corruption_never_reports_success() {
        let mut upload = X86_64.stream().unwrap();
        let worker = upload.worker.take().unwrap();
        drop(upload);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), worker)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        let mut upload = Payload {
            sha256: "wrong hash",
            ..X86_64
        }
        .stream()
        .unwrap();
        loop {
            match upload.next().await {
                Ok(Some(_)) => {}
                Ok(None) => panic!("corrupt resource must not signal successful EOF"),
                Err(error) => {
                    assert!(error.contains("checksum"));
                    break;
                }
            }
        }
    }

    #[tokio::test]
    async fn staged_payloads_stream_with_bounded_chunks_and_verified_identity() {
        for payload in [X86_64, AARCH64] {
            let mut upload = payload.stream().unwrap();
            let mut hash = Sha256::new();
            let mut size = 0;
            while let Some(chunk) = upload.next().await.unwrap() {
                assert!(chunk.len() <= CHUNK);
                size += chunk.len();
                hash.update(chunk);
            }
            assert_eq!(size, payload.size);
            assert_eq!(hex(&hash.finalize()), payload.sha256);
        }
    }
}
