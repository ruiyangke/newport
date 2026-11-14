//! Product identity shared by the agent, desktop and release verifier. This
//! describes compiled protocols, independently of dependencies or permissions.
use crate::runtime::service;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: u32 = 1;
pub const CORE_SERVICES: &[&str] = &[
    service::GIT,
    service::MONITOR,
    service::INTEGRATION,
    service::GUI_NOTICES,
    service::LSP,
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    #[default]
    Core,
    Full,
}
impl Variant {
    /// Manual installation permits a managed core-to-full upgrade. Removing
    /// compiled features requires a separate, explicit downgrade request.
    pub fn may_replace(self, installed: Self, automatic: bool, downgrade: bool) -> bool {
        self == installed || (!automatic && (self == Self::Full || downgrade))
    }
    /// The service versions a build of this variant must speak for Newport to
    /// use it.
    pub fn needs(self) -> &'static [(&'static str, u32)] {
        match self {
            Self::Core => &[],
            Self::Full => &[(service::REMOTE_APPS, 1)],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Product {
    pub variant: Variant,
    pub control_versions: Vec<u32>,
    pub services: BTreeMap<String, Vec<u32>>,
}
fn versions(values: &[u32]) -> bool {
    !values.is_empty()
        && values.len() <= 8
        && !values.contains(&0)
        && values
            .iter()
            .enumerate()
            .all(|(i, v)| !values[..i].contains(v))
}
impl Product {
    /// Whether the product is well formed, whatever versions its services
    /// speak.
    pub fn validate_form(&self) -> Result<(), String> {
        if !versions(&self.control_versions)
            || self.services.len() > 15
            || CORE_SERVICES
                .iter()
                .any(|name| !self.services.contains_key(*name))
            || self.services.iter().any(|(name, value)| {
                name.is_empty()
                    || name.len() > 64
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    || name.starts_with("product-")
                    || !versions(value)
            })
            || (self.variant == Variant::Core && self.services.contains_key(service::REMOTE_APPS))
        {
            return Err("Invalid agent product protocols".into());
        }
        Ok(())
    }

    /// Whether Newport can use the product: well formed, and speaking what
    /// its variant needs.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_form()?;
        if self.variant.needs().iter().any(|(name, version)| {
            !self
                .services
                .get(*name)
                .is_some_and(|versions| versions.contains(version))
        }) {
            return Err("Invalid agent product protocols".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Identity {
    pub schema_version: u32,
    pub build_sha256: String,
    /// Product release, independently of protocol versions and executable identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub product: Product,
}
impl Identity {
    /// Whether this names a build and its variant, whatever the build's
    /// services speak: an older build of a variant is still that variant, to
    /// be replaced.
    pub fn validate_build(&self) -> Result<(), String> {
        if self.schema_version != VERSION
            || !valid_build(&self.build_sha256)
            || self
                .version
                .as_deref()
                .is_some_and(|version| !valid_version(version))
        {
            return Err("Invalid agent product identity".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        self.validate_build()?;
        self.product.validate()
    }
}

/// A bounded semantic product version, including prerelease and build metadata.
pub fn valid_version(value: &str) -> bool {
    value.len() <= 128 && semver::Version::parse(value).is_ok()
}

/// Whether a build identity is a lowercase SHA-256 digest.
pub fn valid_build(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn core() -> Product {
        Product {
            variant: Variant::Core,
            control_versions: vec![1],
            services: CORE_SERVICES
                .iter()
                .map(|name| ((*name).into(), vec![1]))
                .collect(),
        }
    }
    #[test]
    fn product_identity_reports_a_semantic_release_version() {
        let mut value = serde_json::json!({
            "schemaVersion": VERSION,
            "buildSha256": "a".repeat(64),
            "product": core(),
        });
        for version in ["0.6.4", "0.7.0-rc.1+local"] {
            value["version"] = version.into();
            let identity: Identity = serde_json::from_value(value.clone()).unwrap();
            identity.validate().unwrap();
            assert_eq!(serde_json::to_value(identity).unwrap()["version"], version);
        }
    }

    #[test]
    fn identity_json_preserves_the_signed_product_shape() {
        let identity = Identity {
            schema_version: VERSION,
            build_sha256: "a".repeat(64),
            version: None,
            product: core(),
        };
        let encoded = serde_json::to_string(&identity).unwrap();
        assert_eq!(
            encoded,
            concat!(
                "{\"schemaVersion\":1,\"buildSha256\":\"",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "\",\"product\":{\"variant\":\"core\",\"controlVersions\":[1],",
                "\"services\":{\"git\":[1],\"gui-notices\":[1],\"integration\":[1],",
                "\"lsp\":[1],\"monitor\":[1]}}}"
            )
        );
        let decoded: Identity = serde_json::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, identity);
    }

    #[test]
    fn product_versions_are_bounded_semver_or_explicitly_unknown() {
        let mut identity = Identity {
            schema_version: VERSION,
            build_sha256: "a".repeat(64),
            version: None,
            product: core(),
        };
        identity.validate().unwrap();
        for version in [
            "", "812297d", "0.6", "v0.6.4", "01.6.4", "0.6.4\n", "0.6.4-01",
        ] {
            identity.version = Some(version.into());
            assert!(identity.validate().is_err(), "{version:?}");
        }
        identity.version = Some(format!("0.6.4+{}", "x".repeat(123)));
        assert!(identity.validate().is_err());
    }
    #[test]
    fn build_validation_checks_schema_and_lowercase_digest() {
        let mut identity = Identity {
            schema_version: VERSION,
            build_sha256: "0123456789abcdef".repeat(4),
            version: None,
            product: core(),
        };
        identity.validate_build().unwrap();
        identity.schema_version += 1;
        assert!(identity.validate_build().is_err());
        identity.schema_version = VERSION;
        for hash in [
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
        ] {
            identity.build_sha256 = hash;
            assert!(!valid_build(&identity.build_sha256));
            assert!(identity.validate_build().is_err());
        }
    }
    #[test]
    fn explicit_products_validate_their_compiled_services() {
        let mut product = core();
        product.validate().unwrap();
        product.variant = Variant::Full;
        assert!(product.validate().is_err());
        product.services.insert("remote-apps".into(), vec![1]);
        product.validate().unwrap();
        product.variant = Variant::Core;
        assert!(product.validate_form().is_err());
    }
    #[test]
    fn incomplete_or_malformed_protocols_fail_closed() {
        for versions in [vec![], vec![0], vec![1, 1], (1..=9).collect()] {
            let mut product = core();
            product.control_versions = versions.clone();
            assert!(product.validate().is_err());
            let mut product = core();
            product.services.insert("monitor".into(), versions);
            assert!(product.validate().is_err());
        }
        let mut product = core();
        product.services.remove("monitor");
        assert!(product.validate().is_err());
        for name in ["", "Invalid", "x/y", "with space"] {
            let mut product = core();
            product.services.insert(name.into(), vec![1]);
            assert!(product.validate().is_err());
        }
        let mut encoded = serde_json::to_value(core()).unwrap();
        encoded["variant"] = "future".into();
        assert!(serde_json::from_value::<Product>(encoded).is_err());
    }
    #[test]
    fn another_service_version_preserves_the_product_identity() {
        let mut product = core();
        product.variant = Variant::Full;
        product.services.insert("remote-apps".into(), vec![2]);
        product.validate_form().unwrap();
        assert_eq!(product.variant, Variant::Full);
        assert!(product.validate().is_err(), "this desktop cannot use it");
    }
    #[test]
    fn automatic_updates_stay_in_variant_and_downgrade_is_explicit() {
        for installed in [Variant::Core, Variant::Full] {
            for candidate in [Variant::Core, Variant::Full] {
                assert_eq!(
                    candidate.may_replace(installed, true, false),
                    candidate == installed
                );
                assert_eq!(
                    candidate.may_replace(installed, true, true),
                    candidate == installed
                );
                assert_eq!(
                    candidate.may_replace(installed, false, false),
                    candidate == installed || candidate == Variant::Full
                );
                assert!(candidate.may_replace(installed, false, true));
            }
        }
    }
}
