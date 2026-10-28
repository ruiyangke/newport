//! Authenticated HTTPS Git endpoint for the agent's `git_https` contract test: a self-signed
//! TLS server on 127.0.0.1 that serves `git http-backend` as CGI behind Basic auth.
use crate::util::{capture, command, root, run as run_command};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header,
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    process::Command,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{self, pki_types::PrivateKeyDer},
};

/// Isolates every git process from the machine's system and global configuration.
const GIT_ENV: [(&str, &str); 3] = [
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_TERMINAL_PROMPT", "0"),
];
const CREDENTIALS: &str = "fixture:fixture-token";
const REQUEST_LIMIT: usize = 8 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const BACKEND_TIMEOUT: Duration = Duration::from_secs(10);
const TEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Cargo arguments that select the test target, then the test binary's own arguments.
const TEST_TARGET: [&str; 6] = [
    "test",
    "--manifest-path",
    "tools/agent/Cargo.toml",
    "--locked",
    "--test",
    "git_https",
];
const TEST_BINARY_ARGS: [&str; 3] = ["--", "--ignored", "--nocapture"];

/// Shared server state: the repository root, the backend and the auth counters.
struct Fixture {
    project_root: PathBuf,
    backend: PathBuf,
    authorization: String,
    authenticated: AtomicUsize,
    denied: AtomicUsize,
}

pub fn run() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("newport-https-")
        .tempdir()?;
    let cert = temp.path().join("cert.pem");
    let tls = tls_config(&cert)?;
    let repo = temp.path().join("repo.git");
    run_command(
        command("git")
            .envs(GIT_ENV)
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&repo),
    )?;
    run_command(command("git").envs(GIT_ENV).arg("-C").arg(&repo).args([
        "config",
        "http.receivepack",
        "true",
    ]))?;
    let backend =
        PathBuf::from(capture(command("git").arg("--exec-path"))?).join("git-http-backend");
    // Build first so the timed run measures the test, not compilation.
    run_command(command("cargo").args(TEST_TARGET).arg("--no-run"))?;
    let fixture = Arc::new(Fixture {
        project_root: temp.path().to_owned(),
        backend,
        authorization: format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(CREDENTIALS)
        ),
        authenticated: AtomicUsize::new(0),
        denied: AtomicUsize::new(0),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(serve(listener, TlsAcceptor::from(tls), fixture.clone()));
        let mut test = Command::from(command("cargo"))
            .args(TEST_TARGET)
            .args(TEST_BINARY_ARGS)
            .current_dir(root())
            .envs(GIT_ENV)
            .env(
                "NEWPORT_GIT_HTTPS_URL",
                format!("https://127.0.0.1:{port}/repo.git"),
            )
            .env("NEWPORT_GIT_HTTPS_CERT", &cert)
            .kill_on_drop(true)
            .spawn()
            .context("Cannot start cargo test")?;
        let status = tokio::time::timeout(TEST_TIMEOUT, test.wait())
            .await
            .context("HTTPS contract test exceeded 120 s")??;
        server.abort();
        ensure!(status.success(), "HTTPS contract test failed ({status})");
        Ok::<_, anyhow::Error>(())
    })?;
    runtime.shutdown_timeout(Duration::from_secs(1));
    let authenticated = fixture.authenticated.load(Ordering::SeqCst);
    let denied = fixture.denied.load(Ordering::SeqCst);
    ensure!(
        authenticated >= 2 && denied >= 1,
        "Authentication path was not exercised"
    );
    println!(
        "PASS: {authenticated} authenticated requests; repository TLS and credential configuration honored"
    );
    Ok(())
}

/// A self-signed certificate for IP 127.0.0.1, written as PEM to `cert` for the client to trust.
fn tls_config(cert: &Path) -> Result<Arc<rustls::ServerConfig>> {
    let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "localhost");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate()?;
    let certificate = params.self_signed(&key)?;
    std::fs::write(cert, certificate.pem())?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(
        vec![certificate.der().clone()],
        PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )?;
    Ok(Arc::new(config))
}

async fn serve(listener: TcpListener, tls: TlsAcceptor, fixture: Arc<Fixture>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (tls, fixture) = (tls.clone(), fixture.clone());
        tokio::spawn(async move {
            let Ok(stream) = tls.accept(stream).await else {
                return;
            };
            let service = service_fn(move |req| handle(req, fixture.clone()));
            let _ = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(REQUEST_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

fn empty(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::default());
    *response.status_mut() = status;
    response
}

async fn handle(
    req: Request<Incoming>,
    fixture: Arc<Fixture>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .is_some_and(|value| value.as_bytes() == fixture.authorization.as_bytes());
    if !authorized {
        fixture.denied.fetch_add(1, Ordering::SeqCst);
        let mut response = empty(StatusCode::UNAUTHORIZED);
        let headers = response.headers_mut();
        headers.insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Basic realm=\"fixture\""),
        );
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from_static("0"),
        );
        return Ok(response);
    }
    fixture.authenticated.fetch_add(1, Ordering::SeqCst);
    if !req.uri().path().starts_with("/repo.git/") {
        return Ok(empty(StatusCode::NOT_FOUND));
    }
    Ok(backend(req, &fixture).await.unwrap_or_else(|error| {
        eprintln!("{error:#}");
        empty(StatusCode::INTERNAL_SERVER_ERROR)
    }))
}

/// Run one request through `git http-backend` and translate its CGI response.
async fn backend(req: Request<Incoming>, fixture: &Fixture) -> Result<Response<Full<Bytes>>> {
    let (parts, body) = req.into_parts();
    let body = tokio::time::timeout(REQUEST_TIMEOUT, Limited::new(body, REQUEST_LIMIT).collect())
        .await
        .context("Fixture request timed out")?
        .map_err(|_| anyhow::anyhow!("Oversized fixture request"))?
        .to_bytes();
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let mut child = Command::new(&fixture.backend)
        .envs(GIT_ENV)
        .env("GIT_PROJECT_ROOT", &fixture.project_root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", parts.uri.path())
        .env("QUERY_STRING", parts.uri.query().unwrap_or_default())
        .env("REQUEST_METHOD", parts.method.as_str())
        .env("REMOTE_USER", "fixture")
        .env("CONTENT_TYPE", content_type)
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start git http-backend")?;
    let mut stdin = child.stdin.take().context("Backend stdin")?;
    let stdout = child.stdout.take().context("Backend stdout")?;
    let exchange = async {
        let write = async move {
            let _ = stdin.write_all(&body).await;
        };
        let mut output = Vec::new();
        let mut stdout = stdout.take(OUTPUT_LIMIT as u64 + 1);
        let read = stdout.read_to_end(&mut output);
        let ((), read) = tokio::join!(write, read);
        read?;
        ensure!(
            output.len() <= OUTPUT_LIMIT,
            "Git HTTP fixture output too large"
        );
        let status = child.wait().await?;
        ensure!(status.success(), "Git HTTP fixture failed");
        Ok(output)
    };
    let output = tokio::time::timeout(BACKEND_TIMEOUT, exchange)
        .await
        .context("Git HTTP fixture timed out")??;
    cgi_response(&output)
}

/// Split CGI output into headers and body, honoring a `Status:` header.
fn cgi_response(output: &[u8]) -> Result<Response<Full<Bytes>>> {
    let split = output
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("Invalid CGI response")?;
    let head = std::str::from_utf8(&output[..split]).context("Invalid CGI headers")?;
    let mut response = Response::builder();
    let mut status = StatusCode::OK;
    for line in head.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            bail!("Invalid CGI header");
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("status") {
            let code = value.split(' ').next().unwrap_or_default();
            status = StatusCode::from_bytes(code.as_bytes()).context("Invalid CGI status")?;
        } else {
            response = response.header(name, value);
        }
    }
    Ok(response
        .status(status)
        .body(Full::new(Bytes::copy_from_slice(&output[split + 4..])))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgi_status_and_headers_are_translated() {
        let response =
            cgi_response(b"Status: 404 Not Found\r\nContent-Type: text/plain\r\n\r\nmissing")
                .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["content-type"], "text/plain");
        let response = cgi_response(b"Expires: never\r\n\r\n").unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(cgi_response(b"Content-Type: text/plain\r\n").is_err());
    }
}
