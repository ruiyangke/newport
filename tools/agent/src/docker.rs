//! The Docker Engine API over its unix socket: discovery, version
//! negotiation, a small HTTP/1.1 client (one connection per request,
//! `Connection: close`, content-length or chunked bodies), the log stream
//! format and RFC 3339 timestamps. Podman's Docker-compatible socket works too.
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_HEAD: usize = 64 << 10;
pub const MAX_BODY: usize = 64 << 20;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// The oldest API this agent speaks, and the one it prefers when the engine
/// offers more: newer engines drop deprecated fields from newer versions only.
const MIN_API: (u32, u32) = (1, 40);
const PREFERRED_API: (u32, u32) = (1, 47);

pub const NOT_RUNNING: &str = "Docker isn't installed or isn't running on this server.";

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    pub fn success(&self) -> bool {
        (200..300).contains(&self.status)
    }
    /// The engine's own words for a failed request.
    pub fn message(&self) -> String {
        let parsed = serde_json::from_slice::<Value>(&self.body).ok();
        if let Some(message) = parsed
            .as_ref()
            .and_then(|v| v["message"].as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            return message.to_owned();
        }
        let text = String::from_utf8_lossy(&self.body);
        let text = text.trim();
        if text.is_empty() || text.len() > 512 {
            format!("the engine answered HTTP {}", self.status)
        } else {
            text.to_owned()
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A chunked body: hex size lines (extensions ignored), data, CRLF, until a
/// zero-size chunk. Trailers are ignored.
pub fn decode_chunked(mut raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let end = find(raw, b"\r\n").ok_or("chunked response cut short")?;
        let line = std::str::from_utf8(&raw[..end]).map_err(|_| "bad chunk size")?;
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| format!("bad chunk size {line:?}"))?;
        raw = &raw[end + 2..];
        if size == 0 {
            return Ok(body);
        }
        if raw.len() < size + 2 {
            return Err("chunked response cut short".into());
        }
        if body.len() + size > MAX_BODY {
            return Err("response too large".into());
        }
        body.extend_from_slice(&raw[..size]);
        if &raw[size..size + 2] != b"\r\n" {
            return Err("malformed chunk".into());
        }
        raw = &raw[size + 2..];
    }
}

/// A complete HTTP/1.1 response as read from the socket.
pub fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let end = find(raw, b"\r\n\r\n").ok_or("response cut short")?;
    let head = String::from_utf8_lossy(&raw[..end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(format!("not an HTTP response: {status_line:?}"));
    }
    let status = parts
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("bad status line {status_line:?}"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let rest = &raw[end + 4..];
    let response = Response {
        status,
        headers,
        body: Vec::new(),
    };
    let chunked = response
        .header("transfer-encoding")
        .is_some_and(|te| te.to_ascii_lowercase().contains("chunked"));
    let body = if chunked {
        decode_chunked(rest)?
    } else if let Some(length) = response.header("content-length") {
        let length: usize = length.parse().map_err(|_| "bad content-length")?;
        if rest.len() < length {
            return Err("response cut short".into());
        }
        rest[..length].to_vec()
    } else if status == 204 || status == 304 {
        Vec::new()
    } else {
        rest.to_vec()
    };
    Ok(Response { body, ..response })
}

/// Whether `raw` already holds a whole response, so a server that keeps the
/// connection open despite `Connection: close` does not cost the deadline.
fn complete(raw: &[u8]) -> bool {
    let Some(end) = find(raw, b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
    if head.contains("transfer-encoding: chunked") {
        return raw.ends_with(b"\r\n\r\n") && parse_response(raw).is_ok();
    }
    if let Some(length) = head
        .split("\r\n")
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|l| l.trim().parse::<usize>().ok())
    {
        return raw.len() >= end + 4 + length;
    }
    head.starts_with("http/1.1 204") || head.starts_with("http/1.1 304")
}

/// One request on a fresh connection, bounded by `timeout` overall.
pub fn request(
    socket: &Path,
    method: &str,
    target: &str,
    timeout: Duration,
) -> io::Result<Response> {
    let deadline = Instant::now() + timeout;
    let mut stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: docker\r\nUser-Agent: newport-agent\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )?;
    stream.flush()?;
    let mut raw = Vec::new();
    let mut buffer = vec![0; 64 << 10];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }
        stream.set_read_timeout(Some(left))?;
        let read = match stream.read(&mut buffer) {
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"))
            }
            Err(e) => return Err(e),
        };
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
        if raw.len() > MAX_BODY + MAX_HEAD {
            return Err(io::Error::other("response too large"));
        }
        if complete(&raw) {
            break;
        }
    }
    parse_response(&raw).map_err(io::Error::other)
}

/// Where an engine socket may be, in the order they are tried.
pub fn candidates(docker_host: Option<&str>, runtime: Option<&str>, uid: u32) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = docker_host
        .and_then(|h| h.strip_prefix("unix://"))
        .filter(|p| p.starts_with('/'))
    {
        paths.push(PathBuf::from(path));
    }
    paths.push("/var/run/docker.sock".into());
    let user_runtime = format!("/run/user/{uid}");
    let runtime = runtime.filter(|r| r.starts_with('/'));
    if let Some(runtime) = runtime {
        paths.push(Path::new(runtime).join("docker.sock"));
    }
    paths.push(Path::new(&user_runtime).join("docker.sock"));
    paths.push("/run/podman/podman.sock".into());
    if let Some(runtime) = runtime {
        paths.push(Path::new(runtime).join("podman/podman.sock"));
    }
    // A non-login SSH command may not have XDG_RUNTIME_DIR exported.
    paths.push(Path::new(&user_runtime).join("podman/podman.sock"));
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
    paths
}

/// What the person sees when the socket cannot be used.
pub fn connect_error(socket: &Path, error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::PermissionDenied => format!(
            "This account can't use Docker: permission denied on {}. Add it to the docker group, or use rootless Docker.",
            socket.display()
        ),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => NOT_RUNNING.into(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            "Docker didn't answer: it did not reply in time.".into()
        }
        _ => format!("Docker didn't answer: {error}"),
    }
}

fn api_version(text: &str) -> Option<(u32, u32)> {
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The API version to use: the preferred one, within what the engine offers.
pub fn negotiate(engine_max: &str, engine_min: Option<&str>) -> Result<(u32, u32), String> {
    let max = api_version(engine_max)
        .ok_or_else(|| format!("Docker didn't answer: unknown API version {engine_max:?}"))?;
    if max < MIN_API {
        return Err(format!(
            "Docker didn't answer: its API v{}.{} is older than v{}.{}, the oldest Newport supports.",
            max.0, max.1, MIN_API.0, MIN_API.1
        ));
    }
    let min = engine_min.and_then(api_version).unwrap_or(MIN_API);
    Ok(PREFERRED_API.min(max).max(min))
}

/// A reachable engine and the versioned API prefix to use with it.
#[derive(Clone, Debug)]
pub struct Engine {
    pub socket: PathBuf,
    prefix: String,
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub rootless: bool,
    pub podman: bool,
    /// The host's memory: a container "limited" to all of it is unlimited.
    pub mem_total: u64,
}

impl Engine {
    /// The first candidate socket that answers. A socket this account may
    /// not open, or a stale one, does not hide a working one further down.
    pub fn discover(candidates: &[PathBuf]) -> Result<Self, String> {
        let mut first_error = None;
        for socket in candidates.iter().filter(|p| p.exists()) {
            match Self::connect(socket) {
                Ok(engine) => return Ok(engine),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(first_error.unwrap_or_else(|| NOT_RUNNING.into()))
    }

    fn connect(socket: &Path) -> Result<Self, String> {
        let version = request(socket, "GET", "/version", PROBE_TIMEOUT)
            .map_err(|e| connect_error(socket, &e))?;
        if !version.success() {
            return Err(format!("Docker didn't answer: {}", version.message()));
        }
        let version: Value = serde_json::from_slice(&version.body)
            .map_err(|e| format!("Docker didn't answer: unreadable version: {e}"))?;
        let api = negotiate(
            version["ApiVersion"].as_str().unwrap_or_default(),
            version["MinAPIVersion"].as_str(),
        )?;
        let prefix = format!("/v{}.{}", api.0, api.1);
        let info = request(socket, "GET", &format!("{prefix}/info"), PROBE_TIMEOUT)
            .ok()
            .filter(Response::success)
            .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .unwrap_or(Value::Null);
        let podman = version["Components"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["Name"].as_str().is_some_and(|n| n.contains("Podman")))
            || version["Platform"]["Name"]
                .as_str()
                .is_some_and(|n| n.contains("Podman"));
        let rootless = info["SecurityOptions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|o| o.as_str().is_some_and(|o| o.contains("name=rootless")));
        let os = info["OperatingSystem"]
            .as_str()
            .or_else(|| version["Os"].as_str())
            .unwrap_or_default()
            .to_owned();
        Ok(Self {
            socket: socket.to_owned(),
            prefix,
            version: version["Version"].as_str().unwrap_or_default().to_owned(),
            api_version: format!("{}.{}", api.0, api.1),
            os,
            rootless,
            podman,
            mem_total: info["MemTotal"].as_u64().unwrap_or(0),
        })
    }

    pub fn call(&self, method: &str, path: &str, timeout: Duration) -> Result<Response, String> {
        request(
            &self.socket,
            method,
            &format!("{}{path}", self.prefix),
            timeout,
        )
        .map_err(|e| {
            if e.kind() == io::ErrorKind::TimedOut {
                format!(
                    "Docker didn't answer: no reply within {} seconds.",
                    timeout.as_secs().max(1)
                )
            } else {
                connect_error(&self.socket, &e)
            }
        })
    }

    /// A JSON GET; engine errors come back in the engine's words.
    pub fn get(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        let response = self.call("GET", path, timeout)?;
        if !response.success() {
            return Err(response.message());
        }
        serde_json::from_slice(&response.body)
            .map_err(|e| format!("Docker didn't answer: unreadable reply: {e}"))
    }
}

/// Query-string encoding for filter JSON and other values.
pub fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// An RFC 3339 timestamp (Docker prints nanoseconds) as nanoseconds since the
/// epoch. Docker's zero time, 0001-01-01, is out of range and comes out None.
pub fn parse_time(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (hour, minute, second) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &text[19..];
    let mut nanos: i64 = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let count = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if count == 0 {
            return None;
        }
        let kept = &fraction[..count.min(9)];
        nanos = kept.parse::<i64>().ok()? * 10i64.pow(9 - kept.len() as u32);
        rest = &fraction[count..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        zone if zone.len() == 6 && (zone.starts_with('+') || zone.starts_with('-')) => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            if zone.as_bytes()[3] != b':' {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
        _ => return None,
    };
    let days = crate::services::days_from_civil(year, month, day);
    let seconds = days * 86400 + hour * 3600 + minute * 60 + second - offset;
    seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
}

/// A timestamp as epoch milliseconds; Docker's zero time is "never".
pub fn time_ms(text: Option<&str>) -> Option<i64> {
    text.and_then(parse_time)
        .filter(|ns| *ns > 0)
        .map(|ns| ns / 1_000_000)
}

/// One log line from the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Nanoseconds since the epoch, when the line carried a timestamp.
    pub at: Option<i64>,
    pub stderr: bool,
    pub message: String,
}

/// Messages beyond this are cut, with an ellipsis.
pub const MAX_MESSAGE: usize = 16 << 10;

fn line(raw: &[u8], stderr: bool) -> Line {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    let text = String::from_utf8_lossy(raw);
    let (at, message) = match text.split_once(' ') {
        Some((stamp, rest)) => match parse_time(stamp) {
            Some(at) => (Some(at), rest),
            None => (None, text.as_ref()),
        },
        None => match parse_time(&text) {
            Some(at) => (Some(at), ""),
            None => (None, text.as_ref()),
        },
    };
    let mut message = message.to_owned();
    if message.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push('…');
    }
    Line {
        at,
        stderr,
        message,
    }
}

fn lines(payload: &[u8], stderr: bool, out: &mut Vec<Line>) {
    let payload = payload.strip_suffix(b"\n").unwrap_or(payload);
    if payload.is_empty() {
        return;
    }
    out.extend(payload.split(|b| *b == b'\n').map(|l| line(l, stderr)));
}

/// Whether a logs body uses the 8-byte frame headers of a non-TTY container.
/// With timestamps on, a raw (TTY) body starts with a digit, never 0..=2.
pub fn multiplexed(content_type: Option<&str>, body: &[u8]) -> bool {
    if content_type.is_some_and(|t| t.contains("multiplexed-stream")) {
        return true;
    }
    body.len() >= 8 && body[0] <= 2 && body[1..4] == [0, 0, 0]
}

/// Log lines from a logs body: frames (stream byte, three zero bytes, a
/// big-endian length, then the payload) or raw TTY output. A frame cut short
/// at the end keeps what arrived; a header cut short is dropped.
pub fn demux(body: &[u8], frames: bool) -> Vec<Line> {
    let mut out = Vec::new();
    if !frames {
        lines(body, false, &mut out);
        return out;
    }
    let mut rest = body;
    while rest.len() >= 8 {
        let stderr = rest[0] == 2;
        let size = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        let end = (8 + size).min(rest.len());
        lines(&rest[8..end], stderr, &mut out);
        rest = &rest[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_with_length_chunks_or_neither() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}trailing";
        let response = parse_response(raw).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert_eq!(response.body, b"{\"ok\":true}");
        assert!(complete(raw));
        assert!(!complete(&raw[..raw.len() - 20]));
        assert!(parse_response(&raw[..raw.len() - 20]).is_err());

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=1\r\nWiki\r\n6\r\npedia \r\nE\r\nin \r\n\r\nchunks.\r\n0\r\nX-Trailer: 1\r\n\r\n";
        let response = parse_response(chunked).unwrap();
        assert_eq!(response.body, b"Wikipedia in \r\n\r\nchunks.");
        assert!(complete(chunked));
        // Cut anywhere before the last chunk, the body is incomplete.
        for cut in [50, 60, 70] {
            assert!(parse_response(&chunked[..cut]).is_err(), "{cut}");
            assert!(!complete(&chunked[..cut]), "{cut}");
        }
        // Chunk data that merely looks like the terminator is not the end.
        let tricky = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n0\r\n\r\n";
        assert!(!complete(tricky));
        assert!(decode_chunked(b"zz\r\n").is_err());
        assert!(decode_chunked(b"3\r\nabcX\r\n0\r\n\r\n").is_err());

        let empty = parse_response(b"HTTP/1.1 204 No Content\r\nServer: Docker\r\n\r\n").unwrap();
        assert_eq!((empty.status, empty.body.len()), (204, 0));
        let until_close = parse_response(b"HTTP/1.0 200 OK\r\n\r\nabc").unwrap();
        assert_eq!(until_close.body, b"abc");
        assert!(parse_response(b"SSH-2.0-OpenSSH\r\n\r\n").is_err());
    }

    #[test]
    fn engine_messages_prefer_the_json_message() {
        let failed = parse_response(
            b"HTTP/1.1 409 Conflict\r\nContent-Length: 49\r\n\r\n{\"message\":\"Container abc is not running\"}\n      ",
        )
        .unwrap();
        assert_eq!(failed.message(), "Container abc is not running");
        let plain =
            parse_response(b"HTTP/1.1 500 Oops\r\nContent-Length: 5\r\n\r\nboom\n").unwrap();
        assert_eq!(plain.message(), "boom");
        let bare = parse_response(b"HTTP/1.1 502 Bad\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!(bare.message(), "the engine answered HTTP 502");
    }

    #[test]
    fn sockets_are_tried_in_the_documented_order() {
        let paths = candidates(
            Some("unix:///srv/docker.sock"),
            Some("/run/user/1000"),
            1000,
        );
        let paths: Vec<_> = paths.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(
            paths,
            [
                "/srv/docker.sock",
                "/var/run/docker.sock",
                "/run/user/1000/docker.sock",
                "/run/podman/podman.sock",
                "/run/user/1000/podman/podman.sock",
            ]
        );
        // tcp:// and ssh:// hosts are not sockets; the runtime directory
        // falls back to /run/user/<uid>.
        let paths = candidates(Some("tcp://10.0.0.1:2375"), None, 1001);
        let paths: Vec<_> = paths.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(
            paths,
            [
                "/var/run/docker.sock",
                "/run/user/1001/docker.sock",
                "/run/podman/podman.sock",
                "/run/user/1001/podman/podman.sock",
            ]
        );
        let custom = candidates(None, Some("/tmp/xdg"), 7);
        assert_eq!(custom[1], Path::new("/tmp/xdg/docker.sock"));
        assert_eq!(custom[2], Path::new("/run/user/7/docker.sock"));
        assert_eq!(custom[4], Path::new("/tmp/xdg/podman/podman.sock"));
    }

    #[test]
    fn unusable_sockets_explain_themselves() {
        let path = Path::new("/var/run/docker.sock");
        assert_eq!(
            connect_error(path, &io::Error::from(io::ErrorKind::PermissionDenied)),
            "This account can't use Docker: permission denied on /var/run/docker.sock. Add it to the docker group, or use rootless Docker."
        );
        assert_eq!(
            connect_error(path, &io::Error::from(io::ErrorKind::ConnectionRefused)),
            NOT_RUNNING
        );
        assert!(
            connect_error(path, &io::Error::other("reset")).starts_with("Docker didn't answer: ")
        );
        assert_eq!(Engine::discover(&[]).unwrap_err(), NOT_RUNNING);
        assert_eq!(
            Engine::discover(&["/nonexistent/docker.sock".into()]).unwrap_err(),
            NOT_RUNNING
        );
    }

    #[test]
    fn api_versions_are_negotiated_within_the_engine_range() {
        assert_eq!(negotiate("1.54", Some("1.44")), Ok((1, 47)));
        assert_eq!(negotiate("1.41", Some("1.24")), Ok((1, 41)));
        assert_eq!(negotiate("1.60", Some("1.50")), Ok((1, 50)));
        assert_eq!(negotiate("1.40", None), Ok((1, 40)));
        assert!(negotiate("1.39", None)
            .unwrap_err()
            .starts_with("Docker didn't answer: "));
        assert!(negotiate("x", None).is_err());
    }

    #[test]
    fn timestamps_with_nanoseconds_and_offsets() {
        assert_eq!(
            parse_time("2026-10-04T10:21:05.123456789Z"),
            Some(1_791_109_265_123_456_789)
        );
        assert_eq!(
            parse_time("2026-10-04T10:21:05Z"),
            Some(1_791_109_265_000_000_000)
        );
        assert_eq!(
            parse_time("2026-10-04T12:21:05.5+02:00"),
            Some(1_791_109_265_500_000_000)
        );
        assert_eq!(parse_time("0001-01-01T00:00:00Z"), None);
        assert!(parse_time("1960-01-01T00:00:00Z").unwrap() < 0);
        assert_eq!(time_ms(Some("0001-01-01T00:00:00Z")), None);
        assert_eq!(
            time_ms(Some("2026-10-04T10:21:05.123456789Z")),
            Some(1_791_109_265_123)
        );
        for bad in [
            "",
            "2026-10-04",
            "2026-10-04T10:21:05",
            "2026-13-04T10:21:05Z",
            "hello world, this is long",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    fn frame(stream: u8, payload: &str) -> Vec<u8> {
        let mut out = vec![stream, 0, 0, 0];
        out.extend((payload.len() as u32).to_be_bytes());
        out.extend(payload.as_bytes());
        out
    }

    #[test]
    fn multiplexed_frames_are_split_by_stream() {
        let mut body = frame(1, "2026-10-04T10:21:05.000000001Z listening on :8080\n");
        body.extend(frame(2, "2026-10-04T10:21:05.000000002Z warning: slow\r\n"));
        body.extend(frame(1, "2026-10-04T10:21:06Z \n"));
        assert!(multiplexed(None, &body));
        let lines = demux(&body, true);
        assert_eq!(
            lines,
            [
                Line {
                    at: Some(1_791_109_265_000_000_001),
                    stderr: false,
                    message: "listening on :8080".into()
                },
                Line {
                    at: Some(1_791_109_265_000_000_002),
                    stderr: true,
                    message: "warning: slow".into()
                },
                Line {
                    at: Some(1_791_109_266_000_000_000),
                    stderr: false,
                    message: String::new()
                },
            ]
        );
        // A frame cut short keeps what arrived; a header cut short is dropped.
        let mut cut = frame(1, "2026-10-04T10:21:05Z whole\n");
        cut.extend(&frame(2, "2026-10-04T10:21:06Z partial line")[..32]);
        let lines = demux(&cut, true);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].message, "par");
        assert!(lines[1].stderr);
        let mut header_cut = frame(1, "2026-10-04T10:21:05Z whole\n");
        header_cut.extend([1, 0, 0, 0, 0]);
        assert_eq!(demux(&header_cut, true).len(), 1);
    }

    #[test]
    fn tty_output_is_raw_and_untimestamped_lines_survive() {
        let body = b"2026-10-04T10:21:05Z $ ls\r\n2026-10-04T10:21:05.5Z a  b\r\nno stamp here\n";
        assert!(!multiplexed(
            Some("application/vnd.docker.raw-stream"),
            body
        ));
        assert!(multiplexed(
            Some("application/vnd.docker.multiplexed-stream"),
            b""
        ));
        let lines = demux(body, false);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].message, "$ ls");
        assert_eq!(lines[1].at, Some(1_791_109_265_500_000_000));
        assert_eq!(lines[1].message, "a  b");
        assert_eq!(lines[2].at, None);
        assert_eq!(lines[2].message, "no stamp here");
        assert!(lines.iter().all(|l| !l.stderr));
        assert!(demux(b"", false).is_empty());
    }

    #[test]
    fn long_messages_are_cut_on_a_character_boundary() {
        let long = format!("2026-10-04T10:21:05Z {}\n", "é".repeat(MAX_MESSAGE));
        let lines = demux(long.as_bytes(), false);
        assert!(lines[0].message.ends_with('…'));
        assert!(lines[0].message.len() <= MAX_MESSAGE + '…'.len_utf8());
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(
            encode(r#"{"label":["a=b"]}"#),
            "%7B%22label%22%3A%5B%22a%3Db%22%5D%7D"
        );
        assert_eq!(encode("safe-_.~1"), "safe-_.~1");
    }
}
