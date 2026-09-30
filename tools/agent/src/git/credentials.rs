//! HTTPS credential helpers configured by the repository owner. Git transfers
//! stay in libgit2; only explicitly configured credential programs are executed.
use git2::{Config, Cred, Error, ErrorClass, ErrorCode, Repository};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{io::AsRawFd, process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn unavailable() -> Error {
    Error::new(
        ErrorCode::Auth,
        ErrorClass::Net,
        "Configured HTTPS credentials are unavailable",
    )
}

struct Settings {
    helpers: Vec<String>,
    username: Option<String>,
    path: bool,
}

// Match HTTPS credential sections to their origin and optional path prefix.
// Never send credentials selected for another port, host, or path component.
fn matches(scope: &str, target: &url::Url, username: Option<&str>) -> bool {
    let Ok(scope) = url::Url::parse(scope) else {
        return false;
    };
    scope.scheme() == target.scheme()
        && scope.host_str() == target.host_str()
        && scope.port_or_known_default() == target.port_or_known_default()
        && (scope.username().is_empty() || decode(scope.username()).ok().as_deref() == username)
        && (scope.path() == "/"
            || target.path() == scope.path()
            || target
                .path()
                .strip_prefix(scope.path().trim_end_matches('/'))
                .is_some_and(|rest| rest.starts_with('/')))
}

fn settings(config: &Config, url: &url::Url, username: Option<&str>) -> Result<Settings, Error> {
    let mut result = Settings {
        helpers: Vec::new(),
        username: username.map(str::to_owned),
        path: false,
    };
    let mut entries = config.entries(None)?;
    while let Some(entry) = entries.next() {
        let entry = entry?;
        let Some(name) = entry
            .name()
            .ok()
            .and_then(|name| name.strip_prefix("credential."))
        else {
            continue;
        };
        let field = if let Some((scope, field)) = name.rsplit_once('.') {
            if !matches(scope, url, username) {
                continue;
            }
            field
        } else {
            name
        };
        if !["helper", "username", "usehttppath"].contains(&field) {
            continue;
        }
        let value = entry.value()?;
        match field {
            "helper" if value.is_empty() => result.helpers.clear(),
            "helper" => result.helpers.push(value.to_owned()),
            "username" if username.is_none() => result.username = Some(value.to_owned()),
            "usehttppath" => result.path = git2::Config::parse_bool(value)?,
            _ => {}
        }
    }
    Ok(result)
}

/// A process group prevents a helper's shell descendants from outliving a
/// timeout. Neither helper output nor command strings enter logs or RPC errors.
struct Helper {
    child: std::process::Child,
    reaped: bool,
}
impl Drop for Helper {
    fn drop(&mut self) {
        // The child is created as leader of a new process group below.
        if !self.reaped {
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}
fn execute(
    repo: &Repository,
    helper: &str,
    input: &[u8],
    deadline: Instant,
) -> Result<Vec<u8>, Error> {
    if helper.len() > 8192 || Instant::now() >= deadline {
        return Err(unavailable());
    }
    let command = if let Some(shell) = helper.strip_prefix('!') {
        format!("{shell} get")
    } else if helper.starts_with('/') {
        format!("{helper} get")
    } else {
        // Git's documented helper naming convention, including helper options.
        format!("git credential-{helper} get")
    };
    let mut input_file = tempfile::tempfile().map_err(|_| unavailable())?;
    input_file.write_all(input).map_err(|_| unavailable())?;
    input_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| unavailable())?;
    let child = Command::new("sh")
        .args(["-c", &command])
        .current_dir(repo.workdir().unwrap_or(repo.path()))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::from(input_file))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|_| unavailable())?;
    super::metrics::helper_started(!helper.starts_with('!') && !helper.starts_with('/'));
    let mut child = Helper {
        child,
        reaped: false,
    };
    let mut output = child.child.stdout.take().ok_or_else(unavailable)?;
    let fd = output.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        if Instant::now() >= deadline {
            return Err(unavailable());
        }
        match output.read(&mut buffer) {
            Ok(0) => {
                if let Some(status) = child.child.try_wait().map_err(|_| unavailable())? {
                    child.reaped = true;
                    return if status.success() {
                        Ok(bytes)
                    } else {
                        Err(unavailable())
                    };
                }
            }
            Ok(n) => {
                if bytes.len() + n > 64 * 1024 {
                    return Err(unavailable());
                }
                bytes.extend_from_slice(&buffer[..n]);
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(unavailable()),
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn https(
    repo: &Repository,
    address: &str,
    username: Option<&str>,
    deadline: Instant,
) -> Result<Cred, Error> {
    let (username, password) = resolve(repo, address, username, deadline)?;
    Cred::userpass_plaintext(&username, &password)
}

// Decode URL attributes before entering Git's line-oriented helper protocol.
fn decode(value: &str) -> Result<String, Error> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let high = input
                .next()
                .and_then(|c| (c as char).to_digit(16))
                .ok_or_else(unavailable)?;
            let low = input
                .next()
                .and_then(|c| (c as char).to_digit(16))
                .ok_or_else(unavailable)?;
            ((high << 4) | low) as u8
        } else {
            byte
        });
    }
    let value = String::from_utf8(bytes).map_err(|_| unavailable())?;
    if value.contains(['\n', '\r', '\0']) {
        return Err(unavailable());
    }
    Ok(value)
}

fn resolve(
    repo: &Repository,
    address: &str,
    username: Option<&str>,
    deadline: Instant,
) -> Result<(String, String), Error> {
    let url = url::Url::parse(address).map_err(|_| unavailable())?;
    if url.scheme() != "https" {
        return Err(unavailable());
    }
    let url_username = decode(url.username())?;
    let username = username.or_else(|| {
        (!url_username.is_empty() || url.password().is_some()).then_some(url_username.as_str())
    });
    // A complete URL credential wins over helpers, matching Git. Avoid a
    // helper process and avoid passing a configured secret to unrelated helpers.
    if let (Some(username), Some(password)) = (username, url.password()) {
        return Ok((username.to_owned(), decode(password)?));
    }
    let config = repo.config()?.snapshot()?;
    let settings = settings(&config, &url, username)?;
    let mut username = settings.username;
    let mut password: Option<String> = None;
    let deadline = deadline.min(Instant::now() + Duration::from_secs(10));
    for helper in settings.helpers {
        let mut input = format!(
            "protocol=https\nhost={}{}\n",
            url.host_str().ok_or_else(unavailable)?,
            url.port().map(|p| format!(":{p}")).unwrap_or_default()
        );
        if settings.path {
            input.push_str(&format!(
                "path={}\n",
                decode(url.path().trim_start_matches('/'))?
            ));
        }
        if let Some(name) = &username {
            if name.contains(['\n', '\r', '\0']) {
                return Err(unavailable());
            }
            input.push_str(&format!("username={name}\n"));
        }
        if let Some(secret) = &password {
            if secret.contains(['\n', '\r', '\0']) {
                return Err(unavailable());
            }
            input.push_str(&format!("password={secret}\n"));
        }
        input.push('\n');
        let output = match execute(repo, &helper, input.as_bytes(), deadline) {
            Ok(output) => output,
            Err(_) if Instant::now() < deadline => continue,
            Err(e) => return Err(e),
        };
        let output = std::str::from_utf8(&output).map_err(|_| unavailable())?;
        for line in output.lines().take_while(|line| !line.is_empty()) {
            if let Some(value) = line.strip_prefix("username=") {
                username = Some(value.into());
            }
            if let Some(value) = line.strip_prefix("password=") {
                password = Some(value.to_owned());
            }
            if line == "quit=true" || line == "quit=1" {
                return Err(unavailable());
            }
        }
        if let (Some(name), Some(secret)) = (&username, &password) {
            return Ok((name.clone(), secret.clone()));
        }
    }
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(config: &str) -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        // An empty helper clears any inherited user configuration.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(repo.path().join("config"))
            .unwrap();
        writeln!(file, "\n[credential]\nhelper =\n{config}").unwrap();
        (dir, repo)
    }

    fn lookup(repo: &Repository, url: &str) -> Result<(String, String), Error> {
        resolve(repo, url, None, Instant::now() + Duration::from_secs(2))
    }

    #[test]
    fn complete_url_credentials_precede_helpers_and_are_decoded() {
        let (dir, repo) = fixture(r#"helper = "!touch helper-was-called; exit 1""#);
        assert_eq!(
            lookup(&repo, "https://alice%40team:to%3Aken%2B@example.test/repo").unwrap(),
            ("alice@team".into(), "to:ken+".into())
        );
        assert_eq!(
            lookup(&repo, "https://:token@example.test/repo").unwrap(),
            (String::new(), "token".into())
        );
        assert!(!dir.path().join("helper-was-called").exists());
        assert!(lookup(&repo, "https://alice:bad%0Apassword@example.test/repo").is_err());
    }

    #[test]
    fn repository_config_resets_inherited_helpers_and_reads_includes() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        let local = dir.path().join("local");
        let include = dir.path().join("included");
        std::fs::write(
            &global,
            "[credential]\nhelper=global\nusername=global-user\n",
        )
        .unwrap();
        std::fs::write(&include, "[credential]\nhelper=second\n").unwrap();
        std::fs::write(
            &local,
            format!(
                "[credential]\nhelper=\nhelper=local\nusername=local-user\n[include]\npath={}\n",
                include.display()
            ),
        )
        .unwrap();
        let mut config = Config::new().unwrap();
        config
            .add_file(&global, git2::ConfigLevel::Global, false)
            .unwrap();
        config
            .add_file(&local, git2::ConfigLevel::Local, false)
            .unwrap();
        let settings = settings(
            &config,
            &url::Url::parse("https://example.test/repo").unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(settings.helpers, ["local", "second"]);
        assert_eq!(settings.username.as_deref(), Some("local-user"));
    }

    #[test]
    fn helper_chain_collects_partial_credentials_in_repository_directory() {
        let (_dir, repo) = fixture(
            r#"
helper = "!f() { test -d .git || exit 1; printf 'username=alice\\n'; }; f"
helper = "!f() { input=$(cat); case \"$input\" in *username=alice*) printf 'password=token\\n';; esac; }; f"
"#,
        );
        assert_eq!(
            lookup(&repo, "https://example.test/repo").unwrap(),
            ("alice".into(), "token".into())
        );
    }

    #[test]
    fn local_helper_reset_and_url_scoping() {
        let (_dir, repo) = fixture(
            r#"
helper = "!f() { printf 'username=wrong\\npassword=wrong\\n'; }; f"
[credential "https://example.test/team"]
helper =
username = alice
helper = "!f() { printf 'password=right\\n'; }; f"
"#,
        );
        assert_eq!(
            lookup(&repo, "https://example.test/team/repo").unwrap(),
            ("alice".into(), "right".into())
        );
        for url in [
            "https://other.test/team/repo",
            "https://example.test:8443/team/repo",
            "https://example.test/teammate",
        ] {
            assert_eq!(
                lookup(&repo, url).unwrap(),
                ("wrong".into(), "wrong".into())
            );
        }
    }

    #[test]
    fn username_in_url_and_use_http_path_reach_helper_decoded() {
        let (_dir, repo) = fixture(
            r#"
username = fallback
useHttpPath = true
helper = "!f() { input=$(cat); case \"$input\" in *path=team/my\\ repo*username=alice@example.test*) printf 'password=token\\n';; esac; }; f"
"#,
        );
        assert_eq!(
            lookup(
                &repo,
                "https://alice%40example.test@example.test/team/my%20repo"
            )
            .unwrap(),
            ("alice@example.test".into(), "token".into())
        );
        assert!(lookup(&repo, "https://example.test/team/%0Apassword=bad").is_err());
    }

    #[test]
    fn path_is_omitted_by_default() {
        let (_dir, repo) = fixture(
            r#"
helper = "!f() { input=$(cat); case \"$input\" in *path=*) exit 1;; esac; printf 'username=alice\\npassword=token\\n'; }; f"
"#,
        );
        assert!(lookup(&repo, "https://example.test/team/repo").is_ok());
    }

    #[test]
    fn helper_quit_stops_chain_and_failures_do_not_leak_output() {
        let (_dir, repo) = fixture(
            r#"
helper = "!f() { printf 'quit=true\\npassword=secret\\n'; }; f"
helper = "!f() { printf 'username=alice\\npassword=token\\n'; }; f"
"#,
        );
        let error = lookup(&repo, "https://example.test/repo").unwrap_err();
        assert_eq!(error.code(), ErrorCode::Auth);
        assert!(!error.message().contains("secret"));
    }

    #[test]
    fn failed_helper_falls_through() {
        let (_dir, repo) = fixture(
            r#"
helper = "!f() { printf secret >&2; exit 1; }; f"
helper = "!f() { printf 'username=alice\\npassword=token\\n'; }; f"
"#,
        );
        assert!(lookup(&repo, "https://example.test/repo").is_ok());
    }

    #[test]
    fn helpers_have_time_and_output_limits() {
        let (_dir, repo) = fixture("");
        let start = Instant::now();
        assert!(execute(
            &repo,
            "!sleep 10;",
            b"\n",
            start + Duration::from_millis(100)
        )
        .is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(execute(&repo, "!f() { while :; do printf '0123456789012345678901234567890123456789012345678901234567890123456789'; done; }; f", b"\n", Instant::now() + Duration::from_secs(2)).is_err());
    }
}
