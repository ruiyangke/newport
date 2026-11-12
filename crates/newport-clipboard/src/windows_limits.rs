//! Native representations have a separate budget from transferred bytes.
use std::collections::BTreeMap;

pub(super) const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const FORMAT_OVERHEAD: usize = 4096;

pub(super) const fn publication_limit(wire_limit: usize) -> usize {
    // LF becomes UTF-16 CRLF; PNG also supplies one decoded DIB representation.
    wire_limit
        .saturating_mul(4)
        .saturating_add(MAX_IMAGE_BYTES)
        .saturating_add(FORMAT_OVERHEAD)
}

#[derive(Default)]
pub(super) struct NativeBudget {
    formats: BTreeMap<u32, usize>,
    bytes: usize,
}
impl NativeBudget {
    pub(super) fn permits(&self, format: u32, bytes: usize, limit: usize) -> bool {
        self.bytes
            .checked_sub(self.formats.get(&format).copied().unwrap_or(0))
            .and_then(|used| used.checked_add(bytes))
            .is_some_and(|total| total <= limit)
    }
    pub(super) fn record(&mut self, format: u32, bytes: usize) {
        self.bytes -= self.formats.insert(format, bytes).unwrap_or(0);
        self.bytes += bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_budget_counts_formats_together_and_replacements_once() {
        let mut budget = NativeBudget::default();
        assert!(budget.permits(1, 6, 10));
        budget.record(1, 6);
        assert!(!budget.permits(2, 5, 10));
        assert!(budget.permits(2, 4, 10));
        budget.record(2, 4);
        assert!(budget.permits(1, 6, 10));
        assert!(!budget.permits(1, 7, 10));
        budget.record(1, 3);
        assert!(budget.permits(3, 3, 10));
        assert!(!budget.permits(3, usize::MAX, usize::MAX));
    }
    #[test]
    fn native_budget_allows_text_expansion_and_an_image_without_changing_wire_limits() {
        let wire = 8 * 1024 * 1024;
        assert!(publication_limit(wire) >= wire * 4 + MAX_IMAGE_BYTES);
        assert_eq!(publication_limit(usize::MAX), usize::MAX);
    }
}
