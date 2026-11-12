use super::{Error, POLL, check};
use newport_protocol::{
    encoding::protobuf::{FrameDecoder, FrameError},
    monitor::{
        self,
        ports::discovery::{
            Snapshot,
            helper::{Reply, Request, wire},
        },
    },
};
use std::{
    fs::File,
    io::{self, Read, Write},
    process::{Child, ExitStatus},
    time::Instant,
};

const STDERR_LIMIT: usize = 4096;

pub(super) struct Pipes<'a> {
    pub input: &'a File,
    pub output: &'a File,
    pub errors: &'a File,
}

struct Capture {
    decoder: FrameDecoder,
    reply: Option<Reply>,
    diagnostic: Vec<u8>,
    stdout_eof: bool,
    stderr_eof: bool,
}

pub(super) fn run(
    child: &mut Child,
    reaped: &mut bool,
    mut pipes: Pipes<'_>,
    request: &Request,
    mut pending: &[u8],
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Snapshot, Error> {
    let mut capture = Capture {
        decoder: FrameDecoder::new(monitor::SERVER_LIMITS.message),
        reply: None,
        diagnostic: Vec::new(),
        stdout_eof: false,
        stderr_eof: false,
    };
    loop {
        check(deadline, cancelled)?;
        let mut progress = send(&mut pipes.input, &mut pending)?;
        progress |= capture.output(&mut pipes.output, request)?;
        progress |= capture.errors(&mut pipes.errors)?;
        if capture.stdout_eof && capture.stderr_eof {
            // EOF must precede reaping, keeping the process group reserved during capture.
            if let Some(status) = child.try_wait()? {
                *reaped = true;
                return capture.finish(status);
            }
        }
        if !progress {
            std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

fn send(input: &mut &File, pending: &mut &[u8]) -> Result<bool, Error> {
    if pending.is_empty() {
        return Ok(false);
    }
    match input.write(pending) {
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "Discovery control pipe stopped accepting input",
        )
        .into()),
        Ok(count) => {
            *pending = &pending[count..];
            Ok(true)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        }
        // A denied sudo command may close stdin before reporting its exit and diagnostic.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            *pending = &[];
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

impl Capture {
    fn output(&mut self, output: &mut &File, request: &Request) -> Result<bool, Error> {
        if self.stdout_eof {
            return Ok(false);
        }
        let mut buffer = [0; 8192];
        match output.read(&mut buffer) {
            Ok(0) => {
                self.stdout_eof = true;
                Ok(false)
            }
            Ok(count) => {
                if self.reply.is_some() {
                    return Err(Error::Invalid("Discovery helper sent trailing output"));
                }
                let mut chunk = &buffer[..count];
                if let Some(frame) = self
                    .decoder
                    .consume(&mut chunk)
                    .map_err(wire::Error::from)?
                {
                    self.reply = Some(wire::decode_reply(frame.body(), request)?);
                }
                if !chunk.is_empty() {
                    return Err(Error::Invalid("Discovery helper sent multiple replies"));
                }
                Ok(true)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn errors(&mut self, errors: &mut &File) -> Result<bool, Error> {
        if self.stderr_eof {
            return Ok(false);
        }
        let mut buffer = [0; 8192];
        match errors.read(&mut buffer) {
            Ok(0) => {
                self.stderr_eof = true;
                Ok(false)
            }
            Ok(count) if count <= STDERR_LIMIT.saturating_sub(self.diagnostic.len()) => {
                self.diagnostic.extend_from_slice(&buffer[..count]);
                Ok(true)
            }
            Ok(_) => Err(Error::Invalid("Discovery helper diagnostic limit reached")),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn finish(mut self, status: ExitStatus) -> Result<Snapshot, Error> {
        if !status.success() {
            let mut diagnostic = String::from_utf8_lossy(&self.diagnostic).into_owned();
            super::super::super::super::truncate(
                &mut diagnostic,
                monitor::ports::discovery::MAX_DIAGNOSTIC,
            );
            return Err(Error::Exited { status, diagnostic });
        }
        self.decoder.finish().map_err(wire::Error::from)?;
        let reply = self
            .reply
            .ok_or_else(|| Error::Wire(wire::Error::from(FrameError::Truncated)))?;
        reply
            .result
            .map(|snapshot| *snapshot)
            .map_err(Error::Remote)
    }
}
