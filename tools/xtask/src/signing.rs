//! Apple code signing: local development identity, profile checks, codesign and CI keychain setup.
use crate::util;
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{
    alphabet,
    engine::{general_purpose, DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use plist::{Dictionary, Value};
use serde::Deserialize;
use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const IDENTITY: &str = "NEWPORT_SIGNING_IDENTITY";
const PROFILE: &str = "NEWPORT_PROVISIONING_PROFILE";
const ENTITLEMENTS: &str = "NEWPORT_SIGNING_ENTITLEMENTS";

/// Development signing settings from the environment or `.cargo/signing.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevSigning {
    pub identity: String,
    pub provisioning_profile: PathBuf,
    pub entitlements: PathBuf,
}

/// `.cargo/signing.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SigningFile {
    identity: String,
    provisioning_profile: PathBuf,
    entitlements: PathBuf,
}

impl DevSigning {
    /// `NEWPORT_SIGNING_IDENTITY`, `NEWPORT_PROVISIONING_PROFILE` and
    /// `NEWPORT_SIGNING_ENTITLEMENTS`, else `.cargo/signing.toml`, else `None`.
    pub fn load() -> Result<Option<Self>> {
        Self::resolve(|name| env::var_os(name), &util::root())
    }

    /// All three variables (empty counts as unset), else `<root>/.cargo/signing.toml`
    /// with paths relative to `root`. Variable paths are relative to the current directory.
    pub fn resolve(var: impl Fn(&str) -> Option<OsString>, root: &Path) -> Result<Option<Self>> {
        let names = [IDENTITY, PROFILE, ENTITLEMENTS];
        let [identity, profile, entitlements] =
            names.map(|name| var(name).filter(|v| !v.is_empty()));
        if identity.is_some() || profile.is_some() || entitlements.is_some() {
            let missing: Vec<_> = names
                .iter()
                .zip([&identity, &profile, &entitlements])
                .filter(|(_, value)| value.is_none())
                .map(|(name, _)| *name)
                .collect();
            ensure!(
                missing.is_empty(),
                "Incomplete signing settings: also set {}",
                missing.join(" and ")
            );
            let (identity, profile, entitlements) =
                (identity.unwrap(), profile.unwrap(), entitlements.unwrap());
            return Ok(Some(Self {
                identity: identity
                    .into_string()
                    .map_err(|_| anyhow!("{IDENTITY} is not valid UTF-8"))?,
                provisioning_profile: std::path::absolute(profile)?,
                entitlements: std::path::absolute(entitlements)?,
            }));
        }
        let path = root.join(".cargo/signing.toml");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("Cannot read {}", path.display())),
        };
        let file: SigningFile =
            toml::from_str(&text).with_context(|| format!("Invalid {}", path.display()))?;
        ensure!(
            !file.identity.is_empty(),
            "{} has an empty identity",
            path.display()
        );
        Ok(Some(Self {
            identity: file.identity,
            provisioning_profile: root.join(file.provisioning_profile),
            entitlements: root.join(file.entitlements),
        }))
    }
}

/// Whole-string glob match: `*` is any run of characters, `?` exactly one.
pub fn glob_matches(value: &str, pattern: &str) -> bool {
    let (v, p): (Vec<char>, Vec<char>) = (value.chars().collect(), pattern.chars().collect());
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while i < v.len() {
        if j < p.len() && p[j] == '*' {
            star = Some((j, i));
            j += 1;
        } else if j < p.len() && (p[j] == '?' || p[j] == v[i]) {
            i += 1;
            j += 1;
        } else if let Some((s, m)) = star {
            star = Some((s, m + 1));
            j = s + 1;
            i = m + 1;
        } else {
            return false;
        }
    }
    p[j..].iter().all(|&c| c == '*')
}

fn application_identifier(d: &Dictionary) -> Result<&str> {
    match d
        .get("com.apple.application-identifier")
        .or_else(|| d.get("application-identifier"))
    {
        None => Ok(""),
        Some(value) => value
            .as_string()
            .context("The application identifier must be a string"),
    }
}

fn strings<'a>(d: &'a Dictionary, key: &str) -> Result<Vec<&'a str>> {
    match d.get(key) {
        None => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .with_context(|| format!("{key} must be an array"))?
            .iter()
            .map(|item| {
                item.as_string()
                    .with_context(|| format!("{key} must hold strings"))
            })
            .collect(),
    }
}

/// Require the entitlements to identify `identifier`, lead with its Keychain group,
/// and stay within what the profile's entitlements (`allowed`) authorize.
pub fn validate(entitlements: &Dictionary, allowed: &Dictionary, identifier: &str) -> Result<()> {
    let suffix = format!(".{identifier}");
    let id = application_identifier(entitlements)?;
    ensure!(
        id.ends_with(&suffix),
        "Signing entitlements must identify {identifier}"
    );
    ensure!(
        glob_matches(id, application_identifier(allowed)?),
        "Provisioning profile does not authorize the app identifier"
    );
    let groups = strings(entitlements, "keychain-access-groups")?;
    let permitted = strings(allowed, "keychain-access-groups")?;
    ensure!(
        groups.first().is_some_and(|g| g.ends_with(&suffix)),
        "The first Keychain group must be the Newport group"
    );
    ensure!(
        groups
            .iter()
            .all(|g| permitted.iter().any(|p| glob_matches(g, p))),
        "Provisioning profile does not authorize all Keychain groups"
    );
    Ok(())
}

/// The `Entitlements` dictionary of a decoded provisioning profile property list.
pub fn profile_entitlements(xml: &[u8]) -> Result<Dictionary> {
    Value::from_reader(io::Cursor::new(xml))
        .context("Invalid provisioning profile property list")?
        .into_dictionary()
        .and_then(|mut d| d.remove("Entitlements"))
        .and_then(Value::into_dictionary)
        .context("Provisioning profile has no entitlements")
}

/// The signed property list inside a provisioning profile, via `security cms -D`.
pub fn decode_profile(profile: &Path) -> Result<Vec<u8>> {
    let output = Command::new("security")
        .args(["cms", "-D", "-i"])
        .arg(profile)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("Cannot start security")?;
    ensure!(
        output.status.success(),
        "Cannot decode provisioning profile {}",
        profile.display()
    );
    Ok(output.stdout)
}

/// Require the entitlements to match the provisioning profile and bundle identifier.
pub fn check(entitlements: &Path, profile: &Path, identifier: &str) -> Result<()> {
    let requested = util::read_plist(entitlements)?
        .into_dictionary()
        .with_context(|| format!("{} is not a dictionary", entitlements.display()))?;
    let allowed = profile_entitlements(&decode_profile(profile)?)?;
    validate(&requested, &allowed, identifier)
}

/// `codesign --force --sign <identity> <options…> <path>`.
pub fn codesign(path: &Path, identity: &str, options: &[&str]) -> Result<()> {
    quietly(
        Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", identity])
            .args(options)
            .arg(path),
    )
    .with_context(|| format!("Cannot sign {}", path.display()))
}

/// `codesign --verify --deep --strict <path>`.
pub fn verify(path: &Path) -> Result<()> {
    quietly(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(path),
    )
    .with_context(|| format!("Signature verification failed: {}", path.display()))
}

/// Run `codesign`, showing its output only when it fails.
fn quietly(c: &mut Command) -> Result<()> {
    let output = c
        .stdin(Stdio::null())
        .output()
        .context("Cannot start codesign")?;
    if !output.status.success() {
        bail!(
            "codesign failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// CI: write the certificate, profile and entitlements and create a temporary keychain.
pub fn setup_ci() -> Result<()> {
    let identifier = util::app()?.identifier;
    setup(
        |name| env::var(name).ok(),
        |entitlements, profile| check(entitlements, profile, &identifier),
        |args| {
            util::succeeds(util::without_release_secrets(
                Command::new("security").args(args),
            ))
        },
    )
}

/// Owner-only file, replacing any previous content.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        let mut file = options
            .open(path)
            .with_context(|| format!("Cannot write {}", path.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
    }
    #[cfg(not(unix))]
    options
        .open(path)
        .with_context(|| format!("Cannot write {}", path.display()))?
        .write_all(bytes)?;
    Ok(())
}

/// Lenient standard base64: whitespace ignored, padding optional.
fn decode_secret(name: &str, value: &str) -> Result<Vec<u8>> {
    const ENGINE: GeneralPurpose = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    let compact: String = value.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    ENGINE
        .decode(compact)
        .map_err(|_| anyhow!("{name} is not valid base64"))
}

/// [`setup_ci`] with its environment, profile check and `security` runner injected.
fn setup(
    var: impl Fn(&str) -> Option<String>,
    check: impl Fn(&Path, &Path) -> Result<()>,
    mut security: impl FnMut(&[&str]) -> bool,
) -> Result<()> {
    let required = |name: &str| {
        var(name)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow!("Missing secret or setting: {name}"))
    };
    let temp = PathBuf::from(required("RUNNER_TEMP")?);
    let certificate = required("APPLE_CERTIFICATE")?;
    let profile_data = required("APPLE_PROVISIONING_PROFILE")?;
    let entitlements_data = required("APPLE_ENTITLEMENTS")?;
    let github_env = required("GITHUB_ENV")?;
    let certificate_password =
        var("APPLE_CERTIFICATE_PASSWORD").context("Missing APPLE_CERTIFICATE_PASSWORD")?;

    let cert = temp.join("signing.p12");
    let profile = temp.join("release.provisionprofile");
    let entitlements = temp.join("release.entitlements");
    write_private(&cert, &decode_secret("APPLE_CERTIFICATE", &certificate)?)?;
    write_private(
        &profile,
        &decode_secret("APPLE_PROVISIONING_PROFILE", &profile_data)?,
    )?;
    write_private(&entitlements, entitlements_data.as_bytes())?;
    let requested = util::read_plist(&entitlements)?;
    if requested
        .as_dictionary()
        .and_then(|d| d.get("com.apple.security.get-task-allow"))
        .is_some_and(|v| v.as_boolean() != Some(false))
    {
        bail!("Release signing must disable get-task-allow");
    }
    check(&entitlements, &profile)?;

    let keychain = temp.join("release.keychain-db");
    let keychain = keychain
        .to_str()
        .context("RUNNER_TEMP is not valid UTF-8")?;
    let cert = cert.to_str().context("RUNNER_TEMP is not valid UTF-8")?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)
        .map_err(|e| anyhow!("Cannot generate a keychain password: {e}"))?;
    let password = general_purpose::URL_SAFE_NO_PAD.encode(random);
    for args in [
        &["create-keychain", "-p", &password, keychain][..],
        &["set-keychain-settings", "-lut", "7200", keychain],
        &["unlock-keychain", "-p", &password, keychain],
        &[
            "import",
            cert,
            "-k",
            keychain,
            "-P",
            &certificate_password,
            "-T",
            "/usr/bin/codesign",
        ],
        &[
            "set-key-partition-list",
            "-S",
            "apple-tool:,apple:,codesign:",
            "-s",
            "-k",
            &password,
            keychain,
        ],
        &["list-keychains", "-d", "user", "-s", keychain],
    ] {
        ensure!(security(args), "Signing keychain setup failed");
    }
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&github_env)
        .context("Cannot open GITHUB_ENV")?
        .write_all(
            format!(
                "{PROFILE}={}\n{ENTITLEMENTS}={}\n",
                profile.display(),
                entitlements.display()
            )
            .as_bytes(),
        )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn dict(pairs: &[(&str, Value)]) -> Dictionary {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn array(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| Value::from(*s)).collect())
    }

    #[test]
    fn globs_match_whole_strings() {
        assert!(glob_matches("TEAM.app.newport", "TEAM.*"));
        assert!(glob_matches("TEAM.app.newport", "TEAM.app.newport"));
        assert!(glob_matches("TEAM.app.newport", "TEAM.app.newpor?"));
        assert!(glob_matches("a.b.c", "*.c"));
        assert!(glob_matches("", ""));
        assert!(!glob_matches("TEAM.app.newport", ""));
        assert!(!glob_matches("TEAMXapp", "TEAM.*"));
        assert!(!glob_matches("OTHER.app.newport", "TEAM.*"));
        assert!(!glob_matches("TEAM.app.newport.x", "TEAM.app.newport"));
        assert!(glob_matches("a(b)+c", "a(b)+?"));
    }

    #[test]
    fn signing_requires_the_newport_keychain_group_and_authorized_groups_only() {
        let ent = dict(&[
            (
                "com.apple.application-identifier",
                "TEAM.app.newport".into(),
            ),
            ("keychain-access-groups", array(&["TEAM.app.newport"])),
        ]);
        let allowed = dict(&[
            (
                "com.apple.application-identifier",
                "TEAM.app.newport".into(),
            ),
            ("keychain-access-groups", array(&["TEAM.*"])),
        ]);
        validate(&ent, &allowed, "app.newport").unwrap();
        let mut bad = vec![{
            let mut e = ent.clone();
            e.insert(
                "com.apple.application-identifier".into(),
                "TEAM.com.example.other".into(),
            );
            e
        }];
        for groups in [
            &[][..],
            &["TEAM.com.example.other", "TEAM.app.newport"],
            &["OTHER.app.newport"],
            &["TEAM.app.newport", "OTHER.com.example.other"],
        ] {
            let mut e = ent.clone();
            e.insert("keychain-access-groups".into(), array(groups));
            bad.push(e);
        }
        for e in bad {
            assert!(validate(&e, &allowed, "app.newport").is_err(), "{e:?}");
        }
        let unauthorized = dict(&[
            ("application-identifier", "TEAM.app.other".into()),
            ("keychain-access-groups", array(&["TEAM.*"])),
        ]);
        assert!(validate(&ent, &unauthorized, "app.newport")
            .unwrap_err()
            .to_string()
            .contains("app identifier"));
    }

    #[test]
    fn profile_extraction_accepts_certificate_data_and_date_fields() {
        let xml = br#"<?xml version="1.0"?><plist version="1.0"><dict><key>CreationDate</key><date>2026-01-01T00:00:00Z</date><key>Entitlements</key><dict><key>keychain-access-groups</key><array><string>TEAM.*</string></array></dict><key>DeveloperCertificates</key><array><data>YWJj</data></array></dict></plist>"#;
        assert_eq!(
            profile_entitlements(xml).unwrap(),
            dict(&[("keychain-access-groups", array(&["TEAM.*"]))])
        );
        assert!(profile_entitlements(b"<plist version=\"1.0\"><dict/></plist>").is_err());
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn dev_signing_comes_from_all_three_variables_or_the_settings_file() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(DevSigning::resolve(vars(&[]), root.path()).unwrap(), None);
        let full = DevSigning::resolve(
            vars(&[
                (IDENTITY, "ABC"),
                (PROFILE, "/p/Newport.provisionprofile"),
                (ENTITLEMENTS, "/p/Newport.entitlements"),
            ]),
            root.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(full.identity, "ABC");
        assert_eq!(full.entitlements, Path::new("/p/Newport.entitlements"));
        let relative = DevSigning::resolve(
            vars(&[
                (IDENTITY, "ABC"),
                (PROFILE, "a.profile"),
                (ENTITLEMENTS, "b"),
            ]),
            root.path(),
        )
        .unwrap()
        .unwrap();
        assert!(relative.provisioning_profile.is_absolute());
        let error =
            DevSigning::resolve(vars(&[(IDENTITY, "ABC"), (ENTITLEMENTS, "")]), root.path())
                .unwrap_err()
                .to_string();
        assert!(
            error.contains(PROFILE) && error.contains(ENTITLEMENTS),
            "{error}"
        );
        assert!(!error.contains(IDENTITY), "{error}");

        fs::create_dir(root.path().join(".cargo")).unwrap();
        let file = root.path().join(".cargo/signing.toml");
        fs::write(
            &file,
            "identity = \"0123\"\nprovisioning_profile = \".cargo/N.provisionprofile\"\nentitlements = \"/abs/N.entitlements\"\n",
        )
        .unwrap();
        assert_eq!(
            DevSigning::resolve(vars(&[]), root.path()).unwrap(),
            Some(DevSigning {
                identity: "0123".into(),
                provisioning_profile: root.path().join(".cargo/N.provisionprofile"),
                entitlements: "/abs/N.entitlements".into(),
            })
        );
        fs::write(&file, "identity = \"0123\"\n").unwrap();
        assert!(DevSigning::resolve(vars(&[]), root.path()).is_err());
        fs::write(&file, "identity = \"0\"\nprovisioning_profile = \"a\"\nentitlements = \"b\"\nidentiy = \"x\"\n").unwrap();
        assert!(DevSigning::resolve(vars(&[]), root.path()).is_err());
    }

    const ENTITLEMENTS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>com.apple.application-identifier</key><string>TEAM.app.newport</string></dict></plist>"#;

    fn ci_vars(temp: &Path, entitlements: &str) -> HashMap<String, String> {
        [
            ("RUNNER_TEMP", temp.to_str().unwrap()),
            ("APPLE_CERTIFICATE", "Y2VydA==\n"),
            ("APPLE_PROVISIONING_PROFILE", "cHJvZmlsZQ"),
            ("APPLE_ENTITLEMENTS", entitlements),
            ("APPLE_CERTIFICATE_PASSWORD", "p12-secret"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .chain([(
            "GITHUB_ENV".to_string(),
            temp.join("github_env").to_str().unwrap().to_string(),
        )])
        .collect()
    }

    #[cfg(unix)]
    #[test]
    fn ci_signing_writes_private_files_and_a_temporary_keychain() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let map = ci_vars(temp.path(), ENTITLEMENTS_XML);
        let mut calls: Vec<Vec<String>> = Vec::new();
        setup(
            |n| map.get(n).cloned(),
            |e, p| {
                assert!(
                    e.ends_with("release.entitlements") && p.ends_with("release.provisionprofile")
                );
                Ok(())
            },
            |args| {
                calls.push(args.iter().map(|a| a.to_string()).collect());
                true
            },
        )
        .unwrap();
        for (name, content) in [
            ("signing.p12", &b"cert"[..]),
            ("release.provisionprofile", b"profile"),
            ("release.entitlements", ENTITLEMENTS_XML.as_bytes()),
        ] {
            let path = temp.path().join(name);
            assert_eq!(fs::read(&path).unwrap(), content);
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let keychain = temp.path().join("release.keychain-db");
        let keychain = keychain.to_str().unwrap();
        let commands: Vec<&str> = calls.iter().map(|c| c[0].as_str()).collect();
        assert_eq!(
            commands,
            [
                "create-keychain",
                "set-keychain-settings",
                "unlock-keychain",
                "import",
                "set-key-partition-list",
                "list-keychains"
            ]
        );
        let password = &calls[0][2];
        assert_eq!(password.len(), 43);
        assert_eq!(calls[2], ["unlock-keychain", "-p", password, keychain]);
        assert!(calls[3].contains(&"p12-secret".to_string()));
        assert!(calls
            .iter()
            .all(|c| c.last().unwrap() == keychain || c[0] == "import"));
        assert_eq!(
            fs::read_to_string(temp.path().join("github_env")).unwrap(),
            format!(
                "{PROFILE}={}\n{ENTITLEMENTS}={}\n",
                temp.path().join("release.provisionprofile").display(),
                temp.path().join("release.entitlements").display()
            )
        );
    }

    #[test]
    fn ci_signing_rejects_missing_settings_debuggable_builds_and_hides_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let ok = |_: &Path, _: &Path| Ok(());
        for name in ["RUNNER_TEMP", "APPLE_CERTIFICATE", "GITHUB_ENV"] {
            let mut map = ci_vars(temp.path(), ENTITLEMENTS_XML);
            map.insert(name.into(), String::new());
            let error = setup(|n| map.get(n).cloned(), ok, |_| true).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("Missing secret or setting: {name}")
            );
        }
        let mut map = ci_vars(temp.path(), ENTITLEMENTS_XML);
        map.insert("APPLE_CERTIFICATE_PASSWORD".into(), String::new());
        setup(|n| map.get(n).cloned(), ok, |_| true).unwrap();
        map.remove("APPLE_CERTIFICATE_PASSWORD");
        assert!(setup(|n| map.get(n).cloned(), ok, |_| true).is_err());

        let debuggable = r#"<plist version="1.0"><dict><key>com.apple.security.get-task-allow</key><true/></dict></plist>"#;
        let map = ci_vars(temp.path(), debuggable);
        let error = setup(|n| map.get(n).cloned(), ok, |_| panic!("no keychain")).unwrap_err();
        assert!(error.to_string().contains("get-task-allow"));

        let mut map = ci_vars(temp.path(), ENTITLEMENTS_XML);
        map.insert("APPLE_CERTIFICATE".into(), "not base64 !".into());
        assert_eq!(
            setup(|n| map.get(n).cloned(), ok, |_| true)
                .unwrap_err()
                .to_string(),
            "APPLE_CERTIFICATE is not valid base64"
        );

        let map = ci_vars(temp.path(), ENTITLEMENTS_XML);
        let error = setup(|n| map.get(n).cloned(), ok, |args| args[0] != "import").unwrap_err();
        let chain = format!("{error:#}");
        assert_eq!(chain, "Signing keychain setup failed");
        assert!(!fs::read_to_string(temp.path().join("github_env"))
            .unwrap_or_default()
            .contains("p12-secret"));
    }
}
