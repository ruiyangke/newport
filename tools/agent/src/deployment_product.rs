//! Product replacement checks run while the installation lock is held. The
//! desktop's earlier observation is fenced by the exact current build hash.
use crate::product::{Identity, Product, Variant};
use std::{
    io::{self, Read},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

#[derive(Default)]
pub(crate) struct Options {
    pub integrations: bool,
    pub expected: Option<String>,
    pub previous: Option<Variant>,
    pub downgrade: bool,
    /// Keep the entry point while a process runs its build.
    pub unless_running: bool,
}
impl Options {
    pub fn parse(args: &[String]) -> io::Result<Self> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--integrations" if !options.integrations => options.integrations = true,
                "--allow-downgrade" if !options.downgrade => options.downgrade = true,
                "--unless-running" if !options.unless_running => options.unless_running = true,
                "--expect-current" if options.expected.is_none() => {
                    let hash = args
                        .next()
                        .ok_or_else(|| io::Error::other("Missing expected build"))?;
                    if hash != "missing" {
                        crate::deployment::valid_hash(hash)?;
                    }
                    options.expected = Some(hash.clone());
                }
                "--expect-variant" if options.previous.is_none() => {
                    options.previous = Some(match args.next().map(String::as_str) {
                        Some("core") => Variant::Core,
                        Some("full") => Variant::Full,
                        _ => return Err(io::Error::other("Invalid expected product variant")),
                    });
                }
                _ => return Err(io::Error::other("Invalid deployment install option")),
            }
        }
        if options.previous.is_some() && options.expected.as_deref().is_none_or(|v| v == "missing")
        {
            return Err(io::Error::other(
                "Expected product requires an exact previous build",
            ));
        }
        Ok(options)
    }
}

// Older agents do not expose product --json. Only that exact legacy response
// allows the desktop's hash-bound, separately verified variant to be used.
enum Probe {
    Known(Identity),
    Legacy,
}
fn probe(path: &Path) -> io::Result<Probe> {
    let mut child = Command::new(path)
        .args(["product", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (send, receive) = mpsc::sync_channel(2);
    for (is_output, stream) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let send = send.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream
                .take(8193)
                .read_to_end(&mut bytes)
                .map(|_| (is_output, bytes));
            let _ = send.send(result);
        });
    }
    drop(send);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let result = (|| {
        let mut output = Vec::new();
        let mut error = Vec::new();
        for _ in 0..2 {
            let (is_output, bytes) = receive
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .map_err(|_| io::Error::other("Installed product inspection timed out"))??;
            if bytes.len() > 8192 {
                return Err(io::Error::other("Installed product response exceeds limit"));
            }
            if is_output {
                output = bytes;
            } else {
                error = bytes;
            }
        }
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::other("Installed product inspection timed out"));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !status.success() {
            if status.code() == Some(1)
                && output.is_empty()
                && error == b"newport-agent: Unknown command; use --help\n"
            {
                return Ok(Probe::Legacy);
            }
            return Err(io::Error::other(
                "Installed agent could not report its product identity",
            ));
        }
        let identity: Identity = serde_json::from_slice(&output)?;
        identity.validate().map_err(io::Error::other)?;
        Ok(Probe::Known(identity))
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

pub(crate) fn check(
    path: &Path,
    current: &str,
    product: &Product,
    options: &Options,
) -> io::Result<()> {
    if options
        .expected
        .as_ref()
        .is_some_and(|expected| expected != current)
    {
        return Err(io::Error::other(
            "Installed agent changed during deployment; inspect it again before retrying",
        ));
    }
    if current == "missing" {
        return Ok(());
    }
    let installed = match probe(path)? {
        Probe::Known(identity) => {
            if identity.build_sha256 != current {
                return Err(io::Error::other(
                    "Installed product does not match its executable checksum",
                ));
            }
            identity.product.variant
        }
        Probe::Legacy => options.previous.ok_or_else(|| {
            io::Error::other("Legacy agent requires a product check by Newport before replacement")
        })?,
    };
    if options
        .previous
        .is_some_and(|expected| expected != installed)
    {
        return Err(io::Error::other(
            "Installed product differs from the expected variant",
        ));
    }
    if !product
        .variant
        .may_replace(installed, false, options.downgrade)
    {
        return Err(io::Error::other(
            "Replacing the full agent with core requires --allow-downgrade",
        ));
    }
    Ok(())
}
