//! The identity check owns its process tree and both asynchronous output pipes.
use super::{OUTPUT_LIMIT, PROBE_TIMEOUT, windows_probe_command};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use newport_win32::{
    command_line::{command_line, wide},
    handle::{check, null_device, owned as handle, signaled},
    job::Job,
    pipe,
    process::{self as child, Spawn},
    security::{Descriptor, Token},
};
use serde::Serialize;
use std::{
    cell::UnsafeCell,
    collections::BTreeMap,
    ffi::OsStr,
    fs, io,
    os::windows::{
        io::{AsHandle, AsRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    path::Path,
    process::{Command, ExitStatus, Output},
    ptr,
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::Cryptography::*,
    Storage::FileSystem::*,
    System::{IO::*, JobObjects::*, Pipes::*, Threading::*},
};

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(10);
const READ_SIZE: usize = 16 * 1024;
const MAX_OBSERVED_PROCESSES: usize = 128;
const IDLE: u8 = 0;
const RUNNING: u8 = 1;
const QUARANTINED: u8 = 2;
// Only one context can need retention; later calls cannot accumulate live jobs.
static STATE: AtomicU8 = AtomicU8::new(IDLE);

struct Profile(Option<tempfile::TempDir>);
impl Profile {
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
    fn retain(&mut self) -> std::path::PathBuf {
        self.0.take().unwrap().keep()
    }
    fn close(&mut self) -> Result<()> {
        let home = self.0.take().unwrap();
        let path = home.path().to_owned();
        home.close()
            .with_context(|| format!("Probe profile cleanup is incomplete at {}", path.display()))
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        if STATE.load(Ordering::Acquire) == QUARANTINED
            && let Some(home) = self.0.take()
        {
            let _ = home.keep();
        }
    }
}

pub(super) fn run(executable: &Path) -> Result<Vec<u8>> {
    ensure!(
        STATE.load(Ordering::Acquire) != QUARANTINED,
        "Previous probe cleanup is unconfirmed"
    );
    let mut home = Profile(Some(tempfile::tempdir()?));
    let system = std::env::var_os("SystemRoot").context("SystemRoot is required for the probe")?;
    let executable = executable
        .canonicalize()
        .context("Cannot locate the packaged desktop")?;
    let command = windows_probe_command(&executable, home.path(), Path::new(&system))?;
    let output = match capture(&command, PROBE_TIMEOUT) {
        Ok(output) => output,
        Err(failure) if failure.cleanup.confirmed() => {
            home.close().with_context(|| failure.error.to_string())?;
            return Err(failure.error);
        }
        Err(failure) => {
            let retained = home.retain();
            // Captured application output never enters diagnostics or evidence.
            let saved = serde_json::to_vec_pretty(&failure.cleanup)
                .ok()
                .is_some_and(|evidence| {
                    fs::write(retained.join("probe-cleanup.json"), evidence).is_ok()
                });
            bail!(
                "{}; probe cleanup could not be confirmed; retained profile: {} (cleanup receipt saved: {saved})",
                failure.error,
                retained.display()
            );
        }
    };
    home.close()?;
    ensure!(
        output.status.success(),
        "The packaged desktop could not report its agent identity"
    );
    Ok(output.stdout)
}

#[derive(Debug, Default, Serialize)]
struct Cleanup {
    schema_version: u32,
    process_id: u32,
    process_exited_before_cleanup: bool,
    process_exited: bool,
    job_empty: bool,
    total_processes: u32,
    members_checked: bool,
    observed_members_exited: bool,
    member_observation_error: Option<String>,
    readers_stopped: bool,
    #[cfg(test)]
    processes: Vec<tests::ProcessReceipt>,
}
impl Cleanup {
    fn confirmed(&self) -> bool {
        self.process_exited
            && self.job_empty
            && self.members_checked
            && self.observed_members_exited
            && self.readers_stopped
    }
    fn no_child() -> Self {
        Self {
            schema_version: 1,
            process_exited: true,
            job_empty: true,
            members_checked: true,
            observed_members_exited: true,
            readers_stopped: true,
            ..Self::default()
        }
    }
}
#[derive(Debug)]
struct Failure {
    error: anyhow::Error,
    cleanup: Cleanup,
}

/// Only SYSTEM and this user may open the output pipes.
fn private_security() -> Result<Descriptor> {
    let user = Token::current()?.user()?;
    Ok(Descriptor::from_sddl(&format!(
        "D:P(A;;GA;;;SY)(A;;GA;;;{user})"
    ))?)
}

struct ReadPipe {
    pipe: OwnedHandle,
    event: OwnedHandle,
    // The enclosing Box pins these addresses while the kernel may write them.
    overlapped: UnsafeCell<OVERLAPPED>,
    buffer: UnsafeCell<[u8; READ_SIZE]>,
    data: Vec<u8>,
    pending: bool,
    eof: bool,
}
impl ReadPipe {
    fn new(security: &Descriptor) -> Result<(Box<Self>, OwnedHandle)> {
        let mut random = [0u8; 16];
        // SAFETY: The buffer and its length describe `random`; a null algorithm handle
        // selects the system RNG.
        ensure!(
            unsafe {
                BCryptGenRandom(
                    ptr::null_mut(),
                    random.as_mut_ptr(),
                    random.len() as u32,
                    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                )
            } == 0,
            "Cannot name private probe pipes"
        );
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = wide(OsStr::new(&format!(r"\\.\pipe\NewportIdentity-{name}")))?;
        // SAFETY: `name` is NUL-terminated, and the attributes temporary points into
        // `security`; both outlive the call.
        let pipe = unsafe {
            handle(CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                4096,
                0,
                &security.attributes(false),
            ))
        }?;
        let writer = pipe::open_client(&name, GENERIC_WRITE, 0, true)?;
        ensure!(
            pipe::peer_pid(pipe.as_handle(), true)? == std::process::id(),
            "Private probe pipe was connected by another process"
        );
        // SAFETY: Null attributes and name are allowed; the new handle is owned by `handle`.
        let event = unsafe { handle(CreateEventW(ptr::null(), 1, 0, ptr::null())) }?;
        // SAFETY: OVERLAPPED is plain data for which all-zero is a valid value.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.as_raw_handle();
        Ok((
            Box::new(Self {
                pipe,
                event,
                overlapped: UnsafeCell::new(overlapped),
                buffer: UnsafeCell::new([0; READ_SIZE]),
                data: Vec::new(),
                pending: false,
                eof: false,
            }),
            writer,
        ))
    }
    fn append(&mut self, count: u32) -> Result<()> {
        ensure!(count as usize <= READ_SIZE, "Invalid probe output read");
        ensure!(
            self.data.len() as u64 + count as u64 <= OUTPUT_LIMIT,
            "The check exceeded its output limit"
        );
        // SAFETY: Called only after a read completed, so the kernel no longer writes it.
        let buffer = unsafe { &*self.buffer.get() };
        self.data.extend_from_slice(&buffer[..count as usize]);
        Ok(())
    }
    fn poll(&mut self, discard: bool) -> Result<()> {
        if self.pending {
            // SAFETY: The event handle is owned by `self`.
            match unsafe { WaitForSingleObject(self.event.as_raw_handle(), 0) } {
                WAIT_TIMEOUT => return Ok(()),
                WAIT_OBJECT_0 => {}
                _ => return Err(io::Error::last_os_error().into()),
            }
            let mut count = 0;
            // SAFETY: The pipe and OVERLAPPED belong to the read this pipe started, and
            // the boxed OVERLAPPED stays at a fixed address.
            if unsafe {
                GetOverlappedResult(
                    self.pipe.as_raw_handle(),
                    self.overlapped.get(),
                    &mut count,
                    0,
                )
            } == 0
            {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_IO_INCOMPLETE as i32) {
                    return Ok(());
                }
                self.pending = false;
                if matches!(error.raw_os_error(), Some(code) if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_OPERATION_ABORTED as i32)
                {
                    self.eof = true;
                    return Ok(());
                }
                return Err(error.into());
            }
            self.pending = false;
            if !discard {
                self.append(count)?;
            }
        }
        if discard || self.eof {
            return Ok(());
        }
        // A read that completes at once leaves no event for the supervisor to
        // wake on, so buffered output is drained until a read is pending. The
        // output limit bounds the drain, and a zero-byte read ends it.
        loop {
            // SAFETY: The event handle is owned by `self`.
            check(unsafe { ResetEvent(self.event.as_raw_handle()) })?;
            // SAFETY: No read is pending here, so the kernel does not use the OVERLAPPED.
            unsafe {
                *self.overlapped.get() = std::mem::zeroed();
                (*self.overlapped.get()).hEvent = self.event.as_raw_handle();
            }
            let mut count = 0;
            // SAFETY: The boxed buffer and OVERLAPPED stay at fixed addresses, and Drop of the
            // owning `Run` leaks them if the read cannot be confirmed finished.
            let read = unsafe {
                ReadFile(
                    self.pipe.as_raw_handle(),
                    self.buffer.get().cast(),
                    READ_SIZE as u32,
                    ptr::null_mut(),
                    self.overlapped.get(),
                )
            };
            if read != 0 {
                // SAFETY: The read just completed on this pipe and OVERLAPPED.
                check(unsafe {
                    GetOverlappedResult(
                        self.pipe.as_raw_handle(),
                        self.overlapped.get(),
                        &mut count,
                        0,
                    )
                })?;
                self.append(count)?;
                if count == 0 {
                    return Ok(());
                }
                continue;
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => self.pending = true,
                Some(code) if code == ERROR_BROKEN_PIPE as i32 => self.eof = true,
                _ => return Err(error.into()),
            }
            return Ok(());
        }
    }
    fn cancel(&mut self) {
        if self.pending {
            // SAFETY: Cancels only the pending read on this pipe's OVERLAPPED.
            unsafe {
                CancelIoEx(self.pipe.as_raw_handle(), self.overlapped.get());
            }
        }
    }
}

// These handles fence observed live members against PID reuse and let cleanup
// wait past the job-accounting transition. They are not a historical census.
#[derive(Default)]
struct Processes(BTreeMap<u32, OwnedHandle>);
impl Processes {
    fn snapshot(job: HANDLE) -> Result<Vec<u32>> {
        // Two DWORD header fields precede the pointer-sized IDs on both ABIs.
        let mut storage = [0usize; MAX_OBSERVED_PROCESSES + 2];
        // SAFETY: `storage` is writable for the stated size, and `job` is a live job handle.
        check(unsafe {
            QueryInformationJobObject(
                job,
                JobObjectBasicProcessIdList,
                storage.as_mut_ptr().cast(),
                size_of_val(&storage) as u32,
                ptr::null_mut(),
            )
        })
        .context("Cannot query probe job members")?;
        // SAFETY: `storage` is pointer-aligned, large enough for the header, and was
        // initialized by the query.
        let list = unsafe { &*storage.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
        let count = list.NumberOfProcessIdsInList as usize;
        ensure!(
            count <= MAX_OBSERVED_PROCESSES
                && list.NumberOfAssignedProcesses == list.NumberOfProcessIdsInList,
            "Probe job member list exceeds its observation limit"
        );
        // SAFETY: `count` is within the storage capacity, and the slice is derived from
        // the whole buffer, not from the one-element array the header declares.
        let ids = unsafe {
            std::slice::from_raw_parts(
                storage
                    .as_ptr()
                    .cast::<u8>()
                    .add(std::mem::offset_of!(
                        JOBOBJECT_BASIC_PROCESS_ID_LIST,
                        ProcessIdList
                    ))
                    .cast::<usize>(),
                count,
            )
        };
        ids.iter()
            .map(|&id| u32::try_from(id).context("Invalid probe job process ID"))
            .collect()
    }

    fn observe(&mut self, job: HANDLE, parent: u32) -> Result<()> {
        for pid in Self::snapshot(job)? {
            if pid == parent || self.0.contains_key(&pid) {
                continue;
            }
            if self.0.len() == MAX_OBSERVED_PROCESSES {
                // Completed handles can be retired without weakening the live
                // member fence or retaining an unbounded process history.
                self.0.retain(|_, process| {
                    // SAFETY: The process handle is owned by the map.
                    (unsafe { WaitForSingleObject(process.as_raw_handle(), 0) }) != WAIT_OBJECT_0
                });
            }
            ensure!(
                self.0.len() < MAX_OBSERVED_PROCESSES,
                "Too many live probe job members"
            );
            // SAFETY: Plain handle query; the new handle is owned by `handle`.
            let process = match unsafe {
                handle(OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                ))
            } {
                Ok(process) => process,
                Err(error) => {
                    // A PID may disappear between enumeration and OpenProcess.
                    // Recheck this job; a persistent access/query failure is not
                    // evidence of termination.
                    if !Self::snapshot(job)?.contains(&pid) {
                        continue;
                    }
                    return Err(error).context("Cannot retain a live probe job member");
                }
            };
            let mut member = 0;
            // SAFETY: Both handles are live and `member` is writable.
            check(unsafe { IsProcessInJob(process.as_raw_handle(), job, &mut member) })
                .context("Cannot verify a probe job member")?;
            if member != 0 {
                self.0.insert(pid, process);
            }
            // An unrelated process may have reused a departed member's PID.
            // Never retain or act on it without membership in this exact job.
        }
        Ok(())
    }

    fn exited(&self) -> Result<bool> {
        for process in self.0.values() {
            // SAFETY: The process handle is owned by the map.
            match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => return Ok(false),
                _ => {
                    return Err(io::Error::last_os_error())
                        .context("Cannot wait for probe job member");
                }
            }
        }
        Ok(true)
    }
}

struct Native {
    job: Job,
    process: Option<OwnedHandle>,
    process_id: u32,
    stdout: Box<ReadPipe>,
    stderr: Box<ReadPipe>,
    processes: Processes,
}
impl Native {
    fn new(command: &Command) -> Result<Self> {
        ensure!(
            Path::new(command.get_program()).is_absolute(),
            "Probe executable must be absolute"
        );
        let directory = command
            .get_current_dir()
            .context("Probe working directory is required")?;
        ensure!(
            directory.is_absolute(),
            "Probe working directory must be absolute"
        );
        let directory = wide(directory.as_os_str())?;
        let arguments = command_line(command.get_program(), command.get_args())?;
        ensure!(arguments.len() <= 32767, "Probe command is too long");
        // Command's explicitly configured values are the whole environment.
        let mut values = command.get_envs().collect::<Vec<_>>();
        values.sort_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
        let mut environment = Vec::new();
        for (key, value) in values {
            let value = value.context("Probe environment cannot contain removals")?;
            let key = wide(key)?;
            ensure!(
                key.len() > 1 && !key.contains(&(b'=' as u16)),
                "Invalid probe environment key"
            );
            environment.extend_from_slice(&key[..key.len() - 1]);
            environment.push(b'=' as u16);
            environment.extend(wide(value)?);
        }
        environment.push(0);
        if environment.len() == 1 {
            environment.push(0);
        }
        let security = private_security()?;
        let (stdout, output_writer) = ReadPipe::new(&security)?;
        let (stderr, error_writer) = ReadPipe::new(&security)?;
        let stdin = null_device(GENERIC_READ)?;
        let job = Job::new(None)?;
        let inherited = [
            stdin.as_raw_handle(),
            output_writer.as_raw_handle(),
            error_writer.as_raw_handle(),
        ];
        let spawned = child::spawn(Spawn {
            application: command.get_program(),
            command_line: arguments,
            job: &job,
            stdin: inherited[0],
            stdout: inherited[1],
            stderr: inherited[2],
            inherit: inherited.to_vec(),
            flags: DETACHED_PROCESS,
            environment: Some(environment),
            directory: Some(directory),
            desktop: None,
        })?;
        // Job membership is atomic with creation, before any application code runs.
        Ok(Self {
            job,
            process: Some(spawned.process),
            process_id: spawned.id,
            stdout,
            stderr,
            processes: Processes::default(),
        })
    }
    fn exited(&self) -> io::Result<bool> {
        self.process.as_ref().map_or(Ok(true), signaled)
    }
    fn accounting(&self) -> io::Result<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
        self.job.accounting()
    }
    fn wait(&self, deadline: Instant) -> Result<()> {
        let mut events = Vec::with_capacity(3);
        if !self.exited()? {
            events.push(self.process.as_ref().unwrap().as_raw_handle());
        }
        for pipe in [&self.stdout, &self.stderr] {
            if pipe.pending {
                events.push(pipe.event.as_raw_handle());
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now()).min(POLL);
        if events.is_empty() {
            std::thread::sleep(remaining);
            return Ok(());
        }
        // SAFETY: `events` holds live handles owned by `self`, and its length is passed.
        let result = unsafe {
            WaitForMultipleObjects(
                events.len() as u32,
                events.as_ptr(),
                0,
                remaining.as_millis() as u32,
            )
        };
        if result == WAIT_FAILED {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    fn collect(&mut self, deadline: Instant) -> Result<Output> {
        loop {
            let members_checked = self
                .processes
                .observe(self.job.as_raw_handle(), self.process_id)
                .is_ok();
            ensure!(Instant::now() < deadline, "The check exceeded its deadline");
            self.stdout.poll(false)?;
            self.stderr.poll(false)?;
            if self.exited()?
                && self.stdout.eof
                && self.stderr.eof
                && self.accounting()?.ActiveProcesses == 0
                && members_checked
                && self.processes.exited()?
            {
                let mut status = 0;
                // SAFETY: The process handle is owned by `self` and `status` is writable.
                check(unsafe {
                    GetExitCodeProcess(self.process.as_ref().unwrap().as_raw_handle(), &mut status)
                })?;
                return Ok(Output {
                    status: ExitStatus::from_raw(status),
                    stdout: std::mem::take(&mut self.stdout.data),
                    stderr: std::mem::take(&mut self.stderr.data),
                });
            }
            self.wait(deadline)?;
        }
    }
    fn cleanup(&mut self) -> Cleanup {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        let process_exited_before_cleanup = self.exited().unwrap_or(false);
        let _ = self
            .processes
            .observe(self.job.as_raw_handle(), self.process_id);
        self.job.terminate(1);
        self.stdout.cancel();
        self.stderr.cancel();
        loop {
            let members = self
                .processes
                .observe(self.job.as_raw_handle(), self.process_id);
            let member_exits = self.processes.exited();
            let _ = self.stdout.poll(true);
            let _ = self.stderr.poll(true);
            let accounting = self.accounting().ok();
            let result = Cleanup {
                schema_version: 1,
                process_id: self.process_id,
                process_exited_before_cleanup,
                process_exited: self.exited().unwrap_or(false),
                job_empty: accounting
                    .as_ref()
                    .is_some_and(|value| value.ActiveProcesses == 0),
                total_processes: accounting.map_or(0, |value| value.TotalProcesses),
                members_checked: members.is_ok(),
                observed_members_exited: member_exits.as_ref().is_ok_and(|exited| *exited),
                member_observation_error: members
                    .err()
                    .or_else(|| member_exits.err())
                    .map(|error| error.to_string()),
                readers_stopped: !self.stdout.pending && !self.stderr.pending,
                #[cfg(test)]
                processes: self
                    .processes
                    .receipt(self.process_id, self.process.as_ref().unwrap()),
            };
            if result.confirmed() || Instant::now() >= deadline {
                return result;
            }
            let _ = self.wait(deadline);
        }
    }
}

struct Run(Option<Box<Native>>);
impl Drop for Run {
    fn drop(&mut self) {
        if let Some(mut native) = self.0.take()
            && !native.cleanup().confirmed()
        {
            STATE.store(QUARANTINED, Ordering::Release);
            // Win32 may still own OVERLAPPED/buffer addresses. Keep the one
            // context intact, and permanently refuse subsequent probes.
            std::mem::forget(native);
            return;
        }
        if STATE.load(Ordering::Acquire) != QUARANTINED {
            STATE.store(IDLE, Ordering::Release);
        }
    }
}

fn capture(command: &Command, timeout: Duration) -> std::result::Result<Output, Failure> {
    STATE
        .compare_exchange(IDLE, RUNNING, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|state| Failure {
            error: anyhow!(if state == QUARANTINED {
                "Previous probe cleanup is unconfirmed"
            } else {
                "An identity probe is already running"
            }),
            cleanup: Cleanup::no_child(),
        })?;
    let mut run = Run(None);
    let deadline = Instant::now() + timeout;
    run.0 = Some(Box::new(Native::new(command).map_err(|error| Failure {
        error,
        cleanup: Cleanup::no_child(),
    })?));
    let native = run.0.as_mut().unwrap();
    let output = native.collect(deadline);
    let cleanup = native.cleanup();
    if !cleanup.confirmed() {
        STATE.store(QUARANTINED, Ordering::Release);
        std::mem::forget(run.0.take().unwrap());
        return Err(Failure {
            error: output
                .err()
                .unwrap_or_else(|| anyhow!("Identity probe cleanup failed")),
            cleanup,
        });
    }
    // Output I/O, the direct child, and retained live-member handles have
    // completed, and the exact job is empty before its handles are freed.
    drop(run.0.take());
    output.map_err(|error| Failure { error, cleanup })
}

#[cfg(test)]
#[path = "windows_probe_tests.rs"]
mod tests;
