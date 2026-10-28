//! Product identity shared by the agent, desktop and release verifier. This
//! describes compiled protocols, independently of dependencies or permissions.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: u32 = 1;
pub const CORE_MARKER: &str = "product-core";
pub const FULL_MARKER: &str = "product-full";
pub const CORE_SERVICES: &[&str] = &[
    "git",
    "monitor",
    "integration",
    "gui-notices",
    "gui-launch",
    "lsp",
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    #[default]
    Core,
    Full,
}
impl Variant {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Core => CORE_MARKER,
            Self::Full => FULL_MARKER,
        }
    }
    /// Manual installation permits a managed core-to-full upgrade. Removing
    /// compiled features requires a separate, explicit downgrade request.
    pub fn may_replace(self, installed: Self, automatic: bool, downgrade: bool) -> bool {
        self == installed || (!automatic && (self == Self::Full || downgrade))
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
    pub fn validate(&self) -> Result<(), String> {
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
            || (self.variant == Variant::Full) != self.services.contains_key("remote-apps")
        {
            return Err("Invalid agent product protocols".into());
        }
        Ok(())
    }

    /// Version 1 readers already permit additional capability names. A marker
    /// adds product identity without adding fields to their strict Hello shape.
    /// Unmarked legacy core is recognized only by its closed set of services;
    /// an unknown extension is never assumed safe to replace with core.
    pub fn from_capabilities<'a>(
        control_versions: &[u32],
        capabilities: impl IntoIterator<Item = (&'a str, &'a [u32], bool)>,
    ) -> Result<Self, String> {
        let mut marker = None;
        let mut services = BTreeMap::new();
        for (name, supported, available) in capabilities {
            if name.starts_with("product-") {
                let variant = match name {
                    CORE_MARKER => Variant::Core,
                    FULL_MARKER => Variant::Full,
                    _ => return Err("Unsupported agent product identity".into()),
                };
                if marker.replace(variant).is_some() || !available || supported != [VERSION] {
                    return Err("Ambiguous or unsupported agent product identity".into());
                }
            } else if services
                .insert(name.to_owned(), supported.to_vec())
                .is_some()
            {
                return Err("Duplicate agent service identity".into());
            }
        }
        let variant = match marker {
            Some(variant) => variant,
            None if services
                .keys()
                .all(|name| CORE_SERVICES.contains(&name.as_str())) =>
            {
                Variant::Core
            }
            // The first full development build predates product markers. It is
            // still full, and must never be treated as an unknown core build.
            None if services.contains_key("remote-apps") => Variant::Full,
            None => {
                return Err(
                    "Cannot identify this agent product; update its managing Newport client".into(),
                );
            }
        };
        let product = Self {
            variant,
            control_versions: control_versions.to_vec(),
            services,
        };
        product.validate()?;
        Ok(product)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Identity {
    pub schema_version: u32,
    pub build_sha256: String,
    pub product: Product,
}
impl Identity {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION
            || self.build_sha256.len() != 64
            || !self
                .build_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid agent product identity".into());
        }
        self.product.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn core() -> Vec<(&'static str, &'static [u32], bool)> {
        CORE_SERVICES
            .iter()
            .map(|name| (*name, &[1][..], false))
            .collect()
    }
    #[test]
    fn compiled_product_does_not_depend_on_host_or_account_availability() {
        let mut capabilities = core();
        assert_eq!(
            Product::from_capabilities(&[1], capabilities.clone())
                .unwrap()
                .variant,
            Variant::Core
        );
        capabilities.push((FULL_MARKER, &[1], true));
        capabilities.push(("remote-apps", &[1, 2], false));
        let full = Product::from_capabilities(&[1], capabilities).unwrap();
        assert_eq!(full.variant, Variant::Full);
        assert_eq!(full.services["remote-apps"], [1, 2]);
    }
    #[test]
    fn conflicting_unknown_and_incomplete_products_fail_closed() {
        for extra in [
            vec![("unknown", &[1][..], true)],
            vec![("product-future", &[1][..], true)],
            vec![(CORE_MARKER, &[2][..], true)],
            vec![(CORE_MARKER, &[1][..], false)],
            vec![(CORE_MARKER, &[1][..], true), (FULL_MARKER, &[1][..], true)],
            vec![
                (CORE_MARKER, &[1][..], true),
                ("remote-apps", &[1][..], true),
            ],
            vec![(FULL_MARKER, &[1][..], true)],
        ] {
            let mut capabilities = core();
            capabilities.extend(extra);
            assert!(Product::from_capabilities(&[1], capabilities).is_err());
        }
        assert!(Product::from_capabilities(&[1], core()[1..].iter().copied()).is_err());
        assert!(Product::from_capabilities(&[1, 1], core()).is_err());
        let mut legacy = core();
        legacy.push(("remote-apps", &[1], true));
        assert_eq!(
            Product::from_capabilities(&[1], legacy).unwrap().variant,
            Variant::Full
        );
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
