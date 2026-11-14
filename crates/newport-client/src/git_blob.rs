//! Snapshot-bound immutable Git blob pages, independent of presentation.
use newport_protocol::git::{BlobFragment, BlobMetadata, Page};
use std::sync::Arc;
#[derive(Default)]
pub struct Content {
    decoded: usize,
    pub oid: String,
    pub bytes: Vec<u8>,
    pub snapshot: String,
    pub size: Option<u64>,
    pub cursor: Option<String>,
    pub binary: bool,
    pub text: Option<Arc<String>>,
}
impl Content {
    pub fn append(&mut self, page: &Page<BlobFragment, BlobMetadata>) -> Result<(), String> {
        let invalid =
            || "Conflict preview response does not match the requested content.".to_owned();
        let snapshot = page.snapshot.as_str();
        let size = page.metadata.size;
        if snapshot.is_empty()
            || (self.size.is_some() && self.cursor.is_none())
            || page.metadata.oid.hex != self.oid
            || (!self.snapshot.is_empty() && self.snapshot != snapshot)
            || self.size.is_some_and(|n| n != size)
        {
            return Err(invalid());
        }
        let mut chunk = Vec::new();
        for entry in &page.entries {
            if entry.offset != (self.bytes.len() + chunk.len()) as u64 {
                return Err(invalid());
            }
            chunk.extend_from_slice(&entry.bytes_b64.0);
            if chunk.len() > 65536 {
                return Err(invalid());
            }
        }
        let next = match &page.next_cursor {
            None => None,
            Some(s) if !s.is_empty() => Some(s.clone()),
            Some(_) => return Err(invalid()),
        };
        let end = (self.bytes.len() + chunk.len()) as u64;
        if end > size
            || (next.is_none() && end != size)
            || (next.is_some() && (chunk.is_empty() || end >= size || next == self.cursor))
        {
            return Err(invalid());
        }
        self.bytes.extend(chunk);
        self.snapshot = snapshot.into();
        self.size = Some(size);
        self.cursor = next;
        // Decode only the new suffix, retaining an unfinished UTF-8 sequence
        // until the next page. Invalid bytes display as replacement
        // characters; NUL marks binary content.
        if self.bytes[self.decoded..].contains(&0) {
            self.binary = true;
        }
        if self.binary {
            self.text = None;
            return Ok(());
        }
        let mut suffix = String::new();
        while self.decoded < self.bytes.len() {
            match std::str::from_utf8(&self.bytes[self.decoded..]) {
                Ok(text) => {
                    suffix.push_str(text);
                    self.decoded = self.bytes.len();
                }
                Err(error) => {
                    let end = self.decoded + error.valid_up_to();
                    suffix.push_str(
                        std::str::from_utf8(&self.bytes[self.decoded..end])
                            .expect("validated UTF-8 prefix"),
                    );
                    self.decoded = end;
                    if let Some(length) = error.error_len() {
                        suffix.push('�');
                        self.decoded += length;
                    } else if self.cursor.is_some() {
                        break;
                    } else {
                        suffix.push('�');
                        self.decoded = self.bytes.len();
                    }
                }
            }
        }
        let text = self.text.get_or_insert_with(|| Arc::new(String::new()));
        if !suffix.is_empty() {
            Arc::make_mut(text).push_str(&suffix);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git::{Binary, ObjectId};
    type BlobPage = Page<BlobFragment, BlobMetadata>;
    fn page(oid: &str, bytes: &[u8], offset: usize, size: usize, cursor: Option<&str>) -> BlobPage {
        Page {
            snapshot: "snapshot".into(),
            entries: vec![BlobFragment {
                offset: offset as u64,
                bytes_b64: Binary(bytes.to_vec()),
            }],
            next_cursor: cursor.map(Into::into),
            metadata: BlobMetadata {
                oid: ObjectId {
                    algorithm: "sha1".into(),
                    hex: oid.into(),
                },
                size: size as u64,
            },
        }
    }
    #[test]
    fn invalid_utf8_matches_text_decoder_but_nul_marks_binary() {
        let oid = "a".repeat(40);
        let mut value = Content {
            oid: oid.clone(),
            ..Default::default()
        };
        value
            .append(&page(&oid, b"a\xff\xe2", 0, 4, Some("next")))
            .unwrap();
        assert_eq!(value.text.as_ref().unwrap().as_str(), "a�");
        value.append(&page(&oid, b"z", 3, 4, None)).unwrap();
        assert_eq!(value.text.as_ref().unwrap().as_str(), "a��z");
        assert!(!value.binary);
        assert!(value.append(&page(&oid, b"", 4, 4, None)).is_err());
        let mut value = Content {
            oid: oid.clone(),
            ..Default::default()
        };
        value.append(&page(&oid, b"\xe2", 0, 1, None)).unwrap();
        assert_eq!(value.text.as_ref().unwrap().as_str(), "�");
    }
    #[test]
    fn malformed_pages_do_not_mutate_content_and_repaints_reuse_decoding() {
        let oid = "a".repeat(40);
        let mut value = Content {
            oid: oid.clone(),
            ..Default::default()
        };
        value.append(&page(&oid, b"a", 0, 3, Some("next"))).unwrap();
        let previous = value.text.clone().unwrap();
        for candidate in [
            page(&oid, b"b", 1, 4, None),
            page(&oid, b"b", 1, 3, Some("next")),
            page(&oid, b"b", 1, 3, None),
        ] {
            assert!(value.append(&candidate).is_err());
            assert_eq!(value.bytes, b"a");
            assert!(Arc::ptr_eq(&previous, value.text.as_ref().unwrap()));
        }
        value.append(&page(&oid, b"\0b", 1, 3, None)).unwrap();
        assert!(value.binary && value.text.is_none());
    }
    #[test]
    fn conflict_pages_preserve_split_utf8_and_reject_wrong_identity_or_offsets() {
        let oid = "a".repeat(40);
        let mut preview = Content {
            oid: oid.clone(),
            ..Default::default()
        };
        let bytes = "a😀z".as_bytes();
        preview
            .append(&page(&oid, &bytes[..3], 0, bytes.len(), Some("next")))
            .unwrap();
        assert!(!preview.binary);
        assert_eq!(preview.text.as_ref().unwrap().as_str(), "a");
        let mut wrong = page(&oid, &bytes[3..], 3, bytes.len(), None);
        wrong.metadata.oid.hex = "b".repeat(40);
        assert!(preview.append(&wrong).is_err());
        assert_eq!(preview.bytes, bytes[..3]);
        assert!(
            preview
                .append(&page(&oid, &bytes[3..], 2, bytes.len(), None))
                .is_err()
        );
        preview
            .append(&page(&oid, &bytes[3..], 3, bytes.len(), None))
            .unwrap();
        assert_eq!(preview.text.as_ref().unwrap().as_str(), "a😀z");
        assert!(preview.cursor.is_none());
        let mut binary = Content {
            oid: oid.clone(),
            ..Default::default()
        };
        binary.append(&page(&oid, b"a\0b", 0, 3, None)).unwrap();
        assert!(binary.binary);
    }
}
