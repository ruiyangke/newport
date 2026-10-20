//! Newport account API transport and this computer's identity. Blocking: run
//! on a worker, never the GUI thread or the async executor.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{io::Read as _, time::Duration};
use url::Url;

const BODY_LIMIT: u64 = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// A failed call. `Rejected` carries the API's own error code and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Rejected {
        status: u16,
        code: String,
        message: String,
    },
    Unavailable(String),
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { status, code, .. } => write!(f, "{code} ({status})"),
            Self::Unavailable(error) => f.write_str(error),
        }
    }
}

pub struct Api {
    base: Url,
    agent: ureq::Agent,
    user_agent: String,
}

impl Api {
    /// `base` is the account site's origin. Plain HTTP is accepted only for a
    /// loopback development portal.
    pub fn new(base: Url, app_version: &str) -> Result<Self, String> {
        validate(&base)?;
        // No redirects: the API never sends them, and following one could
        // forward the device credential somewhere else.
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(TIMEOUT));
        // Windows uses Schannel through native-tls; other targets use rustls.
        #[cfg(windows)]
        let config = config.tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        );
        Ok(Self {
            base,
            agent: config.build().new_agent(),
            user_agent: format!("Newport/{app_version}"),
        })
    }

    pub fn post(
        &self,
        path: &str,
        credential: Option<&str>,
        body: &Value,
    ) -> Result<Value, Failure> {
        let url = self.url(path)?;
        let mut request = self
            .agent
            .post(url.as_str())
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .content_type("application/json");
        if let Some(credential) = credential {
            request = request.header("Authorization", format!("Bearer {credential}"));
        }
        let body =
            serde_json::to_vec(body).map_err(|error| Failure::Unavailable(error.to_string()))?;
        let response = request
            .send(&body[..])
            .map_err(|error| Failure::Unavailable(error.to_string()))?;
        read(response)
    }

    pub fn delete(&self, path: &str, credential: &str) -> Result<(), Failure> {
        let url = self.url(path)?;
        let response = self
            .agent
            .delete(url.as_str())
            .header("User-Agent", &self.user_agent)
            .header("Authorization", format!("Bearer {credential}"))
            .call()
            .map_err(|error| Failure::Unavailable(error.to_string()))?;
        read(response).map(|_| ())
    }

    fn url(&self, path: &str) -> Result<Url, Failure> {
        self.base
            .join(path)
            .map_err(|error| Failure::Unavailable(error.to_string()))
    }
}

/// Account URLs must not carry credentials, and only loopback may use HTTP.
pub fn validate(url: &Url) -> Result<(), String> {
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("Account URLs must not contain credentials or fragments.".into());
    }
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => Ok(()),
        ("http", Some(url::Host::Domain("localhost"))) => Ok(()),
        ("http", Some(url::Host::Ipv4(ip))) if ip.is_loopback() => Ok(()),
        ("http", Some(url::Host::Ipv6(ip))) if ip.is_loopback() => Ok(()),
        _ => Err("The account site must use HTTPS.".into()),
    }
}

fn read(mut response: ureq::http::Response<ureq::Body>) -> Result<Value, Failure> {
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(BODY_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Failure::Unavailable(error.to_string()))?;
    if bytes.len() as u64 > BODY_LIMIT {
        return Err(Failure::Unavailable(
            "The account response is too large.".into(),
        ));
    }
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).ok().unwrap_or(Value::Null)
    };
    if (200..300).contains(&status) {
        return Ok(value);
    }
    match value.get("error").and_then(Value::as_str) {
        Some(code) => Err(Failure::Rejected {
            status,
            code: code.to_owned(),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        // Proxies and outages answer without the API's error shape.
        None => Err(Failure::Unavailable(format!(
            "The account site answered {status}."
        ))),
    }
}

/// A stable, app-specific hash of the operating system's machine ID. The raw ID
/// never leaves this computer.
pub fn machine_id() -> Result<String, String> {
    let id = os_machine_id()?;
    let id = id.trim();
    if id.is_empty() {
        return Err("This computer has no machine ID.".into());
    }
    let mut hash = Sha256::new();
    hash.update(b"newport-machine-v1\0");
    hash.update(id.to_ascii_lowercase().as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(hash.finalize()))
}

/// The name people gave this computer, for their account's list of computers.
pub fn computer_name() -> String {
    let name = whoami::fallible::devicename().unwrap_or_default();
    let name: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    if name.trim().is_empty() {
        "This computer".into()
    } else {
        name.trim().to_owned()
    }
}

pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

#[cfg(target_os = "macos")]
fn os_machine_id() -> Result<String, String> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::string::CFString;
    use std::ffi::c_void;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const std::ffi::c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }

    // SAFETY: IOServiceGetMatchingService consumes the matching dictionary;
    // the service is released below and the property follows the create rule.
    unsafe {
        let matching = IOServiceMatching(c"IOPlatformExpertDevice".as_ptr());
        if matching.is_null() {
            return Err("Cannot read this computer's hardware ID.".into());
        }
        let service = IOServiceGetMatchingService(0, matching);
        if service == 0 {
            return Err("Cannot read this computer's hardware ID.".into());
        }
        let key = CFString::from_static_string("IOPlatformUUID");
        let value = IORegistryEntryCreateCFProperty(
            service,
            key.as_concrete_TypeRef().cast(),
            std::ptr::null(),
            0,
        );
        IOObjectRelease(service);
        if value.is_null() {
            return Err("Cannot read this computer's hardware ID.".into());
        }
        CFType::wrap_under_create_rule(value.cast())
            .downcast::<CFString>()
            .map(|value| value.to_string())
            .ok_or_else(|| "Cannot read this computer's hardware ID.".into())
    }
}

#[cfg(windows)]
fn os_machine_id() -> Result<String, String> {
    use winreg::{RegKey, enums::*};
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            "SOFTWARE\\Microsoft\\Cryptography",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .and_then(|key| key.get_value::<String, _>("MachineGuid"))
        .map_err(|error| format!("Cannot read this computer's machine ID: {error}"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_machine_id() -> Result<String, String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .ok_or_else(|| "Cannot read this computer's machine ID.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_is_a_stable_base64url_sha256() {
        let first = machine_id().unwrap();
        assert_eq!(first.len(), 43);
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_eq!(machine_id().unwrap(), first);
    }

    #[test]
    fn account_urls_require_https_except_loopback() {
        for ok in [
            "https://account.newport.app",
            "http://localhost:3000",
            "http://127.0.0.1:3000",
            "http://[::1]:3000",
        ] {
            assert!(validate(&Url::parse(ok).unwrap()).is_ok(), "{ok}");
        }
        for bad in [
            "http://account.newport.app",
            "https://user:pass@account.newport.app",
            "https://account.newport.app/#x",
            "file:///tmp",
        ] {
            assert!(validate(&Url::parse(bad).unwrap()).is_err(), "{bad}");
        }
    }

    #[test]
    fn computer_name_is_never_empty() {
        let name = computer_name();
        assert!(!name.is_empty() && name.chars().count() <= 80);
    }

    fn serve(response: &'static str) -> (Url, std::thread::JoinHandle<String>) {
        use std::io::Write as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!(
            "http://127.0.0.1:{}",
            listener.local_addr().unwrap().port()
        ))
        .unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0; 8192];
            let count = stream.read(&mut request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request[..count]).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn post_sends_the_credential_and_reads_json() {
        let (url, server) = serve(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}",
        );
        let api = Api::new(url, "1.2.3").unwrap();
        let value = api
            .post(
                "/api/v1/license",
                Some("secret"),
                &serde_json::json!({"a":1}),
            )
            .unwrap();
        assert_eq!(value, serde_json::json!({"ok":true}));
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /api/v1/license "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer secret")
        );
        assert!(request.contains("Newport/1.2.3"));
    }

    #[test]
    fn api_errors_keep_their_code() {
        let body = r#"{"error":"device_limit","message":"Full."}"#;
        let response: &'static str = Box::leak(
            format!(
                "HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        );
        let (url, server) = serve(response);
        let api = Api::new(url, "1.2.3").unwrap();
        let failure = api.post("/api/v1/devices", None, &Value::Null).unwrap_err();
        server.join().unwrap();
        assert_eq!(
            failure,
            Failure::Rejected {
                status: 409,
                code: "device_limit".into(),
                message: "Full.".into()
            }
        );
    }

    #[test]
    fn redirects_are_not_followed() {
        let (url, server) = serve(
            "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/steal\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        );
        let api = Api::new(url, "1.2.3").unwrap();
        let failure = api.delete("/api/v1/devices/self", "secret").unwrap_err();
        server.join().unwrap();
        assert!(matches!(failure, Failure::Unavailable(_)));
    }
}
