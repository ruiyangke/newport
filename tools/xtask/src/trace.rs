//! Captured channels printed as Protocol Buffers JSON.
use anyhow::{Context, Result};
use std::{
    borrow::Cow,
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::Path,
};

/// The protocol crates whose descriptor sets `trace` knows by name.
const PROTOCOLS: &[(&str, &[u8])] = &[
    (
        "newport-protocol",
        newport_protocol::encoding::protobuf::DESCRIPTORS,
    ),
    (
        "remote-apps",
        newport_protocol::encoding::protobuf::DESCRIPTORS,
    ),
];

pub fn run(protocol: &str, message: &str, capture: Option<&Path>) -> Result<()> {
    let descriptors = descriptors(protocol)?;
    let capture: Box<dyn Read> = match capture {
        Some(path) => {
            Box::new(File::open(path).with_context(|| format!("opening {}", path.display()))?)
        }
        None => Box::new(io::stdin().lock()),
    };
    let mut output = BufWriter::new(io::stdout().lock());
    newport_protocol::encoding::protobuf::trace::trace(
        &descriptors,
        message,
        &mut BufReader::new(capture),
        &mut output,
    )
    .map_err(anyhow::Error::from_boxed)?;
    output.flush()?;
    Ok(())
}

/// A protocol's descriptor set by name, or the one at the path `protocol`.
fn descriptors(protocol: &str) -> Result<Cow<'static, [u8]>> {
    if let Some(&(_, descriptors)) = PROTOCOLS.iter().find(|(name, _)| *name == protocol) {
        return Ok(Cow::Borrowed(descriptors));
    }
    let names = PROTOCOLS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    fs::read(protocol).map(Cow::Owned).with_context(|| {
        format!("{protocol} is not a protocol ({names}) or a readable descriptor set")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_protocol_crate_is_found_by_name() {
        assert_eq!(
            &*descriptors("newport-protocol").unwrap(),
            newport_protocol::encoding::protobuf::DESCRIPTORS
        );
    }

    #[test]
    fn remote_apps_is_available_without_the_private_checkout() {
        let descriptors = descriptors("remote-apps").unwrap();
        for message in [
            "newport.remote.apps.ServerFrame",
            "newport.remote.apps.ClientFrame",
            "newport.remote.display.Envelope",
            "newport.remote.display.EnvelopeReply",
            "newport.wire.Secret",
        ] {
            newport_protocol::encoding::protobuf::trace::trace(
                &descriptors,
                message,
                &mut io::empty(),
                &mut io::sink(),
            )
            .unwrap_or_else(|error| panic!("{message}: {error}"));
        }
    }

    #[test]
    fn anything_else_is_read_as_a_path() {
        let error = descriptors("/nonexistent/descriptors.bin").unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("newport-protocol"), "{error}");
        assert!(error.contains("remote-apps"), "{error}");
    }
}
