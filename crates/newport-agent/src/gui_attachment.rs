//! Validate an inherited attachment without acquiring a display or replaying a
//! command. Current is a point-in-time observation, not an ownership receipt.
use crate::routing;
use newport_protocol::display::{attachment as protocol, environment::shared_terminal_v1};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    io::{self, Read},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
};

const FIELDS: [&str; 10] = [
    routing::STABLE_DISPLAY,
    "DISPLAY",
    "XAUTHORITY",
    "NEWPORT_GUI_DISPLAY",
    "NEWPORT_GUI_AUTHORITY",
    "WAYLAND_DISPLAY",
    "WAYLAND_SOCKET",
    "NEWPORT_GUI_WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "NEWPORT_GUI_RUNTIME_DIR",
];
const GUIDANCE: &str = "Open or reconnect a current Newport terminal before running this command. The inherited GUI attachment was not replaced.";

struct Claim(BTreeMap<&'static str, Option<OsString>>);
impl Claim {
    fn capture(mut get: impl FnMut(&str) -> Option<OsString>) -> Self {
        Self(FIELDS.into_iter().map(|name| (name, get(name))).collect())
    }
    fn value(&self, name: &str) -> io::Result<Option<&str>> {
        self.0[name]
            .as_ref()
            .map(|value| {
                value
                    .to_str()
                    .ok_or_else(|| io::Error::other(format!("Invalid inherited {name}")))
            })
            .transpose()
    }
    fn required(&self, name: &str) -> io::Result<&str> {
        self.value(name)?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| io::Error::other(format!("Missing inherited {name}")))
    }
    /// The stable display's folder, for a claim that names it.
    fn stable(&self) -> io::Result<Option<&str>> {
        self.value(routing::STABLE_DISPLAY)?
            .map(|folder| {
                Some(folder)
                    .filter(|folder| shared_terminal_v1::folder(folder))
                    .ok_or_else(|| io::Error::other("Invalid inherited NEWPORT_DISPLAY"))
            })
            .transpose()
    }
    /// A shell on the display names its endpoints alone. A program launched
    /// on it also carries markers, which must name the same ones.
    fn endpoints(&self) -> io::Result<protocol::Endpoints> {
        if self.0["WAYLAND_SOCKET"].is_some() {
            return Err(io::Error::other(
                "An inherited Wayland fd is not a shared GUI pathname attachment",
            ));
        }
        let marked = shared_terminal_v1::MARKERS
            .iter()
            .any(|(_, marker)| self.0[marker].is_some());
        let display = self.value("DISPLAY")?;
        let authority = self.value("XAUTHORITY")?;
        if display.is_some() != authority.is_some()
            || marked
                && (display != self.value("NEWPORT_GUI_DISPLAY")?
                    || authority != self.value("NEWPORT_GUI_AUTHORITY")?)
        {
            return Err(io::Error::other(
                "Inherited X11 endpoints do not match the GUI attachment markers",
            ));
        }
        let wayland = self.required("WAYLAND_DISPLAY")?;
        if marked && Some(wayland) != self.value("NEWPORT_GUI_WAYLAND_DISPLAY")? {
            return Err(io::Error::other(
                "Inherited Wayland endpoint does not match the GUI attachment marker",
            ));
        }
        let wayland_socket = if Path::new(wayland).is_absolute() {
            wayland.to_owned()
        } else {
            let runtime = self.required("XDG_RUNTIME_DIR")?;
            if marked && Some(runtime) != self.value("NEWPORT_GUI_RUNTIME_DIR")? {
                return Err(io::Error::other(
                    "Relative Wayland endpoint has no matching GUI runtime marker",
                ));
            }
            Path::new(runtime)
                .join(wayland)
                .to_str()
                .ok_or_else(|| io::Error::other("Invalid Wayland endpoint"))?
                .to_owned()
        };
        Ok(protocol::Endpoints {
            display: display.map(str::to_owned),
            authority: authority.map(str::to_owned),
            wayland_socket,
        })
    }
    /// A claim on the stable display is current while it names the running
    /// display's endpoints. No Mac owns the display, so none is asked, and
    /// none needs to view it.
    fn check_stable(&self, folder: &Path) -> io::Result<()> {
        if self.endpoints()? != running_stable_display(folder)? {
            return Err(io::Error::other(
                "Inherited GUI attachment names another display than the stable display",
            ));
        }
        Ok(())
    }
}

/// Where the account's stable display keeps its socket and files.
pub(crate) fn stable_display_folder() -> Option<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty())?;
    Some(Path::new(&runtime).join(shared_terminal_v1::DIRECTORY))
}

/// The endpoints the stable display in `folder` names while it runs. Only a
/// folder private to this account is the account's display: another account
/// could put any endpoints in one it can write.
pub(crate) fn running_stable_display(folder: &Path) -> io::Result<protocol::Endpoints> {
    if !fs::symlink_metadata(folder).is_ok_and(|m| {
        m.is_dir() && m.uid() == crate::paths::effective_uid() && m.mode() & 0o077 == 0
    }) {
        return Err(io::Error::other(
            "The stable display's folder isn't private",
        ));
    }
    UnixStream::connect(folder.join("display.sock"))
        .map_err(|_| io::Error::other("The stable display isn't running"))?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(folder.join("environment.json"))?;
    let mut bytes = Vec::new();
    file.take(64 * 1024).read_to_end(&mut bytes)?;
    let values: BTreeMap<String, Option<String>> = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("Invalid stable display environment"))?;
    Claim::capture(|name| values.get(name).cloned().flatten().map(OsString::from)).endpoints()
}

/// Whether the account's stable display runs, and the computer viewing it,
/// for a Mac that remembers another computer showing it. The viewer's
/// session tells computers apart, as names needn't; its name is null when
/// that computer gave none. The viewer is null while none views.
pub(crate) fn live_stable_display(
    folder: Option<&Path>,
) -> newport_protocol::monitor::stable_display::State {
    use newport_protocol::monitor::stable_display::{State, Viewer};
    let Some(folder) = folder.filter(|folder| running_stable_display(folder).is_ok()) else {
        return State::default();
    };
    let viewer = routing::viewer(folder).map(|viewer| {
        let computer = Some(viewer.computer).filter(|computer| !computer.is_empty());
        Viewer {
            session: viewer.session,
            computer,
        }
    });
    State {
        running: true,
        viewer,
    }
}

/// An inherited claim is current while it names the running stable
/// display: the one in the folder it names, or else the account's.
pub(crate) fn validate() -> io::Result<()> {
    let claim = Claim::capture(|name| env::var_os(name));
    let result = (|| {
        let folder = match claim.stable()? {
            Some(folder) => PathBuf::from(folder),
            None => stable_display_folder()
                .ok_or_else(|| io::Error::other("The stable display isn't running"))?,
        };
        claim.check_stable(&folder)
    })();
    result.map_err(|error| io::Error::new(error.kind(), format!("{error}. {GUIDANCE}")))
}

#[cfg(test)]
#[path = "gui_attachment_client_tests.rs"]
mod tests;
