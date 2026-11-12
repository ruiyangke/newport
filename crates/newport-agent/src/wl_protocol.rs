//! wlr-data-control objects, offers, and selection notifications. The
//! selection and, for devices that know it, the primary selection both offer
//! the computer's clipboard. On a display that takes copies, the text a
//! program sets as the selection goes to the computer.
use crate::wl_transfer::{read_pipe, write_pipe};
use std::{
    collections::BTreeMap,
    os::fd::AsFd,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use wayland_protocols_wlr::data_control::v1::server::{
    zwlr_data_control_device_v1::{self as device, ZwlrDataControlDeviceV1 as Device},
    zwlr_data_control_manager_v1::{self as manager, ZwlrDataControlManagerV1 as Manager},
    zwlr_data_control_offer_v1::{self as offer, ZwlrDataControlOfferV1 as Offer},
    zwlr_data_control_source_v1::{self as source, ZwlrDataControlSourceV1 as Source},
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
    protocol::wl_compositor::{self, WlCompositor},
    protocol::wl_seat::{self, WlSeat},
};

type Formats = BTreeMap<String, Vec<u8>>;
/// Text types a program's copy may offer, preferred first.
const COPIED_TEXT: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];
/// Other names readers ask for the computer's text by. Not `STRING`, which
/// is Latin-1: a Wayland client that takes the first text type it knows
/// would lose text outside it.
const TEXT: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "TEXT"];
type Revision = crate::clipboard_source::Revision;
const READ_ONLY: &str = "Newport clipboard is read-only";
pub(crate) struct State {
    source: crate::clipboard_source::Source,
    revision: Option<Revision>,
    formats: Arc<Formats>,
    devices: Vec<Weak<Device>>,
    transfers: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    copying: Arc<Copying>,
    gui_candidates: Vec<Client>,
}
/// Programs' copies still being read. The program was already told its
/// selection was replaced, so a read asked for meanwhile waits for them:
/// `wl-copy x; wl-paste` returns `x`, not the computer's clipboard.
#[derive(Default)]
struct Copying {
    reading: Mutex<usize>,
    read: Condvar,
}
impl Copying {
    fn started(&self) {
        *self.reading.lock().unwrap_or_else(|p| p.into_inner()) += 1;
    }
    fn finished(&self) {
        *self.reading.lock().unwrap_or_else(|p| p.into_inner()) -= 1;
        self.read.notify_all();
    }
    fn any(&self) -> bool {
        *self.reading.lock().unwrap_or_else(|p| p.into_inner()) > 0
    }
    /// Waits until no copy is being read. Reads end by themselves, as a
    /// stalled writer is given up on.
    fn settled(&self, stop: &AtomicBool) {
        let mut reading = self.reading.lock().unwrap_or_else(|p| p.into_inner());
        while *reading > 0 && !stop.load(Ordering::Relaxed) {
            reading = self
                .read
                .wait_timeout(reading, Duration::from_millis(50))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }
}
struct Offered {
    revision: Option<Revision>,
    active: Arc<AtomicUsize>,
}
impl Drop for Offered {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

impl State {
    pub(crate) fn new(source: crate::clipboard_source::Source, stop: Arc<AtomicBool>) -> Self {
        Self {
            source,
            revision: None,
            formats: Arc::new(BTreeMap::new()),
            devices: Vec::new(),
            transfers: Arc::new(AtomicUsize::new(0)),
            stop,
            copying: Default::default(),
            gui_candidates: Vec::new(),
        }
    }
    pub(crate) fn register(handle: &DisplayHandle) {
        handle.create_global::<Self, WlSeat, _>(7, ());
        handle.create_global::<Self, Manager, _>(2, ());
        handle.create_global::<Self, WlCompositor, _>(2, ());
    }

    pub(crate) fn publish_gui_notices(&mut self) {
        // Clipboard tools can probe the compositor too. Inspect all bindings
        // dispatched together before deciding whether this client wants a GUI.
        for client in self.gui_candidates.drain(..) {
            if !client
                .get_data::<crate::wl_server::Connection>()
                .is_some_and(|data| data.clipboard.load(Ordering::Relaxed))
            {
                self.source.gui.signal();
            }
        }
    }

    pub(crate) fn refresh(&mut self, handle: &DisplayHandle) {
        self.devices.retain(|device| device.upgrade().is_ok());
        // With no client watching the selection, the source isn't asked: a
        // held shell's is the Mac in use, over the network.
        if self.devices.is_empty() {
            self.revision = None;
            self.formats = Arc::new(BTreeMap::new());
            return;
        }
        self.reload(handle);
    }
    fn reload(&mut self, handle: &DisplayHandle) {
        let offer = self.source.offer().ok();
        let current = offer.as_ref().map(|(r, _)| *r);
        if self.revision == current {
            return;
        }
        self.revision = current;
        self.formats = Arc::new(offer.map(|(_, f)| f).unwrap_or_default());
        for device in &self.devices {
            if let Ok(device) = device.upgrade() {
                self.publish(&device, handle);
            }
        }
    }
    fn publish(&self, device: &Device, handle: &DisplayHandle) {
        // Each selection event takes an offer of its own.
        let primary = device.version() >= device::EVT_PRIMARY_SELECTION_SINCE;
        if self.formats.is_empty() {
            device.selection(None);
            if primary {
                device.primary_selection(None);
            }
            return;
        }
        if let Some(offer) = self.offer(device, handle) {
            device.selection(Some(&offer));
        }
        if primary && let Some(offer) = self.offer(device, handle) {
            device.primary_selection(Some(&offer));
        }
    }
    fn offer(&self, device: &Device, handle: &DisplayHandle) -> Option<Offer> {
        let client = device.client()?;
        let active = device.data::<Arc<AtomicUsize>>().unwrap().clone();
        if active.load(Ordering::Relaxed) >= 32 {
            device.post_error(0u32, "Too many outstanding clipboard offers");
            return None;
        }
        active.fetch_add(1, Ordering::Relaxed);
        let offer = client
            .create_resource::<Offer, _, Self>(
                handle,
                1,
                Offered {
                    revision: self.revision,
                    active,
                },
            )
            .ok()?;
        device.data_offer(&offer);
        for mime in self.formats.keys() {
            offer.offer(mime.clone());
        }
        if self.formats.contains_key("text/plain") {
            for alias in TEXT {
                offer.offer(alias.into());
            }
        }
        Some(offer)
    }
}

impl GlobalDispatch<WlCompositor, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        object: New<WlCompositor>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        if !state
            .gui_candidates
            .iter()
            .any(|pending| pending.id() == client.id())
        {
            state.gui_candidates.push(client.clone());
        }
        init.init(object, ());
    }
}
impl Dispatch<WlCompositor, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        object: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        state.source.gui.signal();
        let message = "Set up GUI apps in Newport, then open a new terminal";
        match request {
            wl_compositor::Request::CreateSurface { id } => init.post_error(id, 0u32, message),
            wl_compositor::Request::CreateRegion { id } => init.post_error(id, 0u32, message),
            _ => object.post_error(0u32, message),
        }
    }
}

impl GlobalDispatch<WlSeat, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        seat: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(seat, ());
        seat.capabilities(wl_seat::Capability::empty());
        if seat.version() >= 2 {
            seat.name("newport".into());
        }
    }
}
impl Dispatch<WlSeat, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        seat: &WlSeat,
        request: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_seat::Request::Release => (),
            wl_seat::Request::GetPointer { id } => init.post_error(id, 0u32, "No pointer"),
            wl_seat::Request::GetKeyboard { id } => init.post_error(id, 0u32, "No keyboard"),
            wl_seat::Request::GetTouch { id } => init.post_error(id, 0u32, "No touch device"),
            _ => seat.post_error(0u32, "Unsupported seat request"),
        }
    }
}
impl GlobalDispatch<Manager, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        manager: New<Manager>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        if let Some(data) = client.get_data::<crate::wl_server::Connection>() {
            data.clipboard.store(true, Ordering::Relaxed);
        }
        init.init(manager, ());
    }
}
impl Dispatch<Manager, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &Manager,
        request: manager::Request,
        _: &(),
        handle: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::GetDataDevice { id, .. } => {
                state.reload(handle);
                if state.devices.len() >= 32 {
                    init.post_error(id, 0u32, "Too many clipboard devices");
                    return;
                }
                let device = init.init(id, Arc::new(AtomicUsize::new(0)));
                state.publish(&device, handle);
                state.devices.push(device.downgrade());
            }
            manager::Request::CreateDataSource { id } if state.source.takes_copies() => {
                init.init(id, Mutex::new(Vec::<String>::new()));
            }
            manager::Request::CreateDataSource { id } => init.post_error(id, 0u32, READ_ONLY),
            _ => (),
        }
    }
}
impl Dispatch<Device, Arc<AtomicUsize>> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        device: &Device,
        request: device::Request,
        _: &Arc<AtomicUsize>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            device::Request::SetSelection { .. } | device::Request::SetPrimarySelection { .. }
                if !state.source.takes_copies() =>
            {
                device.post_error(0u32, READ_ONLY);
            }
            device::Request::SetSelection {
                source: Some(source),
            } => {
                state.take(&source);
                source.cancelled();
            }
            // A program's primary selection never leaves the server.
            device::Request::SetPrimarySelection {
                source: Some(source),
            } => source.cancelled(),
            _ => (),
        }
    }
}
/// The MIME types a program's source offers.
type Offers = Mutex<Vec<String>>;
impl Dispatch<Source, Offers> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        source: &Source,
        request: source::Request,
        offers: &Offers,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let source::Request::Offer { mime_type } = request {
            let mut offers = offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if offers.len() >= 64 {
                source.post_error(0u32, "Too many clipboard formats");
                return;
            }
            offers.push(mime_type);
        }
    }
}
impl State {
    /// Takes the text a program set as the selection, for the computer. The
    /// program is then told its selection was replaced: the selection is
    /// the computer's clipboard again, which the copy updates.
    fn take(&mut self, source: &Source) {
        let offers = source
            .data::<Offers>()
            .unwrap()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(mime) = COPIED_TEXT
            .into_iter()
            .find(|text| offers.iter().any(|offered| offered == text))
        else {
            return;
        };
        if self.transfers.load(Ordering::Relaxed) >= 8 {
            self.source
                .event("wayland_copy_refused", "too many transfers");
            return;
        }
        let Ok((reader, writer)) = std::io::pipe() else {
            return;
        };
        source.send(mime.into(), writer.as_fd());
        drop(writer);
        let (active, stop, clipboard, copying) = (
            self.transfers.clone(),
            self.stop.clone(),
            self.source.clone(),
            self.copying.clone(),
        );
        active.fetch_add(1, Ordering::Relaxed);
        copying.started();
        thread::spawn(move || {
            match read_pipe(reader, newport_terminal::MAX_COPY, &stop) {
                Ok(text) => {
                    clipboard.copy(text);
                }
                Err(error) => {
                    clipboard.event("wayland_copy_failed", &format!("kind={:?}", error.kind()))
                }
            }
            copying.finished();
            active.fetch_sub(1, Ordering::Relaxed);
        });
    }
}
impl Dispatch<Offer, Offered> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &Offer,
        request: offer::Request,
        data: &Offered,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let offer::Request::Receive { mime_type, fd } = request {
            let (revision, available) = match (data.revision, state.source.offer().ok()) {
                (Some(offered), Some((current, _)))
                    if current == offered && state.revision == Some(offered) =>
                {
                    (offered, state.formats.clone())
                }
                // A program's copy made since the offer is what the selection
                // holds now, as a new offer would say.
                (Some(_), Some((current, formats)))
                    if crate::clipboard_source::is_copy(current) =>
                {
                    (current, Arc::new(formats))
                }
                // Old offers cannot resurrect expired or replaced clipboard contents.
                _ => {
                    state.source.event(
                        "wayland_request_stale",
                        "offer no longer matches current snapshot",
                    );
                    return;
                }
            };
            let mime = if TEXT.contains(&mime_type.as_str()) {
                "text/plain"
            } else {
                &mime_type
            };
            if !available.contains_key(mime) || state.transfers.load(Ordering::Relaxed) >= 8 {
                state.source.event(
                    "wayland_request_rejected",
                    &format!(
                        "mime={:?} available={} active={}",
                        mime.chars().take(128).collect::<String>(),
                        available.contains_key(mime),
                        state.transfers.load(Ordering::Relaxed)
                    ),
                );
                return;
            }
            let mime = mime.to_owned();
            let active = state.transfers.clone();
            let stop = state.stop.clone();
            let source = state.source.clone();
            let copying = state.copying.any().then(|| state.copying.clone());
            source.event(
                "wayland_request",
                &format!("mime={:?}", mime.chars().take(128).collect::<String>(),),
            );
            active.fetch_add(1, Ordering::Relaxed);
            thread::spawn(move || {
                let started = std::time::Instant::now();
                // A copy made before this read, still being read, is what it
                // returns once it is. A computer's clipboard newer than the
                // offer is not: the offer never named it.
                let current = copying.and_then(|copying| {
                    copying.settled(&stop);
                    source.offer().ok().map(|(current, _)| current)
                });
                let result = match current {
                    Some(current)
                        if current != revision && !crate::clipboard_source::is_copy(current) =>
                    {
                        Err(std::io::Error::other(
                            "offer no longer matches current snapshot",
                        ))
                    }
                    current => source
                        .get(current.unwrap_or(revision), &mime)
                        .and_then(|bytes| write_pipe(fd, &bytes, &stop)),
                };
                source.event(
                    "wayland_transfer_finished",
                    &format!(
                        "success={} elapsed_ms={} error={:?}",
                        result.is_ok(),
                        started.elapsed().as_millis(),
                        result.err().map(|e| e.kind())
                    ),
                );
                active.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }
}
