//! Only the core X11 requests needed by clipboard programs. No rendering
//! backend. CLIPBOARD and PRIMARY both offer the computer's clipboard, so a
//! plain `xclip -o` and a middle-click paste read it too. On a display that
//! takes copies, the server takes what a program puts on CLIPBOARD for the
//! computer, then gives the selection back to the computer's clipboard.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};

const CLIENT_PROPERTY_LIMIT: usize = 64 * 1024;
const PROPERTY_COUNT_LIMIT: usize = 128;
// One maximum-size clipboard plus the existing client negotiation allowance.
const PROPERTY_BYTES_LIMIT: usize =
    crate::snapshot::LIMIT + PROPERTY_COUNT_LIMIT * CLIENT_PROPERTY_LIMIT;
const OWNER: u32 = 2;
/// The server's own window, on which it asks owners for what they copied.
const REQUESTOR: u32 = 3;
/// The property owners answer on, on `REQUESTOR`.
const ANSWER: &str = "NEWPORT_SELECTION";
/// The text targets offered for the computer's text, beside `STRING`.
const TEXT_NAMES: [&str; 5] = [
    "TARGETS",
    "UTF8_STRING",
    "TEXT",
    "text/plain;charset=utf-8",
    "text/plain",
];
/// Text targets an owner may serve, preferred first.
const TEXT_TARGETS: [&str; 4] = [
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain",
    "STRING",
];

#[derive(Clone, Copy)]
pub struct Order(pub bool);
impl Order {
    fn u16(self, b: &[u8], at: usize) -> u16 {
        let b = [b[at], b[at + 1]];
        if self.0 {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    }
    fn u32(self, b: &[u8], at: usize) -> u32 {
        let b = [b[at], b[at + 1], b[at + 2], b[at + 3]];
        if self.0 {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }
    fn set16(self, b: &mut [u8], at: usize, n: u16) {
        b[at..at + 2].copy_from_slice(&if self.0 {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        });
    }
    fn set32(self, b: &mut [u8], at: usize, n: u32) {
        b[at..at + 4].copy_from_slice(&if self.0 {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        });
    }
    fn words(self, words: impl IntoIterator<Item = u32>) -> Vec<u8> {
        words
            .into_iter()
            .flat_map(|n| {
                if self.0 {
                    n.to_le_bytes()
                } else {
                    n.to_be_bytes()
                }
            })
            .collect()
    }
}
/// The target the first selection to take is asked for.
fn taking_target(taking: &VecDeque<Taking>) -> &'static str {
    taking.front().map_or("", |taking| taking.targets[0])
}
/// STRING is Latin-1 by convention, but programs often serve UTF-8 there.
fn latin1(bytes: Vec<u8>) -> Vec<u8> {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| error.into_bytes().into_iter().map(char::from).collect())
        .into_bytes()
}
/// The computer's UTF-8 text as STRING, which is Latin-1 (ICCCM): characters
/// outside it become `?`. Text that isn't UTF-8 goes as it is.
fn to_latin1(bytes: Vec<u8>) -> Vec<u8> {
    match String::from_utf8(bytes) {
        Ok(text) => text
            .chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
            .collect(),
        Err(error) => error.into_bytes(),
    }
}
/// The selections that offer the computer's clipboard.
fn offered(selection: &str) -> bool {
    matches!(selection, "CLIPBOARD" | "PRIMARY")
}
fn padded(n: usize) -> usize {
    (n + 3) & !3
}

pub struct Atoms(Vec<String>);
impl Default for Atoms {
    fn default() -> Self {
        // The protocol's predefined atoms keep their fixed IDs, in this order.
        // The text names come first after them: xsel asks for UTF8_STRING only
        // when that atom already exists, and STRING otherwise.
        Self("PRIMARY SECONDARY ARC ATOM BITMAP CARDINAL COLORMAP CURSOR CUT_BUFFER0 CUT_BUFFER1 CUT_BUFFER2 CUT_BUFFER3 CUT_BUFFER4 CUT_BUFFER5 CUT_BUFFER6 CUT_BUFFER7 DRAWABLE FONT INTEGER PIXMAP POINT RECTANGLE RESOURCE_MANAGER RGB_COLOR_MAP RGB_BEST_MAP RGB_BLUE_MAP RGB_DEFAULT_MAP RGB_GRAY_MAP RGB_GREEN_MAP RGB_RED_MAP STRING VISUALID WINDOW WM_COMMAND WM_HINTS WM_CLIENT_MACHINE WM_ICON_NAME WM_ICON_SIZE WM_NAME WM_NORMAL_HINTS WM_SIZE_HINTS WM_ZOOM_HINTS MIN_SPACE NORM_SPACE MAX_SPACE END_SPACE SUPERSCRIPT_X SUPERSCRIPT_Y SUBSCRIPT_X SUBSCRIPT_Y UNDERLINE_POSITION UNDERLINE_THICKNESS STRIKEOUT_ASCENT STRIKEOUT_DESCENT ITALIC_ANGLE X_HEIGHT QUAD_WIDTH WEIGHT POINT_SIZE RESOLUTION COPYRIGHT NOTICE FONT_NAME FAMILY_NAME FULL_NAME CAP_HEIGHT WM_CLASS WM_TRANSIENT_FOR".split_whitespace().map(str::to_owned).chain(TEXT_NAMES.map(str::to_owned)).collect())
    }
}
impl Atoms {
    fn intern(&mut self, name: &str, only: bool) -> u32 {
        if let Some(i) = self.0.iter().position(|n| n == name) {
            return i as u32 + 1;
        }
        if only || self.0.len() >= 4096 {
            return 0;
        }
        self.0.push(name.to_owned());
        self.0.len() as u32
    }
    fn name(&self, id: u32) -> Option<&str> {
        id.checked_sub(1)
            .and_then(|i| self.0.get(i as usize))
            .map(String::as_str)
    }
}

pub struct Shared {
    pub atoms: Mutex<Atoms>,
    pub(crate) source: crate::clipboard_source::Source,
}
#[derive(Clone)]
struct Property {
    kind: u32,
    format: u8,
    bytes: Vec<u8>,
}
struct Client {
    socket: UnixStream,
    order: Order,
    seq: u16,
    gui_notified: bool,
    base: u32,
    shared: Arc<Shared>,
    windows: HashSet<u32>,
    graphics_contexts: HashSet<u32>,
    masks: HashMap<u32, u32>,
    properties: HashMap<(u32, u32), Property>,
    /// The window that owns each selection this client set, by atom, until
    /// the server has taken it.
    owned: HashMap<u32, u32>,
    /// Selections to take from this client, one at a time.
    taking: VecDeque<Taking>,
    /// Selections this client was told it lost.
    cleared: HashSet<u32>,
}

/// A selection the server takes from the program that set it. CLIPBOARD's
/// text goes to the computer; a program's PRIMARY never leaves the server,
/// so only its owner's answer is awaited before giving it back.
struct Taking {
    selection: u32,
    owner: u32,
    time: u32,
    exported: bool,
    /// The target asked for, then those to try if the owner refuses it.
    targets: Vec<&'static str>,
    asked: bool,
    /// What arrived so far, from an owner that sends it in parts (INCR).
    parts: Option<Vec<u8>>,
}

/// X11 setup with one tiny logical screen. Nothing is ever drawn or mapped.
fn setup(o: Order, base: u32) -> Vec<u8> {
    let vendor = b"Newport clipboard";
    let mut b = vec![0; 8 + 32 + padded(vendor.len()) + 8 + 40 + 8 + 24];
    b[0] = 1;
    o.set16(&mut b, 2, 11);
    let extra = ((b.len() - 8) / 4) as u16;
    o.set16(&mut b, 6, extra);
    o.set32(&mut b, 8, 1);
    o.set32(&mut b, 12, base);
    o.set32(&mut b, 16, 0x000f_ffff);
    o.set16(&mut b, 24, vendor.len() as u16);
    o.set16(&mut b, 26, u16::MAX);
    b[28] = 1;
    b[29] = 1;
    b[30] = 0;
    b[31] = 0;
    b[32] = 32;
    b[33] = 32;
    b[34] = 8;
    b[35] = 255;
    b[40..40 + vendor.len()].copy_from_slice(vendor);
    let f = 40 + padded(vendor.len());
    b[f] = 24;
    b[f + 1] = 32;
    b[f + 2] = 32;
    let s = f + 8;
    o.set32(&mut b, s, 1);
    o.set32(&mut b, s + 4, 3);
    o.set32(&mut b, s + 8, 0xffffff);
    for offset in [20, 22, 24, 26, 28, 30] {
        o.set16(&mut b, s + offset, 1);
    }
    o.set32(&mut b, s + 32, 4);
    b[s + 38] = 24;
    b[s + 39] = 1;
    let d = s + 40;
    b[d] = 24;
    o.set16(&mut b, d + 2, 1);
    let v = d + 8;
    o.set32(&mut b, v, 4);
    b[v + 4] = 4;
    b[v + 5] = 8;
    o.set16(&mut b, v + 6, 256);
    o.set32(&mut b, v + 8, 0xff0000);
    o.set32(&mut b, v + 12, 0xff00);
    o.set32(&mut b, v + 16, 0xff);
    b
}

/// Serve one client. With a `cookie`, the client must present it; without,
/// the listener already knows the client (see `x_server`).
pub fn serve(
    mut socket: UnixStream,
    shared: Arc<Shared>,
    cookie: Option<[u8; 16]>,
    base: u32,
) -> io::Result<()> {
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut h = [0; 12];
    socket.read_exact(&mut h)?;
    let o = match h[0] {
        b'l' => Order(true),
        b'B' => Order(false),
        _ => return Err(io::Error::other("invalid byte order")),
    };
    let name = o.u16(&h, 6) as usize;
    let data = o.u16(&h, 8) as usize;
    if name > 64 || data > 64 || o.u16(&h, 2) != 11 {
        return Err(io::Error::other("invalid setup"));
    }
    let mut auth = vec![0; padded(name) + padded(data)];
    socket.read_exact(&mut auth)?;
    if cookie.is_some_and(|cookie| {
        &auth[..name] != b"MIT-MAGIC-COOKIE-1"
            || data != 16
            || auth[padded(name)..padded(name) + data] != cookie
    }) {
        let reason = b"Authentication required";
        let mut b = vec![0; 8 + padded(reason.len())];
        b[1] = reason.len() as u8;
        o.set16(&mut b, 2, 11);
        o.set16(&mut b, 6, (padded(reason.len()) / 4) as u16);
        b[8..8 + reason.len()].copy_from_slice(reason);
        socket.write_all(&b)?;
        return Err(io::Error::other("authentication failed"));
    }
    socket.write_all(&setup(o, base))?;
    // Idle clipboard clients can stay connected. Admission is capped by the listener.
    socket.set_read_timeout(None)?;
    let mut c = Client {
        socket,
        order: o,
        seq: 0,
        gui_notified: false,
        base,
        shared,
        windows: HashSet::new(),
        graphics_contexts: HashSet::new(),
        masks: HashMap::new(),
        properties: HashMap::new(),
        owned: HashMap::new(),
        taking: VecDeque::new(),
        cleared: HashSet::new(),
    };
    loop {
        let mut h = [0; 4];
        c.socket.read_exact(&mut h)?;
        let size = o.u16(&h, 2) as usize * 4;
        if size < 4 {
            return Err(io::Error::other("BIG-REQUESTS is not supported"));
        }
        let mut req = vec![0; size];
        req[..4].copy_from_slice(&h);
        c.socket.read_exact(&mut req[4..])?;
        c.seq = c.seq.wrapping_add(1);
        c.request(&req)?;
    }
}

impl Client {
    fn packet(&self, tag: u8, detail: u8) -> Vec<u8> {
        let mut b = vec![0; 32];
        b[0] = tag;
        b[1] = detail;
        self.order.set16(&mut b, 2, self.seq);
        b
    }
    fn send(&mut self, b: &[u8]) -> io::Result<()> {
        self.socket.write_all(b)
    }
    fn error(&mut self, code: u8, op: u8, value: u32) -> io::Result<()> {
        let mut b = self.packet(0, code);
        self.order.set32(&mut b, 4, value);
        b[10] = op;
        self.send(&b)
    }
    fn atom(&self, name: &str) -> u32 {
        self.shared
            .atoms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .intern(name, false)
    }
    fn notify_property(&mut self, key: (u32, u32), state: u8) -> io::Result<()> {
        if self.masks.get(&key.0).copied().unwrap_or(0) & (1 << 22) == 0 {
            return Ok(());
        }
        let mut b = self.packet(28, 0);
        self.order.set32(&mut b, 4, key.0);
        self.order.set32(&mut b, 8, key.1);
        b[16] = state;
        self.send(&b)
    }
    fn delete(&mut self, key: (u32, u32)) -> io::Result<()> {
        if self.properties.remove(&key).is_some() {
            self.notify_property(key, 1)?;
        }
        Ok(())
    }
    fn property_fits(&self, key: (u32, u32), size: usize) -> bool {
        let retained = self.properties.iter().filter(|(k, _)| **k != key);
        let (count, bytes) = retained.fold((0, 0), |(count, bytes), (_, p)| {
            (count + 1, bytes + p.bytes.len())
        });
        count < PROPERTY_COUNT_LIMIT && size <= PROPERTY_BYTES_LIMIT.saturating_sub(bytes)
    }
    fn selection(&mut self, r: &[u8]) -> io::Result<()> {
        let o = self.order;
        let window = o.u32(r, 4);
        let selection = o.u32(r, 8);
        let target = o.u32(r, 12);
        let mut property = o.u32(r, 16);
        if property == 0 {
            property = target;
        }
        if !self.windows.contains(&window) {
            return self.error(3, 24, window);
        }
        let (selection_name, target_name) = {
            let a = self
                .shared
                .atoms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                a.name(selection).unwrap_or("").to_owned(),
                a.name(target).unwrap_or("").to_owned(),
            )
        };
        let (revision, formats) = if offered(&selection_name) {
            self.shared.source.offer().unwrap_or_else(|error| {
                self.shared
                    .source
                    .event("x11_snapshot_failed", &format!("kind={:?}", error.kind()));
                Default::default()
            })
        } else {
            Default::default()
        };
        let value = if formats.is_empty() {
            None
        } else if target_name == "TARGETS" {
            let mut ids = vec![self.atom("TARGETS"), self.atom("TIMESTAMP")];
            ids.extend(formats.keys().map(|s| self.atom(s)));
            if formats.contains_key("text/plain") {
                ids.extend(
                    [
                        "UTF8_STRING",
                        "STRING",
                        "TEXT",
                        "text/plain;charset=utf-8",
                        "text/plain; charset=utf-8",
                    ]
                    .map(|s| self.atom(s)),
                );
            }
            Some(Property {
                kind: 4,
                format: 32,
                bytes: o.words(ids),
            })
        } else if target_name == "TIMESTAMP" {
            Some(Property {
                kind: 19,
                format: 32,
                bytes: o.words([0]),
            })
        } else {
            let name = match target_name.as_str() {
                "UTF8_STRING"
                | "STRING"
                | "TEXT"
                | "text/plain;charset=utf-8"
                | "text/plain; charset=utf-8" => "text/plain",
                s => s,
            };
            self.shared
                .source
                .get(revision, name)
                .ok()
                .map(|bytes| match target_name.as_str() {
                    "STRING" => to_latin1(bytes),
                    _ => bytes,
                })
                .map(|bytes| Property {
                    kind: if target_name == "TEXT" {
                        self.atom("UTF8_STRING")
                    } else {
                        target
                    },
                    format: 8,
                    bytes,
                })
        };
        let key = (window, property);
        let fits = value
            .as_ref()
            .is_some_and(|value| self.property_fits(key, value.bytes.len()));
        self.shared.source.event(
            "x11_request",
            &format!(
                "target={:?} available={} bytes={} busy={}",
                target_name.chars().take(128).collect::<String>(),
                value.is_some(),
                value.as_ref().map_or(0, |v| v.bytes.len()),
                value.is_some() && !fits
            ),
        );
        if let Some(value) = value {
            if !fits {
                property = 0;
            } else {
                // We own the server and already hold the bounded value, so no
                // ChangeProperty request limit requires INCR. Ordinary properties
                // also avoid arboard's short inter-chunk deadline under VM load.
                // GetProperty still supports partial reads via offset/length.
                self.properties.insert(key, value);
                self.notify_property(key, 0)?;
            }
        } else {
            property = 0;
        }
        let mut b = self.packet(31, 0);
        o.set32(&mut b, 4, o.u32(r, 20));
        o.set32(&mut b, 8, window);
        o.set32(&mut b, 12, selection);
        o.set32(&mut b, 16, target);
        o.set32(&mut b, 20, property);
        self.send(&b)
    }
    /// SetSelectionOwner. Ownership of CLIPBOARD or PRIMARY lasts until the
    /// server has taken the selection: it asks the owner as any requestor
    /// would, then tells it the selection is gone.
    fn own(&mut self, r: &[u8]) -> io::Result<()> {
        let o = self.order;
        let (owner, selection, time) = (o.u32(r, 4), o.u32(r, 8), o.u32(r, 12));
        let name = self
            .shared
            .atoms
            .lock()
            .unwrap()
            .name(selection)
            .map(str::to_owned);
        if !self.shared.source.takes_copies() || !name.as_deref().is_some_and(offered) {
            return self.error(10, 22, 0);
        }
        if owner != 0 && !self.windows.contains(&owner) {
            return self.error(3, 22, owner);
        }
        self.taking.retain(|taking| taking.selection != selection);
        if owner == 0 {
            self.owned.remove(&selection);
        } else {
            self.owned.insert(selection, owner);
            self.taking.push_back(Taking {
                selection,
                owner,
                time,
                exported: name.as_deref() == Some("CLIPBOARD"),
                targets: vec!["TARGETS"],
                asked: false,
                parts: None,
            });
        }
        self.ask()
    }
    /// Sends the owner the request for the first selection to take, unless
    /// it is already asked.
    fn ask(&mut self) -> io::Result<()> {
        let Some(taking) = self.taking.front_mut() else {
            return Ok(());
        };
        if taking.asked {
            return Ok(());
        }
        taking.asked = true;
        let (time, owner, selection) = (taking.time, taking.owner, taking.selection);
        let target = self.atom(taking_target(&self.taking));
        let property = self.atom(ANSWER);
        self.properties.remove(&(REQUESTOR, property));
        let o = self.order;
        let mut b = self.packet(30, 0);
        o.set32(&mut b, 4, time);
        o.set32(&mut b, 8, owner);
        o.set32(&mut b, 12, REQUESTOR);
        o.set32(&mut b, 16, selection);
        o.set32(&mut b, 20, target);
        o.set32(&mut b, 24, property);
        self.send(&b)
    }
    /// The owner's SelectionNotify: its answer is on `property`, if any.
    fn answered(&mut self, selection: u32, target: u32, property: u32) -> io::Result<()> {
        let Some(taking) = self.taking.front() else {
            return Ok(());
        };
        if taking.selection != selection
            || taking.parts.is_some()
            || self.atom(taking_target(&self.taking)) != target
        {
            return Ok(());
        }
        let key = (REQUESTOR, property);
        let Some(value) = self.properties.remove(&key).filter(|_| property != 0) else {
            return self.refused();
        };
        self.notify_property(key, 1)?;
        if value.kind == self.atom("INCR") {
            // Deleting the property asked for the first part.
            self.taking.front_mut().unwrap().parts = Some(Vec::new());
            return Ok(());
        }
        self.received(value.bytes)
    }
    /// A part the owner sent on `key`; an empty one ends the selection.
    fn part(&mut self, key: (u32, u32)) -> io::Result<()> {
        if self
            .taking
            .front()
            .is_none_or(|taking| taking.parts.is_none())
        {
            return Ok(());
        }
        let Some(value) = self.properties.remove(&key) else {
            return Ok(());
        };
        self.notify_property(key, 1)?;
        let parts = self.taking.front_mut().unwrap().parts.as_mut().unwrap();
        if value.bytes.is_empty() {
            let bytes = std::mem::take(parts);
            return self.received(bytes);
        }
        if parts.len() + value.bytes.len() > newport_terminal::MAX_COPY {
            self.shared
                .source
                .event("x11_copy_refused", "larger than a terminal copies");
            return self.finish();
        }
        parts.extend(value.bytes);
        Ok(())
    }
    fn received(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        let exported = self.taking.front().unwrap().exported;
        let target = taking_target(&self.taking);
        if !exported {
            return self.finish();
        }
        if target == "TARGETS" {
            let names: Vec<String> = {
                let atoms = self
                    .shared
                    .atoms
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter_map(|id| atoms.name(self.order.u32(id, 0)).map(str::to_owned))
                    .collect()
            };
            let Some(text) = TEXT_TARGETS
                .into_iter()
                .find(|text| names.iter().any(|name| name == text))
            else {
                return self.finish();
            };
            let taking = self.taking.front_mut().unwrap();
            taking.targets = vec![text];
            taking.asked = false;
            return self.ask();
        }
        let taken = self.shared.source.copy(if target == "STRING" {
            latin1(bytes)
        } else {
            bytes
        });
        let selection = self.taking.front().unwrap().selection;
        if taken == crate::clipboard_source::Taken::Repeated && self.cleared.contains(&selection) {
            // Not given back to a program that took it back after losing it,
            // as clipboard managers do: it would do so without end. Any
            // other, as a second `xclip -i` of the same text, is told it
            // lost it, so it exits.
            self.taking.pop_front();
            return self.ask();
        }
        self.finish()
    }
    /// The owner refused the target: try the next, if any.
    fn refused(&mut self) -> io::Result<()> {
        let taking = self.taking.front_mut().unwrap();
        // An owner that lists no targets may still serve text.
        if taking.exported && taking.targets == ["TARGETS"] {
            taking.targets = vec!["UTF8_STRING", "STRING"];
        } else {
            taking.targets.remove(0);
        }
        if taking.targets.is_empty() || !taking.exported {
            return self.finish();
        }
        taking.asked = false;
        self.ask()
    }
    /// Gives the selection back to the computer's clipboard; the owner may
    /// stop serving it.
    fn finish(&mut self) -> io::Result<()> {
        let taking = self.taking.pop_front().unwrap();
        if self.owned.get(&taking.selection) == Some(&taking.owner) {
            self.owned.remove(&taking.selection);
            self.cleared.insert(taking.selection);
            let o = self.order;
            let mut b = self.packet(29, 0);
            o.set32(&mut b, 4, taking.time);
            o.set32(&mut b, 8, taking.owner);
            o.set32(&mut b, 12, taking.selection);
            self.send(&b)?;
        }
        self.ask()
    }
    fn gui_attempt(&mut self) {
        if !self.gui_notified {
            self.shared.source.gui.signal();
            self.gui_notified = true;
        }
    }
    fn request(&mut self, r: &[u8]) -> io::Result<()> {
        let op = r[0];
        let o = self.order;
        let min = match op {
            1 => 32,
            2 => 12,
            4 | 8 | 9 => 8,
            16 => 8,
            17 => 8,
            18 => 24,
            19 => 12,
            20 => 24,
            22 => 16,
            23 => 8,
            24 => 24,
            25 => 44,
            43 => 4,
            55 => 16,
            56 => 12,
            60 => 8,
            98 => 8,
            127 => 4,
            _ => return self.error(1, op, 0),
        };
        if r.len() < min {
            return self.error(16, op, 0);
        }
        match op {
            // Hidden selection windows are ordinary clipboard traffic. Mapping
            // a window, or asking for a renderer, requires GUI setup.
            8 | 9 => {
                let window = o.u32(r, 4);
                if r.len() != 8 {
                    return self.error(16, op, 0);
                }
                if window != 1 && !self.windows.contains(&window) {
                    return self.error(3, op, window);
                }
                self.gui_attempt();
                self.error(1, op, 0)
            }
            1 | 2 => {
                let id = o.u32(r, 4);
                let at = if op == 1 { 28 } else { 8 };
                let mask = o.u32(r, at);
                if r.len() != min + mask.count_ones() as usize * 4 {
                    return self.error(16, op, 0);
                }
                if op == 1 {
                    if id & !0x000f_ffff != self.base
                        || self.graphics_contexts.contains(&id)
                        || !self.windows.insert(id)
                    {
                        return self.error(14, op, id);
                    }
                    if self.windows.len() > 64 {
                        return Err(io::Error::other("window limit reached"));
                    }
                } else if id != REQUESTOR && !self.windows.contains(&id) {
                    return self.error(3, op, id);
                }
                if mask & (1 << 11) != 0 {
                    let index = (mask & ((1 << 11) - 1)).count_ones() as usize;
                    self.masks.insert(id, o.u32(r, min + index * 4));
                }
                Ok(())
            }
            4 => {
                let id = o.u32(r, 4);
                if !self.windows.remove(&id) {
                    return self.error(3, op, id);
                }
                if self.masks.remove(&id).unwrap_or(0) & (1 << 17) != 0 {
                    let mut b = self.packet(17, 0);
                    o.set32(&mut b, 4, id);
                    o.set32(&mut b, 8, id);
                    self.send(&b)?;
                }
                self.properties.retain(|k, _| k.0 != id);
                // A destroyed owner loses its selections without notice.
                self.owned.retain(|_, owner| *owner != id);
                self.taking.retain(|taking| taking.owner != id);
                self.ask()
            }
            16 => {
                let n = o.u16(r, 4) as usize;
                if n > 256 || r.len() != 8 + padded(n) {
                    return self.error(16, op, 0);
                }
                let Ok(name) = std::str::from_utf8(&r[8..8 + n]) else {
                    return self.error(2, op, 0);
                };
                let id = self
                    .shared
                    .atoms
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .intern(name, r[1] != 0);
                let mut b = self.packet(1, 0);
                o.set32(&mut b, 8, id);
                self.send(&b)
            }
            17 => {
                let id = o.u32(r, 4);
                let name = self
                    .shared
                    .atoms
                    .lock()
                    .unwrap()
                    .name(id)
                    .map(str::to_owned);
                let Some(name) = name else {
                    return self.error(5, op, id);
                };
                let mut b = self.packet(1, 0);
                o.set16(&mut b, 8, name.len() as u16);
                o.set32(&mut b, 4, (padded(name.len()) / 4) as u32);
                b.extend(name.as_bytes());
                b.resize(32 + padded(name.len()), 0);
                self.send(&b)
            }
            // Programs set properties on their own windows, and owners answer
            // on the requestor window.
            18 => {
                let key = (o.u32(r, 4), o.u32(r, 8));
                if key.0 == REQUESTOR {
                    if self.taking.is_empty() {
                        // An answer nobody waits for any more.
                        return Ok(());
                    }
                } else if !self.windows.contains(&key.0) {
                    return self.error(3, op, key.0);
                }
                let (mode, kind, format) = (r[1], o.u32(r, 12), r[16]);
                if ![8, 16, 32].contains(&format) || mode > 2 {
                    return self.error(2, op, 0);
                }
                let n = (o.u32(r, 20) as usize).saturating_mul(format as usize / 8);
                // An answer may fill a whole request.
                if (n > CLIENT_PROPERTY_LIMIT && key.0 != REQUESTOR) || r.len() != 24 + padded(n) {
                    return self.error(16, op, 0);
                }
                let data = &r[24..24 + n];
                // Prepend and append extend a value of the same type; xsel
                // appends nothing to learn the server's time.
                let bytes = match self.properties.get(&key) {
                    Some(old) if mode != 0 && (old.kind != kind || old.format != format) => {
                        return self.error(8, op, 0);
                    }
                    Some(old) if mode == 1 => [data, &old.bytes].concat(),
                    Some(old) if mode == 2 => [&old.bytes, data].concat(),
                    _ => data.to_vec(),
                };
                if !self.property_fits(key, bytes.len()) {
                    return self.error(11, op, 0);
                }
                self.properties.insert(
                    key,
                    Property {
                        kind,
                        format,
                        bytes,
                    },
                );
                self.notify_property(key, 0)?;
                if key.0 == REQUESTOR {
                    return self.part(key);
                }
                Ok(())
            }
            19 => self.delete((o.u32(r, 4), o.u32(r, 8))),
            20 => {
                let key = (o.u32(r, 4), o.u32(r, 8));
                if ![1, OWNER, REQUESTOR].contains(&key.0) && !self.windows.contains(&key.0) {
                    return self.error(3, op, key.0);
                }
                let mut b = self.packet(1, 0);
                let mut delete = false;
                if let Some(p) = self.properties.get(&key) {
                    b[1] = p.format;
                    o.set32(&mut b, 8, p.kind);
                    let kind = o.u32(r, 12);
                    if kind != 0 && kind != p.kind {
                        o.set32(&mut b, 12, p.bytes.len() as u32);
                    } else {
                        let start = o.u32(r, 16) as usize * 4;
                        if start > p.bytes.len() {
                            return self.error(2, op, 0);
                        }
                        let end = start
                            .saturating_add(o.u32(r, 20) as usize * 4)
                            .min(p.bytes.len());
                        let n = end - start;
                        o.set32(&mut b, 12, (p.bytes.len() - end) as u32);
                        o.set32(&mut b, 16, (n / (p.format as usize / 8)) as u32);
                        o.set32(&mut b, 4, (padded(n) / 4) as u32);
                        b.extend_from_slice(&p.bytes[start..end]);
                        b.resize(32 + padded(n), 0);
                        delete = r[1] != 0 && end == p.bytes.len();
                    }
                }
                if delete {
                    self.delete(key)?;
                }
                self.send(&b)
            }
            22 => self.own(r),
            23 => {
                let id = o.u32(r, 4);
                let clipboard = self
                    .shared
                    .atoms
                    .lock()
                    .unwrap()
                    .name(id)
                    .is_some_and(offered);
                let mut b = self.packet(1, 0);
                if let Some(owner) = self.owned.get(&id) {
                    o.set32(&mut b, 8, *owner);
                } else if clipboard && self.shared.source.offer().is_ok_and(|(_, s)| !s.is_empty())
                {
                    o.set32(&mut b, 8, OWNER);
                }
                self.send(&b)
            }
            24 => self.selection(r),
            25 => {
                if r.len() != 44 {
                    return self.error(16, op, 0);
                }
                let destination = o.u32(r, 4);
                let mut event = r[12..44].to_vec();
                if destination == REQUESTOR {
                    if event[0] & 0x7f == 31 {
                        return self.answered(
                            o.u32(&event, 12),
                            o.u32(&event, 16),
                            o.u32(&event, 20),
                        );
                    }
                } else if self.windows.contains(&destination) {
                    // Only this client has windows to deliver to.
                    event[0] |= 0x80;
                    o.set16(&mut event, 2, self.seq);
                    return self.send(&event);
                }
                Ok(())
            }
            43 => {
                let mut b = self.packet(1, 0);
                o.set32(&mut b, 8, 1);
                self.send(&b)
            }
            // Xlib creates a default GC when opening a display, including for
            // read-only clipboard clients. Track its lifecycle; nothing is drawn.
            55 | 56 => {
                let id = o.u32(r, 4);
                let mask = o.u32(r, if op == 55 { 12 } else { 8 });
                if mask & !0x007f_ffff != 0 {
                    return self.error(2, op, mask);
                }
                if r.len() != min + mask.count_ones() as usize * 4 {
                    return self.error(16, op, 0);
                }
                if op == 55 {
                    if id & !0x000f_ffff != self.base
                        || self.windows.contains(&id)
                        || self.graphics_contexts.contains(&id)
                    {
                        return self.error(14, op, id);
                    }
                    let drawable = o.u32(r, 8);
                    if drawable != 1 && !self.windows.contains(&drawable) {
                        return self.error(9, op, drawable);
                    }
                    if self.graphics_contexts.len() >= 64 {
                        return self.error(11, op, id);
                    }
                    self.graphics_contexts.insert(id);
                } else if !self.graphics_contexts.contains(&id) {
                    return self.error(13, op, id);
                }
                Ok(())
            }
            60 => {
                if r.len() != min {
                    return self.error(16, op, 0);
                }
                let id = o.u32(r, 4);
                if !self.graphics_contexts.remove(&id) {
                    return self.error(13, op, id);
                }
                Ok(())
            }
            98 => {
                let n = o.u16(r, 4) as usize;
                if r.len() != 8 + padded(n) {
                    return self.error(16, op, 0);
                }
                if matches!(&r[8..8 + n], b"GLX" | b"RENDER") {
                    self.gui_attempt();
                }
                let b = self.packet(1, 0);
                self.send(&b)
            }
            127 => Ok(()),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_valid_render_requests_signal_gui_intent() {
        for (request, expected) in [
            (vec![8, 0, 2, 0, 1, 0, 0, 0], true),   // MapWindow(root)
            (vec![8, 0, 2, 0, 90, 0, 0, 0], false), // unknown window
            (vec![8, 0, 1, 0], false),              // malformed
            (
                [vec![98, 0, 3, 0, 3, 0, 0, 0], b"GLX\0".to_vec()].concat(),
                true,
            ),
            (
                [vec![98, 0, 4, 0, 6, 0, 0, 0], b"RENDER\0\0".to_vec()].concat(),
                true,
            ),
            (
                [vec![98, 0, 4, 0, 6, 0, 0, 0], b"XFIXES\0\0".to_vec()].concat(),
                false,
            ),
        ] {
            let path = PathBuf::from("/unused-snapshot");
            let source = crate::clipboard_source::Source::new(&path);
            source.gui.enable();
            let (socket, _reader) = UnixStream::pair().unwrap();
            let mut client = Client {
                socket,
                order: Order(true),
                seq: 1,
                gui_notified: false,
                base: 0,
                shared: Arc::new(Shared {
                    atoms: Mutex::new(Atoms::default()),
                    source: source.clone(),
                }),
                windows: Default::default(),
                graphics_contexts: Default::default(),
                masks: Default::default(),
                properties: Default::default(),
                owned: Default::default(),
                taking: Default::default(),
                cleared: Default::default(),
            };
            client.request(&request).unwrap();
            assert_eq!(source.gui.take(), expected, "{request:?}");
        }
    }

    /// Another account (anyone the listener doesn't recognise) needs the cookie.
    #[test]
    fn a_client_without_the_cookie_is_refused_when_one_is_required() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.tar");
        let shared = || {
            Arc::new(Shared {
                atoms: Mutex::new(Atoms::default()),
                source: crate::clipboard_source::Source::new(&path),
            })
        };
        let cookie = [7; 16];
        let setup = |auth: &[u8]| {
            let mut b = vec![b'l', 0, 11, 0, 0, 0];
            b.extend_from_slice(&18u16.to_le_bytes());
            b.extend_from_slice(&16u16.to_le_bytes());
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
            b.extend_from_slice(auth);
            b
        };
        for (auth, admitted) in [([0; 16], false), (cookie, true)] {
            let (server, mut client) = UnixStream::pair().unwrap();
            client.write_all(&setup(&auth)).unwrap();
            let shared = shared();
            let serving = std::thread::spawn(move || serve(server, shared, Some(cookie), 0));
            let mut status = [0];
            client.read_exact(&mut status).unwrap();
            assert_eq!(status[0] == 1, admitted);
            drop(client);
            let _ = serving.join();
        }
    }
}
