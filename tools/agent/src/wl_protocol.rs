//! Read-only wlr-data-control objects, offers, and selection notifications.
use crate::wl_transfer::write_pipe;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
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
type Revision = (u64, u64, u64);
const READ_ONLY: &str = "Porthop clipboard is read-only";

/// Only metadata is polled. A heartbeat updates mtime without reloading the archive.
fn revision(path: &Path) -> Option<Revision> {
    let m = fs::symlink_metadata(path).ok()?;
    (m.is_file()
        && m.uid() == unsafe { libc::geteuid() }
        && m.modified().ok()?.elapsed().ok()? < Duration::from_secs(120))
    .then_some((m.dev(), m.ino(), m.len()))
}

pub(crate) struct State {
    path: PathBuf,
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
    pub(crate) fn new(path: PathBuf, stop: Arc<AtomicBool>) -> Self {
        Self {
            path,
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
        let current = revision(&self.path);
        if self.revision == current {
            return;
        }
        self.revision = current;
        self.formats = Arc::new(crate::snapshot::read(&self.path).unwrap_or_default());
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
                || revision(&state.path) != data.revision
                || state.revision != data.revision
            {
                return;
            }
            let mime = if mime_type == "text/plain;charset=utf-8" {
                "text/plain"
            } else {
                &mime_type
            };
            if !state.formats.contains_key(mime) || state.transfers.load(Ordering::Relaxed) >= 8 {
                return;
            }
            let formats = state.formats.clone();
            let mime = mime.to_owned();
            let active = state.transfers.clone();
            let stop = state.stop.clone();
            active.fetch_add(1, Ordering::Relaxed);
            thread::spawn(move || {
                let _ = write_pipe(fd, &formats[&mime], &stop);
                active.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }
}
