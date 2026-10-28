use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

const INSTALLER: &str = include_str!("agent-install.sh");
const AGENT: &[u8] = br##"#!/bin/sh
set -eu
case "$1" in
    --version)
        printf 'version\n' >> "$HOME/executed"
        printf 'newport-agent/1\n'
        ;;
    deployment)
        [ "$#" -eq 3 ] && [ "$2" = prepare ]
        printf 'prepare:%s\n' "$3" >> "$HOME/executed"
        cp "$0" "$HOME/candidate"
        printf '%s\n' "$0" > "$HOME/prepared-path"
        ;;
    *) exit 1 ;;
esac
"##;

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn executable(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(name))
        .find(|path| {
            fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

struct Fixture {
    _directory: tempfile::TempDir,
    home: PathBuf,
    tools: PathBuf,
}

impl Fixture {
    fn new(with_gzip: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let tools = directory.path().join("tools");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir(&tools).unwrap();
        for name in ["mkdir", "mktemp", "rm", "cat", "chmod", "cp"] {
            symlink(
                executable(name).unwrap_or_else(|| panic!("{name} is required")),
                tools.join(name),
            )
            .unwrap();
        }
        if let Some(checksum) = executable("sha256sum") {
            symlink(checksum, tools.join("sha256sum")).unwrap();
        } else {
            let checksum = executable("shasum").expect("sha256sum or shasum is required");
            let quoted = checksum.to_str().unwrap().replace('\'', "'\"'\"'");
            let adapter = tools.join("sha256sum");
            fs::write(
                &adapter,
                format!("#!/bin/sh\nexec '{quoted}' -a 256 \"$@\"\n"),
            )
            .unwrap();
            fs::set_permissions(adapter, fs::Permissions::from_mode(0o700)).unwrap();
        }
        if with_gzip {
            symlink(
                executable("gzip").expect("gzip is required"),
                tools.join("gzip"),
            )
            .unwrap();
        }
        fs::write(home.join("previous-agent"), b"previous working agent\n").unwrap();
        fs::set_permissions(
            home.join("previous-agent"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        symlink(
            "../../previous-agent",
            home.join(".local/bin/newport-agent"),
        )
        .unwrap();
        Self {
            _directory: directory,
            home,
            tools,
        }
    }

    fn shell(&self, script: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .args(["-c", script])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.tools)
            .env("LC_ALL", "C")
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    async fn run(&self, payload: &[u8], expected_hash: &str, encoding: Option<&str>) -> Output {
        let input = self.home.join("upload");
        fs::write(&input, payload).unwrap();
        let mut command = self.shell(INSTALLER);
        command
            .args(["newport-install", expected_hash])
            .stdin(Stdio::from(fs::File::open(input).unwrap()));
        if let Some(encoding) = encoding {
            command.arg(encoding);
        }
        tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .expect("installer exceeded its deadline")
            .unwrap()
    }

    fn assert_preserved_and_clean(&self) {
        let bin = self.home.join(".local/bin");
        assert_eq!(
            fs::read_link(bin.join("newport-agent")).unwrap(),
            Path::new("../../previous-agent")
        );
        assert_eq!(
            fs::read(bin.join("newport-agent")).unwrap(),
            b"previous working agent\n"
        );
        assert_eq!(
            fs::metadata(bin.join("newport-agent"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let entries: Vec<_> = fs::read_dir(bin)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["newport-agent"]);
    }

    fn assert_rejected(&self, output: Output, diagnostic: &str) {
        assert!(
            !output.status.success(),
            "installer accepted a rejected upload"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(diagnostic), "{stderr}");
        assert!(output.stdout.is_empty());
        assert!(
            !self.home.join("executed").exists(),
            "unverified upload was executed"
        );
        assert!(!self.home.join("candidate").exists());
        self.assert_preserved_and_clean();
    }
}

#[tokio::test]
async fn gzip_and_raw_prepare_identical_candidates_without_activation() {
    let digest = hash(AGENT);
    for (encoding, payload) in [("raw", AGENT.to_vec()), ("gzip", gzip(AGENT))] {
        let fixture = Fixture::new(encoding == "gzip");
        let output = fixture.run(&payload, &digest, Some(encoding)).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(fixture.home.join("candidate")).unwrap(), AGENT);
        assert_eq!(
            fs::metadata(fixture.home.join("candidate"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::read_to_string(fixture.home.join("executed")).unwrap(),
            format!("version\nprepare:{digest}\n")
        );
        let prepared = fs::read_to_string(fixture.home.join("prepared-path")).unwrap();
        let prepared = Path::new(prepared.trim());
        assert_eq!(prepared.parent().unwrap(), fixture.home.join(".local/bin"));
        assert!(
            prepared
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".newport-agent.")
        );
        assert!(!prepared.exists());
        fixture.assert_preserved_and_clean();
    }
}

#[tokio::test]
async fn malformed_truncated_and_corrupt_gzip_never_execute() {
    let compressed = gzip(AGENT);
    let mut corrupt = compressed.clone();
    let checksum = corrupt.len() - 8;
    corrupt[checksum] ^= 1;
    for payload in [
        b"not a gzip stream".to_vec(),
        compressed[..compressed.len() - 1].to_vec(),
        corrupt,
    ] {
        let fixture = Fixture::new(true);
        let output = fixture.run(&payload, &hash(AGENT), Some("gzip")).await;
        fixture.assert_rejected(output, "Cannot decompress agent upload.");
    }
}

#[tokio::test]
async fn wrong_checksum_never_executes_in_either_encoding() {
    for (encoding, payload) in [("raw", AGENT.to_vec()), ("gzip", gzip(AGENT))] {
        let fixture = Fixture::new(encoding == "gzip");
        let output = fixture.run(&payload, &"0".repeat(64), Some(encoding)).await;
        fixture.assert_rejected(output, "Agent upload checksum mismatch.");
    }
}

#[tokio::test]
async fn unsupported_or_missing_encoding_is_rejected() {
    for encoding in [Some("brotli"), None] {
        let fixture = Fixture::new(true);
        let output = fixture.run(AGENT, &hash(AGENT), encoding).await;
        fixture.assert_rejected(output, "Unsupported agent upload encoding.");
    }
}

#[tokio::test]
async fn gzip_requires_the_decoder_and_does_not_fall_back_to_raw() {
    let fixture = Fixture::new(false);
    let output = fixture.run(AGENT, &hash(AGENT), Some("gzip")).await;
    fixture.assert_rejected(output, "gzip is required for this agent upload.");
}

#[tokio::test]
async fn capability_probe_selects_raw_only_when_gzip_is_unavailable() {
    use super::TransferEncoding;

    for (with_gzip, expected) in [
        (true, TransferEncoding::Gzip),
        (false, TransferEncoding::Raw),
    ] {
        let fixture = Fixture::new(with_gzip);
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            fixture.shell(TransferEncoding::PROBE).output(),
        )
        .await
        .expect("capability probe exceeded its deadline")
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let encoding =
            TransferEncoding::parse(String::from_utf8(output.stdout).unwrap().trim()).unwrap();
        assert_eq!(encoding, expected);
        assert!(output.stderr.is_empty());
        assert!(!fixture.home.join("executed").exists());
        fixture.assert_preserved_and_clean();
    }
}
