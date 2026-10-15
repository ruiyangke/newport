//! Display-only tab expansion. Selection and highlights retain source byte offsets.
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

pub(crate) struct TabText {
    pub text: String,
    tabs: Vec<Tab>,
}
struct Tab {
    source: usize,
    display: Range<usize>,
}
impl TabText {
    pub fn new(source: &str, size: usize) -> Self {
        let size = size.clamp(1, 16);
        let mut result = Self {
            text: String::with_capacity(source.len()),
            tabs: vec![],
        };
        let mut start = 0;
        let mut column = 0;
        for (offset, _) in source.match_indices('\t') {
            let segment = &source[start..offset];
            result.text.push_str(segment);
            column += segment.width();
            let spaces = size - column % size;
            let display_start = result.text.len();
            result.text.extend(std::iter::repeat_n(' ', spaces));
            result.tabs.push(Tab {
                source: offset,
                display: display_start..result.text.len(),
            });
            column += spaces;
            start = offset + 1;
        }
        result.text.push_str(&source[start..]);
        result
    }
    pub fn display_offset(&self, source: usize) -> usize {
        let count = self.tabs.partition_point(|tab| tab.source < source);
        source
            + count
                .checked_sub(1)
                .map_or(0, |i| self.tabs[i].display.end - (self.tabs[i].source + 1))
    }
    pub fn source_offset(&self, display: usize) -> usize {
        let count = self.tabs.partition_point(|tab| tab.display.end <= display);
        if let Some(tab) = self.tabs.get(count)
            && display >= tab.display.start
        {
            // Snap either half of an expanded tab to an actual source boundary.
            return tab.source
                + usize::from((display - tab.display.start) * 2 >= tab.display.len());
        }
        display
            - count
                .checked_sub(1)
                .map_or(0, |i| self.tabs[i].display.end - (self.tabs[i].source + 1))
    }
    pub fn display_range(&self, source: Range<usize>) -> Range<usize> {
        self.display_offset(source.start)..self.display_offset(source.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_stops_use_display_columns_and_preserve_utf8_boundaries() {
        for (source, expected) in [
            ("a\tb\t", "a   b   "),
            ("1234\tx", "1234    x"),
            ("雪\tCafé\tx", "雪  Café    x"),
            ("e\u{301}\t🦀\tx", "e\u{301}   🦀  x"),
            ("\t\tx", "        x"),
        ] {
            let view = TabText::new(source, 4);
            assert_eq!(view.text, expected);
            for at in source
                .char_indices()
                .map(|(at, _)| at)
                .chain([source.len()])
            {
                let display = view.display_offset(at);
                assert!(view.text.is_char_boundary(display));
                assert_eq!(view.source_offset(display), at);
            }
        }
        let view = TabText::new("a\tb", 4);
        assert_eq!(view.source_offset(2), 1);
        assert_eq!(view.source_offset(3), 2);
        assert_eq!(view.display_range(1..2), 1..4);
        assert_eq!(view.display_range(2..3), 4..5);
    }
}
