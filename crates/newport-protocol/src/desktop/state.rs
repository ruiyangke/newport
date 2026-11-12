use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Hello,
    Ready,
    Request,
    Pending,
    Closing,
    Opened,
    Ended,
    Poisoned,
}
#[derive(Clone, Copy, Debug)]
enum StreamIdentity {
    Terminal(uuid::Uuid),
    Command(uuid::Uuid),
    RemoteApps(uuid::Uuid),
    Lsp(uuid::Uuid),
}
/// Small correlation state; the executor retains the request payload without copying it.
#[derive(Clone, Copy, Debug)]
pub struct RequestContext {
    pub id: RequestId,
    pub kind: Kind,
    pub server: Option<uuid::Uuid>,
    stream: Option<StreamIdentity>,
}
impl RequestContext {
    fn of(value: &Request) -> Self {
        let stream = match &value.operation {
            Operation::Stream(value) => Some(match value.as_ref() {
                stream::Operation::TerminalOpen(value) => StreamIdentity::Terminal(value.session),
                stream::Operation::CommandOpen(value) => StreamIdentity::Command(value.server),
                stream::Operation::RemoteAppsOpen(value) => {
                    StreamIdentity::RemoteApps(value.server)
                }
                stream::Operation::LspOpen(value) => StreamIdentity::Lsp(value.server),
            }),
            _ => None,
        };
        Self {
            id: value.id,
            kind: value.operation.kind(),
            server: value.operation.server_id(),
            stream,
        }
    }
    fn progress(&self, value: &Progress) -> Result<(), Invalid> {
        value.validate()?;
        if value.id != self.id {
            return Err(Invalid("progress correlation"));
        }
        match &value.state {
            ProgressState::AgentApproval(v) if Some(v.server) == self.server => Ok(()),
            _ => Err(Invalid("progress server")),
        }
    }
    fn response(&self, value: &Response) -> Result<(), Invalid> {
        value.validate()?;
        if value.id != self.id {
            return Err(Invalid("response correlation"));
        }
        let valid = match &value.outcome {
            Outcome::Completed(reply) => reply.kind() == self.kind,
            Outcome::Failed(_) => true,
            Outcome::Opened(value) => match (self.stream, value.as_ref()) {
                (Some(StreamIdentity::Terminal(id)), stream::Opened::Terminal(v)) => {
                    id == v.session
                }
                (Some(StreamIdentity::Command(id)), stream::Opened::Command(v)) => id == v.server,
                (Some(StreamIdentity::RemoteApps(id)), stream::Opened::RemoteApps(v)) => {
                    id == v.server && v.service_version == crate::remote_apps::REVISION
                }
                (Some(StreamIdentity::Lsp(id)), stream::Opened::Lsp(v)) => id == v.server,
                _ => false,
            },
        };
        if valid {
            Ok(())
        } else {
            Err(Invalid("response operation"))
        }
    }
}
/// Pure phase tracking. Adapters also validate results against the retained full Request.
#[derive(Debug)]
pub struct Exchange {
    phase: Phase,
    request: Option<RequestContext>,
    contract_id: Option<String>,
}
impl Default for Exchange {
    fn default() -> Self {
        Self {
            phase: Phase::Hello,
            request: None,
            contract_id: None,
        }
    }
}
impl Exchange {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn request(&self) -> Option<&RequestContext> {
        self.request.as_ref()
    }
    pub fn client(&mut self, message: &ClientMessage) -> Result<(), Invalid> {
        let result = match (self.phase, message) {
            (Phase::Hello, ClientMessage::Hello(value)) => value.validate().map(|()| {
                self.contract_id.clone_from(&value.contract_id);
                self.phase = Phase::Ready;
            }),
            (Phase::Request, ClientMessage::Request(value)) => value.validate().map(|()| {
                self.request = Some(RequestContext::of(value));
                self.phase = Phase::Pending;
            }),
            _ => Err(Invalid("client phase")),
        };
        if result.is_err() {
            self.poison();
        }
        result
    }
    pub fn server(&mut self, message: &ServerMessage) -> Result<(), Invalid> {
        let result = match (self.phase, message) {
            (Phase::Ready, ServerMessage::Ready(value)) => value.validate().and_then(|()| {
                if self.contract_id.is_none() || value.contract_id != self.contract_id {
                    return Err(Invalid("desktop contract mismatch"));
                }
                self.phase = Phase::Request;
                Ok(())
            }),
            (Phase::Ready, ServerMessage::Refused(_)) => {
                self.phase = Phase::Closing;
                Ok(())
            }
            (Phase::Pending, ServerMessage::Progress(value)) => self
                .request
                .as_ref()
                .ok_or(Invalid("pending request"))?
                .progress(value),
            (Phase::Pending, ServerMessage::Response(value)) => self
                .request
                .as_ref()
                .ok_or(Invalid("pending request"))?
                .response(value)
                .map(|()| {
                    self.phase = if matches!(&value.outcome, Outcome::Opened(_)) {
                        Phase::Opened
                    } else {
                        Phase::Closing
                    };
                }),
            _ => Err(Invalid("server phase")),
        };
        if result.is_err() {
            self.poison();
        }
        result
    }
    /// Call only after bounded transport EOF verification; a final reply alone is not closure.
    pub fn eof(&mut self) -> Result<(), Invalid> {
        if self.phase == Phase::Closing {
            self.phase = Phase::Ended;
            Ok(())
        } else {
            self.poison();
            Err(Invalid("premature EOF"))
        }
    }
    pub fn poison(&mut self) {
        self.phase = Phase::Poisoned;
        self.request = None;
    }
}
#[derive(Debug, Default)]
pub struct IdSequence {
    last: u64,
}
impl IdSequence {
    pub const fn after(last: u64) -> Self {
        Self { last }
    }
    pub fn allocate(&mut self) -> Result<RequestId, Invalid> {
        let next = self
            .last
            .checked_add(1)
            .ok_or(Invalid("request identifiers exhausted"))?;
        let value = RequestId::new(next)?;
        self.last = next;
        Ok(value)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocation {
    Ordinary,
    Large,
}
/// Inspect the announced frame size before allocation. Retain the permit until its body is dropped.
pub fn request_allocation(length: u64) -> Result<Allocation, Invalid> {
    if length == 0 {
        return Err(Invalid("empty request frame"));
    }
    if length <= MAX_REQUEST as u64 {
        Ok(Allocation::Ordinary)
    } else if length <= MAX_SAVE_REQUEST as u64 {
        Ok(Allocation::Large)
    } else {
        Err(Invalid("request frame size"))
    }
}
