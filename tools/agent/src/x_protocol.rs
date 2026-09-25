//! Only the core X11 requests needed by clipboard readers. No rendering backend.
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const CHUNK: usize = 64 * 1024;
const OWNER: u32 = 2;

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
fn padded(n: usize) -> usize {
    (n + 3) & !3
}

pub struct Atoms(Vec<String>);
impl Default for Atoms {
    fn default() -> Self {
        // Core protocol atom IDs are fixed; dynamic names start at 69.
        Self("PRIMARY SECONDARY ARC ATOM BITMAP CARDINAL COLORMAP CURSOR CUT_BUFFER0 CUT_BUFFER1 CUT_BUFFER2 CUT_BUFFER3 CUT_BUFFER4 CUT_BUFFER5 CUT_BUFFER6 CUT_BUFFER7 DRAWABLE FONT INTEGER PIXMAP POINT RECTANGLE RESOURCE_MANAGER RGB_COLOR_MAP RGB_BEST_MAP RGB_BLUE_MAP RGB_DEFAULT_MAP RGB_GRAY_MAP RGB_GREEN_MAP RGB_RED_MAP STRING VISUALID WINDOW WM_COMMAND WM_HINTS WM_CLIENT_MACHINE WM_ICON_NAME WM_ICON_SIZE WM_NAME WM_NORMAL_HINTS WM_SIZE_HINTS WM_ZOOM_HINTS MIN_SPACE NORM_SPACE MAX_SPACE END_SPACE SUPERSCRIPT_X SUPERSCRIPT_Y SUBSCRIPT_X SUBSCRIPT_Y UNDERLINE_POSITION UNDERLINE_THICKNESS STRIKEOUT_ASCENT STRIKEOUT_DESCENT ITALIC_ANGLE X_HEIGHT QUAD_WIDTH WEIGHT POINT_SIZE RESOLUTION COPYRIGHT NOTICE FONT_NAME FAMILY_NAME FULL_NAME CAP_HEIGHT WM_CLASS WM_TRANSIENT_FOR".split_whitespace().map(str::to_owned).collect())
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
    pub snapshot: PathBuf,
    pub(crate) source: crate::clipboard_source::Source,
}
#[derive(Clone)]
struct Property {
    kind: u32,
    format: u8,
    bytes: Vec<u8>,
}
struct Transfer {
    key: (u32, u32),
    kind: u32,
    bytes: Vec<u8>,
    offset: usize,
}
struct Client {
    socket: UnixStream,
    order: Order,
    seq: u16,
    base: u32,
    shared: Arc<Shared>,
    windows: HashSet<u32>,
    graphics_contexts: HashSet<u32>,
    masks: HashMap<u32, u32>,
    properties: HashMap<(u32, u32), Property>,
    transfer: Option<Transfer>,
}

/// X11 setup with one tiny logical screen. Nothing is ever drawn or mapped.
fn setup(o: Order, base: u32) -> Vec<u8> {
    let vendor = b"Porthop clipboard";
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

pub fn serve(
    mut socket: UnixStream,
    shared: Arc<Shared>,
    cookie: [u8; 16],
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
    if &auth[..name] != b"MIT-MAGIC-COOKIE-1"
        || data != 16
        || auth[padded(name)..padded(name) + data] != cookie
    {
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
        base,
        shared,
        windows: HashSet::new(),
        graphics_contexts: HashSet::new(),
        masks: HashMap::new(),
        properties: HashMap::new(),
        transfer: None,
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
        self.shared.atoms.lock().unwrap().intern(name, false)
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
        if let Some(mut t) = self.transfer.take() {
            if t.key == key {
                let end = (t.offset + CHUNK).min(t.bytes.len());
                self.properties.insert(
                    key,
                    Property {
                        kind: t.kind,
                        format: 8,
                        bytes: t.bytes[t.offset..end].to_vec(),
                    },
                );
                let finished = t.offset == t.bytes.len();
                t.offset = end;
                if !finished {
                    self.transfer = Some(t);
                } else {
                    crate::diagnostics::event(
                        &self.shared.snapshot,
                        "x11_transfer_served",
                        &format!("bytes={}", t.bytes.len()),
                    );
                }
                self.notify_property(key, 0)?;
            } else {
                self.transfer = Some(t);
            }
        }
        Ok(())
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
            let a = self.shared.atoms.lock().unwrap();
            (
                a.name(selection).unwrap_or("").to_owned(),
                a.name(target).unwrap_or("").to_owned(),
            )
        };
        let (revision, formats) = if selection_name == "CLIPBOARD" {
            self.shared.source.offer().unwrap_or_else(|error| {
                crate::diagnostics::event(
                    &self.shared.snapshot,
                    "x11_snapshot_failed",
                    &format!("kind={:?}", error.kind()),
                );
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
                | "TEXT"
                | "text/plain;charset=utf-8"
                | "text/plain; charset=utf-8" => "text/plain",
                s => s,
            };
            self.shared
                .source
                .get(revision, name)
                .ok()
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
        crate::diagnostics::event(
            &self.shared.snapshot,
            "x11_request",
            &format!(
                "target={:?} available={} bytes={} busy={}",
                target_name.chars().take(128).collect::<String>(),
                value.is_some(),
                value.as_ref().map_or(0, |v| v.bytes.len()),
                self.transfer.is_some() || self.properties.len() >= 128
            ),
        );
        if let Some(value) = value {
            let key = (window, property);
            if self.transfer.is_some() || self.properties.len() >= 128 {
                property = 0;
            } else {
                if value.bytes.len() > CHUNK {
                    let size = value.bytes.len() as u32;
                    self.transfer = Some(Transfer {
                        key,
                        kind: value.kind,
                        bytes: value.bytes,
                        offset: 0,
                    });
                    self.properties.insert(
                        key,
                        Property {
                            kind: self.atom("INCR"),
                            format: 32,
                            bytes: o.words([size]),
                        },
                    );
                } else {
                    self.properties.insert(key, value);
                }
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
    fn request(&mut self, r: &[u8]) -> io::Result<()> {
        let op = r[0];
        let o = self.order;
        let min = match op {
            1 => 32,
            2 => 12,
            4 => 8,
            16 => 8,
            17 => 8,
            18 => 24,
            19 => 12,
            20 => 24,
            22 => 16,
            23 => 8,
            24 => 24,
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
                } else if !self.windows.contains(&id) {
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
                if self.transfer.as_ref().is_some_and(|t| t.key.0 == id) {
                    self.transfer = None;
                }
                Ok(())
            }
            16 => {
                let n = o.u16(r, 4) as usize;
                if n > 256 || r.len() != 8 + padded(n) {
                    return self.error(16, op, 0);
                }
                let Ok(name) = std::str::from_utf8(&r[8..8 + n]) else {
                    return self.error(2, op, 0);
                };
                let id = self.shared.atoms.lock().unwrap().intern(name, r[1] != 0);
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
            // Clipboard state is read-only. Client-side properties are allowed for negotiation.
            18 => {
                let key = (o.u32(r, 4), o.u32(r, 8));
                if !self.windows.contains(&key.0) {
                    return self.error(3, op, key.0);
                }
                let format = r[16];
                if ![8, 16, 32].contains(&format) || r[1] != 0 {
                    return self.error(2, op, 0);
                }
                let n = (o.u32(r, 20) as usize).saturating_mul(format as usize / 8);
                if n > CHUNK || r.len() != 24 + padded(n) {
                    return self.error(16, op, 0);
                }
                if self.properties.len() >= 128 {
                    return self.error(11, op, 0);
                }
                self.properties.insert(
                    key,
                    Property {
                        kind: o.u32(r, 12),
                        format,
                        bytes: r[24..24 + n].to_vec(),
                    },
                );
                self.notify_property(key, 0)
            }
            19 => self.delete((o.u32(r, 4), o.u32(r, 8))),
            20 => {
                let key = (o.u32(r, 4), o.u32(r, 8));
                if key.0 != 1 && key.0 != OWNER && !self.windows.contains(&key.0) {
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
                self.send(&b)?;
                if delete {
                    self.delete(key)?;
                }
                Ok(())
            }
            22 => self.error(10, op, 0),
            23 => {
                let id = o.u32(r, 4);
                let clipboard = self.shared.atoms.lock().unwrap().name(id) == Some("CLIPBOARD");
                let mut b = self.packet(1, 0);
                if clipboard && self.shared.source.offer().is_ok_and(|(_, s)| !s.is_empty()) {
                    o.set32(&mut b, 8, OWNER);
                }
                self.send(&b)
            }
            24 => self.selection(r),
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
                let b = self.packet(1, 0);
                self.send(&b)
            }
            127 => Ok(()),
            _ => unreachable!(),
        }
    }
}
