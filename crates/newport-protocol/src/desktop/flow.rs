use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpaqueKind {
    Command,
    Lsp,
}
#[derive(Debug)]
pub struct OpaqueFlow {
    kind: OpaqueKind,
    input_complete: bool,
    completed: bool,
    ended: bool,
    poisoned: bool,
}
impl OpaqueFlow {
    pub fn new(kind: OpaqueKind) -> Self {
        Self {
            kind,
            input_complete: false,
            completed: false,
            ended: false,
            poisoned: false,
        }
    }
    pub fn client(&mut self, value: &stream::OpaqueClient) -> Result<(), Invalid> {
        let result = self.client_inner(value);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn client_inner(&mut self, value: &stream::OpaqueClient) -> Result<(), Invalid> {
        if self.poisoned || self.input_complete || self.completed || self.ended {
            return Err(Invalid("opaque input phase"));
        }
        value.validate()?;
        match value {
            stream::OpaqueClient::Data(_) => {}
            stream::OpaqueClient::InputComplete(_) => self.input_complete = true,
        }
        Ok(())
    }
    pub fn server(&mut self, value: &stream::OpaqueServer) -> Result<(), Invalid> {
        let result = self.server_inner(value);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn server_inner(&mut self, value: &stream::OpaqueServer) -> Result<(), Invalid> {
        if self.poisoned || self.completed || self.ended {
            return Err(Invalid("opaque output phase"));
        }
        value.validate()?;
        if let stream::OpaqueServer::Completion(completion) = value {
            if !matches!(
                (self.kind, completion.as_ref()),
                (OpaqueKind::Command, stream::Completion::Command(_))
                    | (OpaqueKind::Lsp, stream::Completion::Lsp(_))
            ) {
                return Err(Invalid("opaque completion kind"));
            }
            self.completed = true;
        }
        Ok(())
    }
    pub fn eof(&mut self) -> Result<(), Invalid> {
        if self.poisoned || !self.completed || self.ended {
            self.poisoned = true;
            return Err(Invalid("incomplete opaque stream"));
        }
        self.ended = true;
        Ok(())
    }
}

/// Directional accounting shared by terminal adapters. Stale input is acknowledged and discarded.
#[derive(Debug, Default)]
pub struct TerminalFlow {
    output: u64,
    input: u64,
    epoch: u64,
    poisoned: bool,
}
impl TerminalFlow {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn output_available(&self) -> u64 {
        OUTPUT_CREDIT - self.output
    }
    pub fn input_available(&self) -> u64 {
        INPUT_WINDOW - self.input
    }
    pub fn output(&mut self, bytes: usize) -> Result<(), Invalid> {
        self.change(bytes, true)
    }
    pub fn acknowledge(&mut self, bytes: u64) -> Result<(), Invalid> {
        self.release(bytes, true)
    }
    /// `false` means input belongs to a departed shell; still account and report Taken for it.
    pub fn input(&mut self, epoch: u64, bytes: usize) -> Result<bool, Invalid> {
        if epoch > self.epoch {
            self.poisoned = true;
            return Err(Invalid("future terminal epoch"));
        }
        self.change(bytes, false)?;
        Ok(epoch == self.epoch)
    }
    pub fn taken(&mut self, bytes: u64) -> Result<(), Invalid> {
        self.release(bytes, false)
    }
    pub fn depart(&mut self) -> Result<u64, Invalid> {
        if self.poisoned {
            return Err(Invalid("retired terminal flow"));
        }
        let next = self
            .epoch
            .checked_add(1)
            .ok_or(Invalid("terminal epochs exhausted"));
        match next {
            Ok(next) => {
                self.epoch = next;
                Ok(next)
            }
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }
    fn change(&mut self, bytes: usize, output: bool) -> Result<(), Invalid> {
        let result = (|| {
            if self.poisoned {
                return Err(Invalid("retired terminal flow"));
            }
            if bytes > MAX_DATA {
                return Err(Invalid("terminal data size"));
            }
            let bytes = u64::try_from(bytes).map_err(|_| Invalid("terminal data size"))?;
            let (used, limit) = if output {
                (&mut self.output, OUTPUT_CREDIT)
            } else {
                (&mut self.input, INPUT_WINDOW)
            };
            let next = used
                .checked_add(bytes)
                .ok_or(Invalid("terminal counter overflow"))?;
            if next > limit {
                return Err(Invalid("terminal credit"));
            }
            *used = next;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn release(&mut self, bytes: u64, output: bool) -> Result<(), Invalid> {
        if self.poisoned {
            return Err(Invalid("retired terminal flow"));
        }
        let used = if output {
            &mut self.output
        } else {
            &mut self.input
        };
        let Some(next) = used.checked_sub(bytes) else {
            self.poisoned = true;
            return Err(Invalid("terminal excess acknowledgement"));
        };
        *used = next;
        Ok(())
    }
}
