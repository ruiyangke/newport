//! Finite identity scanning over a caller-owned control pipe.
use super::{Discovery, Failure, Mode};
use newport_protocol::{
    common,
    monitor::{
        self,
        ports::discovery::{
            Issue, IssueKind, Outcome, Report, Snapshot,
            helper::{Reply, Request, wire},
        },
    },
};
use std::{
    fs::File,
    io,
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};

mod client;
mod elevated;
mod io_bound;
pub use client::{Client, Error};
pub use elevated::Elevated;
use io_bound::{Control, Input, Nonblocking, Output};

pub const LIFETIME: Duration = Duration::from_secs(5);

/// Serves scans, one per request, until the caller closes the control pipe,
/// without starting a runtime, service manager, or display. Each scan is
/// bounded by [`LIFETIME`] and ends early if the pipe closes during it.
pub fn serve() -> io::Result<()> {
    let input = File::from(io::stdin().as_fd().try_clone_to_owned()?);
    let output = File::from(io::stdout().as_fd().try_clone_to_owned()?);
    serve_on(&input, &output, |request, deadline, cancelled| {
        Discovery::default()
            .collect(request.generation, deadline, Mode::Identity, cancelled)
            .map(|scan| scan.snapshot)
    })
}

fn serve_on(
    input: &File,
    output: &File,
    collect: impl Fn(&Request, Instant, &dyn Fn() -> bool) -> Result<Snapshot, Failure>,
) -> io::Result<()> {
    while request_waiting(input)? {
        exchange(input, output, Instant::now() + LIFETIME, false, &collect)?;
    }
    Ok(())
}

/// Waits for the next request: false once the caller has closed the pipe.
fn request_waiting(input: &File) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: input.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one initialized pollfd for a descriptor `input` keeps open.
    while unsafe { libc::poll(&mut poll, 1, -1) } < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    let mut available: libc::c_int = 0;
    // SAFETY: FIONREAD writes one int for a descriptor `input` keeps open.
    if unsafe { libc::ioctl(input.as_raw_fd(), libc::FIONREAD, &mut available) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available > 0)
}

/// Serves one project enrichment, as the account, then exits.
pub fn serve_projects() -> io::Result<()> {
    let input = File::from(io::stdin().as_fd().try_clone_to_owned()?);
    let output = File::from(io::stdout().as_fd().try_clone_to_owned()?);
    exchange(
        &input,
        &output,
        Instant::now() + LIFETIME,
        true,
        |request, deadline, cancelled| {
            let Some(snapshot) = &request.projects else {
                return Err(Failure::Invalid("Project helper requires a snapshot"));
            };
            let mut snapshot = *snapshot.clone();
            let root = std::path::Path::new("/");
            if snapshot.scope != super::scan::scope(root) {
                return Err(Failure::Unavailable("Project helper namespace changed"));
            }
            super::projects::enrich(root, &mut snapshot, deadline, cancelled);
            Ok(snapshot)
        },
    )
}

fn exchange(
    input: &File,
    output: &File,
    deadline: Instant,
    projects: bool,
    collect: impl FnOnce(&Request, Instant, &dyn Fn() -> bool) -> Result<Snapshot, Failure>,
) -> io::Result<()> {
    let _input_flags = Nonblocking::pipe(input.as_fd())?;
    let _output_flags = Nonblocking::pipe(output.as_fd())?;
    let request = wire::read_request(&mut Input {
        file: input,
        deadline,
    })?;
    let control = Control::new(input, deadline);
    control.finish(Ok(()))?;
    let result = match wire::require_contract_id(request.contract_id.as_deref()) {
        Ok(()) if request.projects.is_some() != projects => Err(common::Error {
            code: "invalid_request".into(),
            message: "Discovery helper command does not accept this operation.".into(),
            retry: common::Retry::No,
        }),
        Ok(()) => collect(&request, deadline, &|| control.cancelled())
            .and_then(|snapshot| {
                if !monitor::ports::discovery::wire::fits_response(&snapshot)? {
                    return Err(Failure::Exhausted(
                        "Discovery helper observation exceeds the response budget.",
                    ));
                }
                Ok(Box::new(snapshot))
            })
            .map_err(scan_error),
        Err(error) => Err(error),
    };
    control.finish(Ok(()))?;
    let reply = Reply {
        contract_id: Some(monitor::CONTRACT_ID.into()),
        generation: request.generation,
        result,
    };
    let result = wire::write_reply(
        &mut Output {
            file: output,
            control: &control,
        },
        &reply,
        &request,
    )
    .map_err(io::Error::from);
    // The caller may send its next request as soon as the reply is complete,
    // so the pipe is no longer read for cancellation here.
    control.settle(result)
}

fn scan_error(failure: Failure) -> common::Error {
    let (code, retry) = match failure.kind() {
        IssueKind::PermissionDenied => ("permission_denied", common::Retry::No),
        IssueKind::Unsupported => ("unsupported", common::Retry::No),
        IssueKind::TimedOut => ("timed_out", common::Retry::Yes),
        IssueKind::Interrupted => ("interrupted", common::Retry::Yes),
        IssueKind::BudgetExceeded => ("budget_exceeded", common::Retry::No),
        IssueKind::Io => ("io", common::Retry::Yes),
        IssueKind::InvalidResponse => ("invalid_response", common::Retry::No),
        IssueKind::Unavailable => ("unavailable", common::Retry::Unknown),
    };
    let mut message = failure.to_string();
    super::super::truncate(&mut message, monitor::ports::discovery::MAX_DIAGNOSTIC);
    common::Error {
        code: code.into(),
        message,
        retry,
    }
}

pub(super) fn report(error: &Error) -> Report {
    let kind = match error {
        Error::Io(error) => match error.kind() {
            std::io::ErrorKind::PermissionDenied => IssueKind::PermissionDenied,
            std::io::ErrorKind::NotFound | std::io::ErrorKind::Unsupported => {
                IssueKind::Unavailable
            }
            _ => IssueKind::Io,
        },
        Error::TimedOut => IssueKind::TimedOut,
        Error::Cancelled => IssueKind::Interrupted,
        Error::Wire(_) | Error::Invalid(_) => IssueKind::InvalidResponse,
        Error::Remote(error) => match error.code.as_str() {
            "permission_denied" => IssueKind::PermissionDenied,
            "unsupported" => IssueKind::Unsupported,
            "timed_out" => IssueKind::TimedOut,
            "interrupted" => IssueKind::Interrupted,
            "budget_exceeded" => IssueKind::BudgetExceeded,
            "invalid_response" => IssueKind::InvalidResponse,
            "io" => IssueKind::Io,
            _ => IssueKind::Unavailable,
        },
        Error::Refused { .. } => IssueKind::PermissionDenied,
        Error::Exited { .. } | Error::CleanupPending { .. } => IssueKind::Unavailable,
    };
    let mut message = error.to_string();
    super::super::truncate(
        &mut message,
        newport_protocol::monitor::ports::discovery::MAX_DIAGNOSTIC,
    );
    Report {
        outcome: Outcome::Unavailable,
        issues: vec![Issue {
            kind,
            count: 1,
            message,
        }],
    }
}

#[cfg(test)]
mod tests;
