//! Lingering while Newport needs it. logind stops an account's user manager,
//! and every scope in it, at the account's last logout unless the account
//! lingers, which an account may set for itself. A stable display or a held
//! terminal must outlive that logout, so starting one turns lingering on when
//! it is off, with a marker saying Newport did; once none runs, `reconcile`
//! turns it off again and removes the marker. Lingering an account had
//! already is never touched.
use crate::detached::bounded;
use std::{
    ffi::OsString,
    fs::{self, File},
    io,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The scope of a stable display, and the prefix of a held terminal's.
const DISPLAY_SCOPE: &str = "newport-display.scope";
const TERMINAL_SCOPES: &str = "newport-terminal-*.scope";
/// Beside the marker: serializes turning lingering on with the start that
/// needs it, and checking for scopes with turning it off.
const LOCK: &str = "linger.lock";
/// Present while lingering is on because Newport turned it on.
const MARKER: &str = "linger";

pub(crate) struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        // A concurrent fork can retain this file description until exec.
        // Ownership ends with this scope, not with the last inherited handle.
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// The tools and state lingering goes through.
pub(crate) struct Linger {
    pub(crate) loginctl: OsString,
    pub(crate) systemctl: OsString,
    /// Where the marker lives; without it Newport never turns lingering on,
    /// since it couldn't turn it off again.
    pub(crate) state: Option<PathBuf>,
    pub(crate) uid: u32,
}

impl Linger {
    pub(crate) fn system() -> Self {
        let state = std::env::var_os("HOME").and_then(|home| {
            crate::paths::private_home_directory(home.as_ref(), ".local/state/newport").ok()
        });
        Self {
            loginctl: "loginctl".into(),
            systemctl: "systemctl".into(),
            state,
            uid: crate::paths::real_uid(),
        }
    }

    /// Holds the lock until dropped; `None` when there is no state.
    fn lock(&self) -> Option<Lock> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.state.as_ref()?.join(LOCK))
            .ok()?;
        fs2::FileExt::lock_exclusive(&file).ok()?;
        Some(Lock(file))
    }

    /// Before starting `unit`, which must outlive the last logout: lingering
    /// on if it is off, with the marker. A server that refuses is reported;
    /// `unit` then ends at that logout. Keep the returned lock until `unit`'s
    /// scope runs, so no reconcile turns lingering off before it counts.
    #[must_use]
    pub(crate) fn hold_for(&self, unit: &str) -> Option<Lock> {
        let state = self.state.as_ref()?;
        let lock = self.lock()?;
        if self.lingers() == Some(false) {
            if self.loginctl("enable-linger") {
                let _ = fs::write(state.join(MARKER), b"");
            } else {
                let what = if unit.starts_with("newport-terminal-") {
                    "terminals"
                } else {
                    "remote apps"
                };
                eprintln!(
                    "newport-agent: This server won't let the account keep running after logout, so {what} end when Newport disconnects."
                );
            }
        }
        Some(lock)
    }

    /// The display's admission deadline also covers logind and lock contention.
    pub(crate) fn hold_for_until(&self, unit: &str, deadline: Instant) -> io::Result<Option<Lock>> {
        bounded::remaining(deadline)?;
        let Some(state) = &self.state else {
            return Ok(None);
        };
        let Some(lock) = self.lock_until(deadline)? else {
            return Ok(None);
        };
        if self.lingers_until(deadline)? == Some(false) {
            let marker = state.join(MARKER);
            let marked = marker.exists();
            // Keep the marker if logind changes state but its reply times out.
            fs::write(&marker, b"")?;
            if !self.enable_until(deadline)? {
                if !marked {
                    let _ = fs::remove_file(marker);
                }
                let what = if unit.starts_with("newport-terminal-") {
                    "terminals"
                } else {
                    "remote apps"
                };
                eprintln!(
                    "newport-agent: This server won't let the account keep running after logout, so {what} end when Newport disconnects."
                );
            }
        }
        Ok(Some(lock))
    }

    fn lock_until(&self, deadline: Instant) -> io::Result<Option<Lock>> {
        let Some(state) = &self.state else {
            return Ok(None);
        };
        let Ok(file) = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state.join(LOCK))
        else {
            return Ok(None);
        };
        loop {
            bounded::remaining(deadline)?;
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Some(Lock(file))),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5).min(bounded::remaining(deadline)?));
                }
                Err(_) => return Ok(None),
            }
        }
    }

    fn lingers_until(&self, deadline: Instant) -> io::Result<Option<bool>> {
        let result = bounded::output(
            Command::new(&self.loginctl).args(["show-user", &self.uid.to_string(), "-p", "Linger"]),
            deadline,
        );
        let output = match result {
            Ok((status, output)) if status.success() => output,
            Ok(_) | Err(bounded::Error::Spawn(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(match String::from_utf8_lossy(&output).trim() {
            "Linger=yes" => Some(true),
            "Linger=no" => Some(false),
            _ => None,
        })
    }

    fn enable_until(&self, deadline: Instant) -> io::Result<bool> {
        match bounded::status(
            Command::new(&self.loginctl).args([
                "--no-ask-password",
                "enable-linger",
                &self.uid.to_string(),
            ]),
            deadline,
        ) {
            Ok(status) if status.code().is_some() => Ok(status.success()),
            Err(bounded::Error::Spawn(_)) => Ok(false),
            Err(error) => Err(error.into()),
            Ok(_) => Err(io::Error::other(
                "Lingering command ended without confirmation",
            )),
        }
    }

    /// Lingering off, and the marker gone, when Newport turned it on and no
    /// stable display or held terminal runs. `ending` is a scope whose last
    /// process is the caller, which is ending.
    pub(crate) fn reconcile_with(&self, ending: Option<&str>) {
        let Some(state) = &self.state else {
            return;
        };
        let Some(_lock) = self.lock() else {
            return;
        };
        let marker = state.join(MARKER);
        if !marker.exists() {
            return;
        }
        let Some(running) = self.scopes() else {
            return;
        };
        if running.iter().any(|scope| Some(scope.as_str()) != ending) {
            return;
        }
        if self.loginctl("disable-linger") {
            let _ = fs::remove_file(marker);
        }
    }

    /// Whether the account lingers; `None` without logind.
    fn lingers(&self) -> Option<bool> {
        let output = Command::new(&self.loginctl)
            .args(["show-user", &self.uid.to_string(), "-p", "Linger"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        match String::from_utf8_lossy(&output.stdout).trim() {
            "Linger=yes" => Some(true),
            "Linger=no" => Some(false),
            _ => None,
        }
    }

    fn loginctl(&self, verb: &str) -> bool {
        Command::new(&self.loginctl)
            .args(["--no-ask-password", verb, &self.uid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// The stable display's and held terminals' scopes that are active;
    /// `None` when the user manager can't say.
    fn scopes(&self) -> Option<Vec<String>> {
        let output = Command::new(&self.systemctl)
            .args([
                "--user",
                "list-units",
                "--type=scope",
                "--state=active",
                "--plain",
                "--no-legend",
                "--no-pager",
                DISPLAY_SCOPE,
                TERMINAL_SCOPES,
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .map(str::to_owned)
                .collect(),
        )
    }
}

/// Turns lingering off when Newport turned it on and no stable display or
/// held terminal runs. Call it when the agent starts, and when a display or
/// held terminal ends, from its own process.
pub fn reconcile() {
    Linger::system().reconcile_with(ending_scope().as_deref());
}

/// This process's scope, when it is the last process in it.
fn ending_scope() -> Option<String> {
    let cgroup = fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let scope = path.rsplit('/').next()?.to_owned();
    if !scope.ends_with(".scope") {
        return None;
    }
    let procs = fs::read_to_string(
        PathBuf::from("/sys/fs/cgroup")
            .join(path.trim_start_matches('/'))
            .join("cgroup.procs"),
    )
    .ok()?;
    let own = std::process::id().to_string();
    procs.lines().all(|pid| pid.trim() == own).then_some(scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_a_lock_does_not_wait_for_an_inherited_file_description() {
        let directory = tempfile::tempdir().unwrap();
        let linger = Linger {
            loginctl: OsString::new(),
            systemctl: OsString::new(),
            state: Some(directory.path().to_path_buf()),
            uid: 1000,
        };
        let lock = linger
            .lock_until(Instant::now() + Duration::from_secs(1))
            .unwrap()
            .unwrap();
        // A duplicate retains the open file description exactly as fork does.
        let inherited = lock.0.try_clone().unwrap();
        let next = File::options()
            .write(true)
            .open(directory.path().join(LOCK))
            .unwrap();
        drop(lock);
        let acquired = fs2::FileExt::try_lock_exclusive(&next);
        drop(inherited);
        acquired.expect("a descendant retained the released linger lock");
    }
}
