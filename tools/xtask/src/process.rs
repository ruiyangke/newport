use anyhow::{bail, ensure, Context, Result};
use std::{
    io::{Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
pub struct Process(pub Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Process {
    pub fn spawn(c: &mut Command) -> Result<Self> {
        Ok(Self(c.spawn().context("Starting fixture process")?))
    }
    pub fn wait(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(s) = self.0.try_wait()? {
                return Ok(s);
            }
            ensure!(
                Instant::now() < deadline,
                "Fixture process exceeded {timeout:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    pub fn success(&mut self, timeout: Duration) -> Result<()> {
        ensure!(self.wait(timeout)?.success(), "Fixture process failed");
        Ok(())
    }
}
pub fn run(c: &mut Command) -> Result<()> {
    Process::spawn(c)?.success(Duration::from_secs(1200))
}
pub fn capture(c: &mut Command, input: Option<&[u8]>) -> Result<Vec<u8>> {
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = Process::spawn(c)?;
    let output = child.0.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let mut stdin = child.0.stdin.take().unwrap();
    let bytes = input.unwrap_or_default().to_vec();
    let writer = thread::spawn(move || stdin.write_all(&bytes));
    let status = child.wait(Duration::from_secs(120))?;
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Output reader panicked"))??;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("Input writer panicked"))??;
    ensure!(status.success(), "Fixture command failed: {status}");
    ensure!(bytes.len() <= 32 * 1024 * 1024, "Fixture output too large");
    Ok(bytes)
}
pub fn text(c: &mut Command) -> Result<String> {
    Ok(String::from_utf8(capture(c, None)?)?.trim().into())
}
pub fn ready(path: &std::path::Path, child: &mut Process) -> Result<()> {
    for _ in 0..100 {
        if path.exists() {
            return Ok(());
        }
        if let Some(status) = child.0.try_wait()? {
            bail!("Fixture exited before readiness: {status}");
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("Fixture did not become ready")
}
