//! Bounded desktop catalogue shared by discovery and the native launcher.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

#[cfg(feature = "protobuf")]
pub mod wire;
pub const MAX_CATALOGUE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_APPLICATIONS: usize = 1024;
pub const MAX_ID: usize = 4096;
pub const MAX_NAME: usize = 4096;
pub const MAX_DESCRIPTION: usize = 16384;
pub const MAX_KEYWORDS: usize = 128;
pub const MAX_KEYWORD: usize = 4096;
pub const MAX_ACTIONS: usize = 32;
pub const MAX_ACTION_ID: usize = 256;
pub const MAX_ERROR_MESSAGE: usize = 4096;
pub const CATALOGUE_BUDGET: usize = 16;
pub const SERVER_LIMITS: crate::common::Limits = crate::common::Limits {
    message: MAX_CATALOGUE_BYTES,
    metadata: MAX_CATALOGUE_BYTES,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Invalid {}
pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_ID || id.contains(['/', '\0']) || !id.ends_with(".desktop") {
        return Err("Invalid desktop application ID".into());
    }
    Ok(())
}
pub fn validate_action(action: &str) -> Result<(), String> {
    if action.is_empty()
        || action.len() > MAX_ACTION_ID
        || !action
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Invalid desktop action identifier".into());
    }
    Ok(())
}
/// Explicit remote-side URIs to open, never shell arguments or local uploads.
/// Empty means ordinary activation. Bound the whole request before launching.
pub fn validate_open_uris(uris: &[String]) -> Result<(), String> {
    if uris.len() > 32 || uris.iter().map(String::len).sum::<usize>() > 65536 {
        return Err("Too many or oversized application URIs".into());
    }
    for uri in uris {
        let (scheme, rest) = uri
            .split_once(':')
            .ok_or("Application URI must be absolute")?;
        if uri.len() > 8192
            || rest.is_empty()
            || scheme.is_empty()
            || !scheme.as_bytes()[0].is_ascii_alphabetic()
            || !scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
            || uri.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("Invalid application URI".into());
        }
        let bytes = uri.as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'%'
                && (index + 2 >= bytes.len()
                    || !bytes[index + 1].is_ascii_hexdigit()
                    || !bytes[index + 2].is_ascii_hexdigit())
            {
                return Err("Invalid application URI escape".into());
            }
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopAction {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub actions: Vec<DesktopAction>,
}
impl Application {
    pub fn validate_metadata(&self) -> Result<(), Invalid> {
        validate_id(&self.id).map_err(|_| Invalid("Invalid desktop application ID"))?;
        if self.actions.len() > MAX_ACTIONS {
            return Err(Invalid("Too many desktop actions"));
        }
        let mut ids = BTreeSet::new();
        for action in &self.actions {
            validate_action(&action.id)
                .map_err(|_| Invalid("Invalid desktop action identifier"))?;
            if action.name.is_empty() || action.name.len() > MAX_NAME || !ids.insert(&action.id) {
                return Err(Invalid("Invalid or duplicate desktop action metadata"));
            }
        }
        if self.name.len() > MAX_NAME
            || self.description.len() > MAX_DESCRIPTION
            || self.keywords.len() > MAX_KEYWORDS
            || self.keywords.iter().any(|s| s.len() > MAX_KEYWORD)
        {
            return Err(Invalid("Desktop entry metadata exceeds limits"));
        }
        Ok(())
    }

    /// Reserves bounded field overhead as well as text, without needing an encoder.
    pub fn byte_budget(&self) -> Result<usize, Invalid> {
        self.validate_metadata()?;
        let icon = self.icon.as_ref().map_or(0, String::len);
        if icon > MAX_CATALOGUE_BYTES {
            return Err(Invalid("Desktop icon exceeds catalogue limit"));
        }
        Ok(64
            + self.id.len()
            + self.name.len()
            + self.description.len()
            + icon
            + self.keywords.iter().map(|s| 8 + s.len()).sum::<usize>()
            + self
                .actions
                .iter()
                .map(|a| 24 + a.id.len() + a.name.len())
                .sum::<usize>())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalogue {
    pub apps: Vec<Application>,
}
impl Catalogue {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.apps.len() > MAX_APPLICATIONS {
            return Err(Invalid("Too many desktop applications"));
        }
        let mut ids = BTreeSet::new();
        let mut used = CATALOGUE_BUDGET;
        for app in &self.apps {
            used += app.byte_budget()?;
            if used > MAX_CATALOGUE_BYTES {
                return Err(Invalid("Catalogue exceeds its byte budget"));
            }
            if !ids.insert(&app.id) {
                return Err(Invalid("Duplicate desktop application ID"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Catalogue(Catalogue),
    Failed(crate::common::Error),
}
impl Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Catalogue(value) => value.validate(),
            Self::Failed(error)
                if !error.code.is_empty()
                    && error.code.len() <= 64
                    && !error.message.is_empty()
                    && error.message.len() <= MAX_ERROR_MESSAGE
                    && !error.code.contains('\0')
                    && !error.message.contains('\0') =>
            {
                Ok(())
            }
            Self::Failed(_) => Err(Invalid("Invalid catalogue failure")),
        }
    }
    pub fn into_result(self) -> Result<Catalogue, crate::common::Error> {
        match self {
            Self::Catalogue(value) => Ok(value),
            Self::Failed(error) => Err(error),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> Application {
        Application {
            id: "test.desktop".into(),
            name: "Test".into(),
            description: String::new(),
            keywords: vec![],
            icon: None,
            actions: vec![],
        }
    }
    #[test]
    fn catalogue_actions_are_bounded_and_unique() {
        let mut entry = app();
        entry.actions = (0..32)
            .map(|i| DesktopAction {
                id: format!("action-{i}"),
                name: "é".repeat(2048),
            })
            .collect();
        assert!(entry.validate_metadata().is_ok());
        let mut catalogue = Catalogue { apps: vec![entry] };
        assert!(catalogue.validate().is_ok());
        catalogue.apps[0].actions[0].name.push('x');
        assert!(catalogue.apps[0].validate_metadata().is_err());
        catalogue.apps[0].actions[0].name = "Valid".into();
        catalogue.apps[0].actions[0].id = "action-1".into();
        assert!(catalogue.apps[0].validate_metadata().is_err());
        catalogue.apps[0].actions[0].id = "action-0".into();
        catalogue.apps[0].actions.push(DesktopAction {
            id: "overflow".into(),
            name: "Overflow".into(),
        });
        assert!(catalogue.apps[0].validate_metadata().is_err());
    }
    #[test]
    fn desktop_action_identifier_is_a_bounded_key_name() {
        for value in ["new-window", "PrivateWindow", "Action123"] {
            assert!(validate_action(value).is_ok());
        }
        for value in [
            "",
            "bad action",
            "bad/action",
            "action\n",
            "[group]",
            "世界",
            "action_name",
        ] {
            assert!(validate_action(value).is_err());
        }
        assert!(validate_action(&"a".repeat(256)).is_ok());
        assert!(validate_action(&"a".repeat(257)).is_err());
    }
    #[test]
    fn open_uris_are_absolute_bounded_and_preserve_literals() {
        for value in [
            "file:///tmp/a%20b",
            "https://example.invalid/?x=$HOME&y=%25",
            "custom+app:value",
        ] {
            assert!(validate_open_uris(&[value.into()]).is_ok());
        }
        for value in [
            "",
            "relative",
            "/tmp/file",
            "1bad:value",
            "https:",
            "https://a b",
            "file:///a\n",
            "file:///a\0",
            "file:///bad%",
            "https://a/%GG",
        ] {
            assert!(validate_open_uris(&[value.into()]).is_err());
        }
        assert!(validate_open_uris(&vec!["x:a".into(); 33]).is_err());
        assert!(validate_open_uris(&[format!("x:{}", "a".repeat(8191))]).is_err());
        assert!(validate_open_uris(&vec![format!("x:{}", "a".repeat(8190)); 9]).is_err());
    }
    #[test]
    fn catalogue_boundaries_and_unique_launch_ids() {
        let mut entry = app();
        entry.id = format!("{}.desktop", "a".repeat(4088));
        entry.name = "é".repeat(2048);
        let mut catalogue = Catalogue { apps: vec![entry] };
        catalogue.validate().unwrap();
        assert_eq!(catalogue.apps[0].name.len(), MAX_NAME);
        catalogue.apps[0].name.push('x');
        assert!(catalogue.validate().is_err());
        assert!(
            Catalogue {
                apps: vec![app(), app()]
            }
            .validate()
            .is_err()
        );
        for id in [
            "",
            "../test.desktop",
            "test\0.desktop",
            "test",
            &format!("{}.desktop", "a".repeat(4089)),
        ] {
            assert!(validate_id(id).is_err());
        }
        let mut huge = app();
        huge.icon = Some("a".repeat(MAX_CATALOGUE_BYTES));
        assert!(Catalogue { apps: vec![huge] }.validate().is_err());
        assert!(
            Catalogue {
                apps: vec![app(); MAX_APPLICATIONS + 1]
            }
            .validate()
            .is_err()
        );
    }
}
