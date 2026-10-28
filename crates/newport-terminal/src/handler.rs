//! Newport's parser handler. Every sequence reaches Alacritty's `Term`; this
//! layer adds grapheme clustering and the modes `Term` does not implement:
//! reverse video, X10/urxvt/pixel mouse reporting, the 47/1047/1048 screen
//! and cursor modes, and xterm's modifyOtherKeys.
use crate::engine::Listener;
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::Dimensions,
    index::Point,
    term::{
        TermMode,
        cell::{Cell, Flags},
    },
    vte::ansi::{
        Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler as VtHandler, Hyperlink,
        KeyboardModes, KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys,
        PrivateMode, Rgb, ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
        cursor_icon::CursorIcon,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Modes that live beside Alacritty's `TermMode`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Modes {
    pub grapheme_clustering: bool,
    pub reverse_video: bool,
    pub x10_mouse: bool,
    pub urxvt_mouse: bool,
    pub pixel_mouse: bool,
    pub modify_other_keys: u8,
}
impl Default for Modes {
    fn default() -> Self {
        Self {
            grapheme_clustering: true,
            reverse_video: false,
            x10_mouse: false,
            urxvt_mouse: false,
            pixel_mouse: false,
            modify_other_keys: 0,
        }
    }
}

/// Parser-side state kept between chunks of output.
#[derive(Default)]
pub(crate) struct Extensions {
    pub modes: Modes,
    /// The grid changed outside `Term`'s damage tracking.
    pub redraw: bool,
    /// The cluster that the next printed character may extend. Its cell is
    /// the one before the cursor: any cursor movement ends the cluster.
    cluster: Option<Cluster>,
    /// The primary screen's saved cursor while mode 47 or 1047 is active.
    primary_saved: Option<alacritty_terminal::grid::Cursor<Cell>>,
}
impl Extensions {
    pub fn reset(&mut self) {
        *self = Self {
            redraw: true,
            ..Self::default()
        };
    }
    /// Cursor movement outside the parser, such as a resize, ends the cluster.
    pub fn end_cluster(&mut self) {
        self.cluster = None;
    }
}

struct Cluster {
    /// The cluster's text; empty while it is only `base`.
    text: String,
    base: char,
    /// The last character joins whatever follows it.
    joins_next: bool,
}
impl Cluster {
    fn start(&mut self, c: char) {
        self.base = c;
        self.text.clear();
        self.joins_next = joins_next(c);
    }
    /// The cluster's text with `c` appended.
    fn with(&mut self, c: char) -> &str {
        if self.text.is_empty() {
            self.text.push(self.base);
        }
        self.text.push(c);
        &self.text
    }
}

pub(crate) struct Handler<'a> {
    pub term: &'a mut Term<Listener>,
    pub ext: &'a mut Extensions,
    pub listener: &'a Listener,
}
impl Handler<'_> {
    fn reply(&self, text: String) {
        self.listener.send_event(Event::PtyWrite(text));
    }

    /// Extend the previous cluster with `c`, or start a new one. Returns false
    /// when `Term` should print `c` itself.
    fn extend_cluster(&mut self, c: char) -> bool {
        let Some(cluster) = self.ext.cluster.as_mut() else {
            return false;
        };
        if starts_cluster(c) && !cluster.joins_next {
            return false;
        }
        if cluster.with(c).graphemes(true).nth(1).is_some() {
            cluster.text.pop();
            return false;
        }
        cluster.joins_next = joins_next(c);
        let point = self.written_cell();
        if c.width() == Some(0) {
            // Term attaches zero-width characters to the cell before the cursor,
            // which is this cluster's cell while the cursor has not moved.
            VtHandler::input(self.term, c);
        } else {
            self.term.grid_mut()[point].push_zerowidth(c);
        }
        self.widen(point);
        self.ext.redraw = true;
        true
    }

    /// Give the cluster at `point` two cells once its text needs them.
    fn widen(&mut self, point: Point) {
        let Some(cluster) = self.ext.cluster.as_ref() else {
            return;
        };
        let wide = if cluster.text.is_empty() {
            cluster_width(cluster.base.encode_utf8(&mut [0; 4]))
        } else {
            cluster_width(&cluster.text)
        };
        if self.term.grid()[point].flags.contains(Flags::WIDE_CHAR) || wide < 2 {
            return;
        }
        let columns = self.term.columns();
        let grid = self.term.grid();
        let cursor = &grid.cursor;
        // Only widen in place: the cursor must sit right after the cluster.
        let after = cursor.point.line == point.line
            && !cursor.input_needs_wrap
            && cursor.point.column == point.column + 1;
        if point.column.0 + 1 >= columns || !after {
            return;
        }
        let insert = self.term.mode().contains(TermMode::INSERT);
        let grid = self.term.grid_mut();
        let spacer = Point::new(point.line, point.column + 1);
        if insert {
            let row = &mut grid[point.line][..];
            for column in (spacer.column.0..columns - 1).rev() {
                row.swap(column + 1, column);
            }
        }
        let template = grid.cursor.template.clone();
        grid[point].flags.insert(Flags::WIDE_CHAR);
        let cell = &mut grid[spacer];
        *cell = template;
        cell.c = ' ';
        cell.flags.insert(Flags::WIDE_CHAR_SPACER);
        if spacer.column.0 + 1 < columns {
            grid.cursor.point.column = spacer.column + 1;
        } else {
            grid.cursor.point.column = spacer.column;
            grid.cursor.input_needs_wrap = true;
        }
    }

    fn start_cluster(&mut self, c: char) {
        match &mut self.ext.cluster {
            Some(cluster) => cluster.start(c),
            None => {
                self.ext.cluster = Some(Cluster {
                    text: String::new(),
                    base: c,
                    joins_next: joins_next(c),
                })
            }
        }
    }

    /// The cell that `Term::input` just wrote a printable character into.
    fn written_cell(&self) -> Point {
        let cursor = &self.term.grid().cursor;
        let mut point = cursor.point;
        if !cursor.input_needs_wrap {
            point.column.0 = point.column.0.saturating_sub(1);
        }
        if self.term.grid()[point]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
        {
            point.column.0 = point.column.0.saturating_sub(1);
        }
        point
    }

    fn set_extension_mode(&mut self, mode: u16, enable: bool) -> bool {
        let modes = &mut self.ext.modes;
        match mode {
            5 => {
                modes.reverse_video = enable;
                self.ext.redraw = true;
            }
            9 => modes.x10_mouse = enable,
            1015 => modes.urxvt_mouse = enable,
            1016 => modes.pixel_mouse = enable,
            2027 => modes.grapheme_clustering = enable,
            47 | 1047 => self.alternate_screen(enable, mode == 1047),
            1048 if enable => VtHandler::save_cursor_position(self.term),
            1048 => VtHandler::restore_cursor_position(self.term),
            _ => return false,
        }
        true
    }

    /// Modes 47 and 1047 switch screens without the cursor save of 1049.
    fn alternate_screen(&mut self, enable: bool, clear_on_exit: bool) {
        let active = self.term.mode().contains(TermMode::ALT_SCREEN);
        if enable && !active {
            self.ext.primary_saved = Some(self.term.grid().saved_cursor.clone());
            self.term.swap_alt();
        } else if !enable && active {
            if clear_on_exit {
                VtHandler::clear_screen(self.term, ClearMode::All);
            }
            self.term.swap_alt();
            if let Some(saved) = self.ext.primary_saved.take() {
                self.term.grid_mut().saved_cursor = saved;
            }
        }
    }

    fn extension_mode_state(&self, mode: u16) -> Option<bool> {
        let modes = &self.ext.modes;
        Some(match mode {
            5 => modes.reverse_video,
            9 => modes.x10_mouse,
            1015 => modes.urxvt_mouse,
            1016 => modes.pixel_mouse,
            2027 => modes.grapheme_clustering,
            47 | 1047 => self.term.mode().contains(TermMode::ALT_SCREEN),
            _ => return None,
        })
    }
}

/// Columns a cluster occupies. Two for a wide character, an emoji
/// presentation selector, regional indicators, or more than one spacing
/// character (Indic conjuncts and spacing marks); otherwise one. Prepend
/// characters take no column of their own.
fn cluster_width(text: &str) -> usize {
    let wide = text.chars().any(|c| {
        c.width() == Some(2) || c == '\u{fe0f}' || ('\u{1f1e6}'..='\u{1f1ff}').contains(&c)
    });
    let spacing = text
        .chars()
        .filter(|&c| !is_prepend(c) && c.width().is_some_and(|width| width > 0))
        .count();
    if wide || spacing > 1 { 2 } else { 1 }
}

/// Characters that never continue a cluster on their own: ASCII, Latin
/// letters, CJK ideographs, kana and Hangul syllables.
fn starts_cluster(c: char) -> bool {
    c.is_ascii()
        || matches!(
            c,
            '\u{a0}'..='\u{2ff}'
                | '\u{3040}'..='\u{30ff}'
                | '\u{3400}'..='\u{4dbf}'
                | '\u{4e00}'..='\u{9fff}'
                | '\u{ac00}'..='\u{d7a3}'
        )
}

/// Characters that join whatever follows them: ZWJ, Prepend, a regional
/// indicator awaiting its pair, and Hangul leading consonants.
fn joins_next(c: char) -> bool {
    c == '\u{200d}'
        || is_prepend(c)
        || ('\u{1f1e6}'..='\u{1f1ff}').contains(&c)
        || matches!(c, '\u{1100}'..='\u{115f}' | '\u{a960}'..='\u{a97f}')
}

/// Grapheme_Cluster_Break=Prepend: joins the character that follows it.
fn is_prepend(c: char) -> bool {
    matches!(
        c,
        '\u{600}'..='\u{605}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{d4e}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{111c2}'..='\u{111c3}'
            | '\u{1193f}'
            | '\u{11941}'
            | '\u{11a3a}'
            | '\u{11a84}'..='\u{11a89}'
            | '\u{11d46}'
            | '\u{11f02}'
    )
}

fn private_mode_number(mode: &PrivateMode) -> u16 {
    match mode {
        PrivateMode::Named(named) => *named as u16,
        PrivateMode::Unknown(number) => *number,
    }
}

// The parser's handler trait gives every method an empty default, so a method
// missing here would be silently dropped. List every method of the pinned
// `vte` handler in `forward!`, `forward_within_cluster!` or the overrides.
macro_rules! forward {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name(&mut self, $($arg: $ty),*) {
                self.ext.cluster = None;
                VtHandler::$name(self.term, $($arg),*)
            }
        )*
    };
}
/// Sequences that neither move the cursor nor edit the grid, so a cluster
/// still being received continues across them.
macro_rules! forward_within_cluster {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name(&mut self, $($arg: $ty),*) {
                VtHandler::$name(self.term, $($arg),*)
            }
        )*
    };
}

impl VtHandler for Handler<'_> {
    fn input(&mut self, c: char) {
        let clustering = self.ext.modes.grapheme_clustering;
        // Printable ASCII starts a cluster unless the previous character joins it.
        if c.is_ascii()
            && !self
                .ext
                .cluster
                .as_ref()
                .is_some_and(|cluster| cluster.joins_next)
        {
            VtHandler::input(self.term, c);
            if c.is_ascii_graphic() || c == ' ' {
                self.start_cluster(c);
            }
            return;
        }
        if clustering && self.extend_cluster(c) {
            return;
        }
        VtHandler::input(self.term, c);
        if c.width().is_some_and(|width| width > 0) {
            self.start_cluster(c);
            // A lone regional indicator still renders as a two-cell flag.
            if self.ext.modes.grapheme_clustering && ('\u{1f1e6}'..='\u{1f1ff}').contains(&c) {
                self.widen(self.written_cell());
                self.ext.redraw = true;
            }
        }
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        match intermediate {
            // VT220 with ANSI color.
            None => self.reply("\x1b[?62;22c".into()),
            _ => VtHandler::identify_terminal(self.term, intermediate),
        }
    }

    fn reset_state(&mut self) {
        self.ext.reset();
        VtHandler::reset_state(self.term);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        self.ext.cluster = None;
        if !self.set_extension_mode(private_mode_number(&mode), true) {
            VtHandler::set_private_mode(self.term, mode);
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        self.ext.cluster = None;
        if !self.set_extension_mode(private_mode_number(&mode), false) {
            VtHandler::unset_private_mode(self.term, mode);
        }
    }

    fn report_private_mode(&mut self, mode: PrivateMode) {
        let number = private_mode_number(&mode);
        match self.extension_mode_state(number) {
            Some(enabled) => {
                self.reply(format!("\x1b[?{number};{}$y", if enabled { 1 } else { 2 }))
            }
            None => VtHandler::report_private_mode(self.term, mode),
        }
    }

    fn set_modify_other_keys(&mut self, mode: ModifyOtherKeys) {
        self.ext.modes.modify_other_keys = match mode {
            ModifyOtherKeys::Reset => 0,
            ModifyOtherKeys::EnableExceptWellDefined => 1,
            ModifyOtherKeys::EnableAll => 2,
        };
    }

    fn report_modify_other_keys(&mut self) {
        self.reply(format!("\x1b[>4;{}m", self.ext.modes.modify_other_keys));
    }

    forward_within_cluster! {
        set_title(title: Option<String>);
        set_cursor_style(style: Option<CursorStyle>);
        set_cursor_shape(shape: CursorShape);
        device_status(arg: usize);
        bell();
        terminal_attribute(attr: Attr);
        report_mode(mode: Mode);
        set_keypad_application_mode();
        unset_keypad_application_mode();
        set_active_charset(index: CharsetIndex);
        configure_charset(index: CharsetIndex, charset: StandardCharset);
        set_color(index: usize, color: Rgb);
        dynamic_color_sequence(prefix: String, index: usize, terminator: &str);
        reset_color(index: usize);
        clipboard_store(clipboard: u8, base64: &[u8]);
        clipboard_load(clipboard: u8, terminator: &str);
        push_title();
        pop_title();
        text_area_size_pixels();
        text_area_size_chars();
        set_hyperlink(hyperlink: Option<Hyperlink>);
        set_mouse_cursor_icon(icon: CursorIcon);
        report_keyboard_mode();
        push_keyboard_mode(mode: KeyboardModes);
        pop_keyboard_modes(to_pop: u16);
        set_keyboard_mode(mode: KeyboardModes, behavior: KeyboardModesApplyBehavior);
        set_scp(char_path: ScpCharPath, update_mode: ScpUpdateMode);
    }

    forward! {
        goto(line: i32, col: usize);
        goto_line(line: i32);
        goto_col(col: usize);
        insert_blank(count: usize);
        move_up(count: usize);
        move_down(count: usize);
        move_forward(col: usize);
        move_backward(col: usize);
        move_down_and_cr(row: usize);
        move_up_and_cr(row: usize);
        put_tab(count: u16);
        backspace();
        carriage_return();
        linefeed();
        substitute();
        newline();
        set_horizontal_tabstop();
        scroll_up(count: usize);
        scroll_down(count: usize);
        insert_blank_lines(count: usize);
        delete_lines(count: usize);
        erase_chars(count: usize);
        delete_chars(count: usize);
        move_backward_tabs(count: u16);
        move_forward_tabs(count: u16);
        save_cursor_position();
        restore_cursor_position();
        clear_line(mode: LineClearMode);
        clear_screen(mode: ClearMode);
        clear_tabs(mode: TabulationClearMode);
        set_tabs(interval: u16);
        reverse_index();
        set_mode(mode: Mode);
        unset_mode(mode: Mode);
        set_scrolling_region(top: usize, bottom: Option<usize>);
        decaln();
    }
}

#[cfg(test)]
mod tests {
    use super::cluster_width;

    #[test]
    fn cluster_widths_follow_wide_characters_and_presentation() {
        for (text, width) in [
            ("a", 1),
            ("é", 1),
            ("中", 2),
            ("👩‍💻", 2),
            ("❤\u{fe0f}", 2),
            ("❤", 1),
            ("🇺🇸", 2),
            ("🇬", 2),
            ("क्षि", 2),
            ("का", 2),
            ("\u{600}a", 1),
            ("\u{600}界", 2),
        ] {
            assert_eq!(cluster_width(text), width, "{text:?}");
        }
    }
}
