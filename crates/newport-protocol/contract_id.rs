//! Build-time identities for the complete schema used by an admission boundary.
use prost::Message;
use prost_types::FileDescriptorSet;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Unnamed,
    Duplicate(String),
    Missing(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unnamed => f.write_str("Contract descriptor has no filename"),
            Self::Duplicate(name) => write!(f, "Contract descriptor repeats {name}"),
            Self::Missing(name) => write!(f, "Contract descriptor is missing {name}"),
        }
    }
}
impl std::error::Error for Error {}

/// Hash the named roots and all their imports, independent of file order and comments.
pub fn fingerprint(descriptors: &FileDescriptorSet, roots: &[&str]) -> Result<String, Error> {
    let mut files = BTreeMap::new();
    for file in &descriptors.file {
        let name = file
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .ok_or(Error::Unnamed)?;
        if files.insert(name, file).is_some() {
            return Err(Error::Duplicate(name.into()));
        }
    }
    let mut selected = BTreeSet::new();
    let mut pending: Vec<&str> = roots.to_vec();
    while let Some(name) = pending.pop() {
        let file = files.get(name).ok_or_else(|| Error::Missing(name.into()))?;
        if selected.insert(name) {
            pending.extend(file.dependency.iter().map(String::as_str));
        }
    }
    let mut canonical = FileDescriptorSet::default();
    for name in selected {
        let mut file = (*files.get(name).ok_or_else(|| Error::Missing(name.into()))?).clone();
        file.source_code_info = None;
        canonical.file.push(file);
    }
    Ok(Sha256::digest(canonical.encode_to_vec())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
