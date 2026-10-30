//! An acknowledgment from the remote shell wrapper, after GUI preflight and
//! before the user's login shell. It travels on the existing PTY channel and
//! reports whether the shell started with the shared display.
const LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ack {
    Ready,
    Unavailable,
}

pub(crate) struct Readiness {
    nonce: uuid::Uuid,
    ready: Vec<u8>,
    unavailable: Vec<u8>,
    checked: usize,
}
impl Readiness {
    pub fn new() -> Self {
        let nonce = uuid::Uuid::new_v4();
        let marker = |state: &str| format!("\u{1e}NEWPORT_GUI_{state}:{nonce}\u{1f}").into_bytes();
        Self {
            nonce,
            ready: marker("READY"),
            unavailable: marker("UNAVAILABLE"),
            checked: 0,
        }
    }
    pub fn command(&self, ack: Ack) -> String {
        let state = match ack {
            Ack::Ready => "READY",
            Ack::Unavailable => "UNAVAILABLE",
        };
        format!("printf '\\036NEWPORT_GUI_{state}:{}\\037'; ", self.nonce)
    }
    fn marker(&self, ack: Ack) -> &[u8] {
        match ack {
            Ack::Ready => &self.ready,
            Ack::Unavailable => &self.unavailable,
        }
    }
    /// Strip only this attempt's marker, preserving banners and login output.
    /// Scan each newly appended byte once, including a fragmented marker.
    pub fn strip(&mut self, output: &mut Vec<u8>) -> Result<Option<Ack>, String> {
        let longest = self.ready.len().max(self.unavailable.len());
        let start = self.checked.saturating_sub(longest - 1);
        for ack in [Ack::Ready, Ack::Unavailable] {
            let marker = self.marker(ack);
            if let Some(position) = output[start..]
                .windows(marker.len())
                .position(|bytes| bytes == marker)
            {
                let position = start + position;
                if position > LIMIT {
                    return Err("Graphical terminal startup output exceeded 64 KiB".into());
                }
                output.drain(position..position + marker.len());
                return Ok(Some(ack));
            }
        }
        if output.len() > LIMIT + longest {
            return Err("Graphical terminal startup output exceeded 64 KiB".into());
        }
        self.checked = output.len();
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fragment_boundary_preserves_output_and_removes_only_this_attempt() {
        let other = Readiness::new();
        for ack in [Ack::Ready, Ack::Unavailable] {
            for split in 0..=other.marker(ack).len() {
                let mut ready = Readiness::new();
                let marker = ready.marker(ack).to_vec();
                let mut output = b"login banner\r\n".to_vec();
                output.extend_from_slice(other.marker(ack));
                let expected = output.clone();
                output.extend_from_slice(&marker[..split]);
                let complete = ready.strip(&mut output).unwrap();
                assert_eq!(complete, (split == marker.len()).then_some(ack));
                if complete.is_none() {
                    output.extend_from_slice(&marker[split..]);
                    output.extend_from_slice(b"prompt> ");
                    assert_eq!(ready.strip(&mut output).unwrap(), Some(ack));
                } else {
                    output.extend_from_slice(b"prompt> ");
                }
                assert_eq!(output, [expected, b"prompt> ".to_vec()].concat());
            }
        }
    }

    #[test]
    fn output_is_bounded_and_a_boundary_marker_may_arrive_one_byte_at_a_time() {
        for ack in [Ack::Ready, Ack::Unavailable] {
            let mut ready = Readiness::new();
            let mut output = vec![b'x'; LIMIT];
            assert_eq!(ready.strip(&mut output).unwrap(), None);
            let marker = ready.marker(ack).to_vec();
            for (i, byte) in marker.iter().enumerate() {
                output.push(*byte);
                assert_eq!(
                    ready.strip(&mut output).unwrap(),
                    (i + 1 == marker.len()).then_some(ack)
                );
            }
            assert_eq!(output.len(), LIMIT);
            let mut ready = Readiness::new();
            let longest = ready.ready.len().max(ready.unavailable.len());
            let mut output = vec![b'x'; LIMIT + longest + 1];
            assert!(ready.strip(&mut output).is_err());
            let mut output = vec![b'x'; LIMIT + 1];
            output.extend_from_slice(ready.marker(ack));
            assert!(ready.strip(&mut output).is_err());
        }
    }

    #[test]
    #[cfg(unix)]
    fn shell_acknowledgment_emits_exact_bytes_without_a_newline() {
        let ready = Readiness::new();
        for ack in [Ack::Ready, Ack::Unavailable] {
            let output = std::process::Command::new("sh")
                .args(["-c", &ready.command(ack)])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, ready.marker(ack));
        }
    }
}
