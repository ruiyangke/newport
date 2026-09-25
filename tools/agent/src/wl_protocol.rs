//! Read-only wlr-data-control objects, offers, and selection notifications.
use crate::wl_transfer::write_pipe;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
};
use wayland_protocols_wlr::data_control::v1::server::{
    zwlr_data_control_device_v1::{self as device, ZwlrDataControlDeviceV1 as Device},
    zwlr_data_control_manager_v1::{self as manager, ZwlrDataControlManagerV1 as Manager},
    zwlr_data_control_offer_v1::{self as offer, ZwlrDataControlOfferV1 as Offer},
};
use wayland_server::{
    protocol::wl_seat::{self, WlSeat},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};

type Formats = BTreeMap<String, Vec<u8>>;
type Revision = crate::clipboard_source::Revision;
const READ_ONLY: &str = "Porthop clipboard is read-only";
pub(crate) struct State {
    path: PathBuf,
    source: crate::clipboard_source::Source,
    revision: Option<Revision>,
    formats: Arc<Formats>,
    devices: Vec<Weak<Device>>,
    transfers: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
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
            path: source.path.clone(),
            source,
            revision: None,
            formats: Arc::new(BTreeMap::new()),
            devices: Vec::new(),
            transfers: Arc::new(AtomicUsize::new(0)),
            stop,
        }
    }
    pub(crate) fn register(handle: &DisplayHandle) {
        handle.create_global::<Self, WlSeat, _>(7, ());
        handle.create_global::<Self, Manager, _>(2, ());
    }

    pub(crate) fn refresh(&mut self, handle: &DisplayHandle) {
        self.devices.retain(|device| device.upgrade().is_ok());
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
        let Some(client) = device.client() else {
            return;
        };
        if self.formats.is_empty() {
            device.selection(None);
            return;
        }
        let active = device.data::<Arc<AtomicUsize>>().unwrap().clone();
        if active.load(Ordering::Relaxed) >= 32 {
            device.post_error(0u32, "Too many outstanding clipboard offers");
            return;
        }
        active.fetch_add(1, Ordering::Relaxed);
        let Ok(offer) = client.create_resource::<Offer, _, Self>(
            handle,
            1,
            Offered {
                revision: self.revision,
                active,
            },
        ) else {
            return;
        };
        device.data_offer(&offer);
        for mime in self.formats.keys() {
            offer.offer(mime.clone());
        }
        if self.formats.contains_key("text/plain") {
            offer.offer("text/plain;charset=utf-8".into());
        }
        device.selection(Some(&offer));
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
            seat.name("porthop".into());
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
        _: &Client,
        manager: New<Manager>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
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
                state.refresh(handle);
                if state.devices.len() >= 32 {
                    init.post_error(id, 0u32, "Too many clipboard devices");
                    return;
                }
                let device = init.init(id, Arc::new(AtomicUsize::new(0)));
                state.publish(&device, handle);
                // No primary selection event: we deliberately do not support it.
                state.devices.push(device.downgrade());
            }
            manager::Request::CreateDataSource { id } => init.post_error(id, 0u32, READ_ONLY),
            _ => (),
        }
    }
}
impl Dispatch<Device, Arc<AtomicUsize>> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        device: &Device,
        request: device::Request,
        _: &Arc<AtomicUsize>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(
            request,
            device::Request::SetSelection { .. } | device::Request::SetPrimarySelection { .. }
        ) {
            device.post_error(0u32, READ_ONLY);
        }
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
            // Old offers cannot resurrect expired or replaced clipboard contents.
            if data.revision.is_none()
                || state.source.offer().ok().map(|(r, _)| r) != data.revision
                || state.revision != data.revision
            {
                crate::diagnostics::event(
                    &state.path,
                    "wayland_request_stale",
                    "offer no longer matches current snapshot",
                );
                return;
            }
            let mime = if mime_type == "text/plain;charset=utf-8" {
                "text/plain"
            } else {
                &mime_type
            };
            if !state.formats.contains_key(mime) || state.transfers.load(Ordering::Relaxed) >= 8 {
                crate::diagnostics::event(
                    &state.path,
                    "wayland_request_rejected",
                    &format!(
                        "mime={:?} available={} active={}",
                        mime.chars().take(128).collect::<String>(),
                        state.formats.contains_key(mime),
                        state.transfers.load(Ordering::Relaxed)
                    ),
                );
                return;
            }
            let revision = data.revision.unwrap();
            let mime = mime.to_owned();
            let active = state.transfers.clone();
            let stop = state.stop.clone();
            let path = state.path.clone();
            let source = state.source.clone();
            crate::diagnostics::event(
                &path,
                "wayland_request",
                &format!("mime={:?}", mime.chars().take(128).collect::<String>(),),
            );
            active.fetch_add(1, Ordering::Relaxed);
            thread::spawn(move || {
                let started = std::time::Instant::now();
                let result = source
                    .get(revision, &mime)
                    .and_then(|bytes| write_pipe(fd, &bytes, &stop));
                crate::diagnostics::event(
                    &path,
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
