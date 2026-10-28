//! What Remote apps need on a server, what a check found missing, and how
//! to fix it. The backend reports [`Gap`]s; the window words and fixes them
//! from here, so the checklist and its alerts say the same thing.
use crate::server_os::{Elevation, PackageManager, PackageNames, ServerOs, Updater};
use serde::{Deserialize, Serialize};

/// Software apps on a Newport display need from the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Requirement {
    Dbus,
    Xwayland,
    Fonts,
    Gles,
    Portal,
    Audio,
}

/// What a check found wrong with a requirement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Finding {
    Missing,
    /// Installed, older than Newport needs.
    Outdated {
        version: String,
    },
    /// Installed, but it doesn't run for this account.
    Inactive {
        reason: Inactive,
    },
    /// The check couldn't tell.
    Unverified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Inactive {
    /// It runs only for regular users.
    Root,
    /// Its user service is installed but stopped.
    Stopped,
    /// It is installed, but didn't start when tried.
    Failed,
}

/// A requirement the server doesn't meet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub requirement: Requirement,
    #[serde(flatten)]
    pub finding: Finding,
}

/// What the server's account can do about gaps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub elevation: Elevation,
    pub updater: Option<Updater>,
    /// The account has a systemd user manager to start services with.
    #[serde(default)]
    pub user_services: bool,
}

/// How to fix one or more gaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Remedy {
    /// A command to run in a terminal on the server.
    Command(String),
    /// NixOS settings to add to the system configuration.
    Configure(String),
    /// Nothing Newport can run; the detail says what to do.
    Manual,
}

/// XWayland's oldest supported version.
pub const XWAYLAND_MINIMUM: &str = "23.1";

impl Requirement {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dbus => "D-Bus",
            Self::Xwayland => "XWayland",
            Self::Fonts => "Fonts",
            Self::Gles => "OpenGL ES",
            Self::Portal => "GTK portal",
            Self::Audio => "Sound",
        }
    }

    /// What needs it.
    pub fn purpose(self) -> &'static str {
        match self {
            Self::Dbus => "Starting apps",
            Self::Xwayland => "X11 apps",
            Self::Fonts => "Text in apps",
            Self::Gles => "GTK 4 apps",
            Self::Portal => "File dialogs in Snap and Flatpak apps",
            Self::Audio => "Sound from apps",
        }
    }

    /// What to install, as advice names it: "Install {noun}".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Dbus => "D-Bus",
            Self::Xwayland => "XWayland",
            Self::Fonts => "a font",
            Self::Gles => "OpenGL ES",
            Self::Portal => "the GTK portal",
            Self::Audio => "a sound server",
        }
    }

    /// Why a server without it falls short.
    fn without(self, server: &str) -> String {
        match self {
            Self::Dbus | Self::Xwayland => format!("Remote apps need {} on {server}.", self.name()),
            Self::Fonts => format!("{server} has no fonts, so apps can’t show text."),
            Self::Gles => format!("GTK 4 apps need OpenGL ES on {server}."),
            Self::Portal => {
                format!("Snap and Flatpak apps need the GTK portal on {server} for file dialogs.")
            }
            Self::Audio => format!("{server} has no sound server, so apps are silent."),
        }
    }

    /// Without it apps don't start or can't show text.
    pub fn blocks(self) -> bool {
        matches!(self, Self::Dbus | Self::Xwayland | Self::Fonts)
    }

    /// The package that provides it on `host`, in each package manager.
    pub fn packages(self, os: Option<&ServerOs>, host: &Host) -> PackageNames {
        use PackageManager::*;
        let names: [(PackageManager, &str); 8] = match self {
            Self::Dbus => [
                (Apt, "dbus"),
                (Dnf, "dbus-daemon"),
                (Zypper, zypper_dbus(os)),
                (Pacman, "dbus"),
                (Apk, "dbus"),
                (Xbps, "dbus"),
                (Emerge, "sys-apps/dbus"),
                (Nix, "dbus"),
            ],
            Self::Xwayland => [
                (Apt, "xwayland"),
                (Dnf, "xorg-x11-server-Xwayland"),
                (Zypper, "xwayland"),
                (Pacman, "xorg-xwayland"),
                (Apk, "xwayland"),
                (Xbps, "xorg-server-xwayland"),
                (Emerge, "x11-base/xwayland"),
                (Nix, "xwayland"),
            ],
            // Every name installs DejaVu Sans Mono, the face apps fall back to.
            Self::Fonts => [
                (Apt, "fonts-dejavu-core"),
                (Dnf, "dejavu-sans-mono-fonts"),
                (Zypper, "dejavu-fonts"),
                (Pacman, "ttf-dejavu"),
                (Apk, "font-dejavu"),
                (Xbps, "dejavu-fonts-ttf"),
                (Emerge, "media-fonts/dejavu"),
                (Nix, "dejavu_fonts"),
            ],
            Self::Gles => [
                (Apt, "libgles2"),
                (Dnf, "libglvnd-gles"),
                (Zypper, "libglvnd"),
                (Pacman, "libglvnd"),
                (Apk, "mesa-gles"),
                (Xbps, "libglvnd"),
                (Emerge, "media-libs/libglvnd"),
                (Nix, "libglvnd"),
            ],
            Self::Portal => [
                (Apt, "xdg-desktop-portal-gtk"),
                (Dnf, "xdg-desktop-portal-gtk"),
                (Zypper, "xdg-desktop-portal-gtk"),
                (Pacman, "xdg-desktop-portal-gtk"),
                (Apk, "xdg-desktop-portal-gtk"),
                (Xbps, "xdg-desktop-portal-gtk"),
                (Emerge, "sys-apps/xdg-desktop-portal-gtk"),
                (Nix, "xdg-desktop-portal-gtk"),
            ],
            // Without a systemd user manager nothing starts PipeWire's pulse
            // server, so Needle starts a PulseAudio of its own instead.
            Self::Audio if !host.user_services => [
                (Apt, "pulseaudio"),
                (Dnf, "pulseaudio"),
                (Zypper, "pulseaudio"),
                (Pacman, "pulseaudio"),
                (Apk, "pulseaudio"),
                (Xbps, "pulseaudio"),
                (Emerge, "media-sound/pulseaudio-daemon"),
                (Nix, "pulseaudio"),
            ],
            Self::Audio => [
                (Apt, "pipewire-pulse"),
                (Dnf, "pipewire-pulseaudio"),
                (Zypper, "pipewire-pulseaudio"),
                (Pacman, "pipewire-pulse"),
                (Apk, "pipewire-pulse"),
                (Xbps, "pipewire"),
                (Emerge, "media-video/pipewire"),
                (Nix, "pipewire"),
            ],
        };
        names
            .into_iter()
            .fold(PackageNames::new(), |names, (manager, name)| {
                names.with(manager, name)
            })
    }

    /// The NixOS setting that provides it.
    pub fn nixos(self) -> &'static str {
        match self {
            Self::Dbus => "services.dbus.enable = true;",
            Self::Xwayland => "programs.xwayland.enable = true;",
            Self::Fonts => "fonts.packages = [ pkgs.dejavu_fonts ];",
            Self::Gles => "hardware.graphics.enable = true;",
            Self::Portal => {
                "xdg.portal = { enable = true; extraPortals = [ pkgs.xdg-desktop-portal-gtk ]; };"
            }
            Self::Audio => "services.pipewire = { enable = true; pulse.enable = true; };",
        }
    }
}

/// openSUSE Leap and SLE before 16 ship the daemon in `dbus-1`.
fn zypper_dbus(os: Option<&ServerOs>) -> &'static str {
    let major = os.and_then(|os| os.version_id.split('.').next()?.parse::<u32>().ok());
    match (os.map(|os| os.id.as_str()), major) {
        (Some("opensuse-leap" | "sles" | "sled" | "sle-micro"), Some(major)) if major < 16 => {
            "dbus-1"
        }
        _ => "dbus-1-daemon",
    }
}

impl Gap {
    /// A package install fixes it.
    pub fn installs(&self) -> bool {
        matches!(self.finding, Finding::Missing | Finding::Outdated { .. })
    }

    /// Only some apps or features need it, so its alert can be dismissed.
    pub fn optional(&self) -> bool {
        !self.requirement.blocks() || matches!(self.finding, Finding::Inactive { .. })
    }

    /// The alert's heading, for example "XWayland is missing".
    pub fn title(&self) -> String {
        let r = self.requirement;
        match &self.finding {
            Finding::Missing => match r {
                Requirement::Fonts => "No fonts are installed".into(),
                Requirement::Audio => "No sound".into(),
                Requirement::Dbus | Requirement::Xwayland => format!("{} is missing", r.name()),
                Requirement::Gles => "GTK 4 apps can’t start".into(),
                Requirement::Portal => "No file dialogs in Snap and Flatpak apps".into(),
            },
            Finding::Outdated { .. } => format!("{} is too old", r.name()),
            Finding::Inactive {
                reason: Inactive::Root,
            } => "No sound for root".into(),
            Finding::Inactive {
                reason: Inactive::Stopped,
            } => format!("{} isn’t running", r.name()),
            Finding::Inactive {
                reason: Inactive::Failed,
            } => format!("{} didn’t start", r.name()),
            Finding::Unverified => format!("{} couldn’t be checked", r.name()),
        }
    }

    /// What's wrong on `server` and what to do, matching [`Gap::remedy`].
    pub fn detail(&self, server: &str, os: Option<&ServerOs>, host: &Host) -> String {
        let r = self.requirement;
        let after = "then check again.";
        match &self.finding {
            Finding::Missing | Finding::Outdated { .. } => {
                let problem = match &self.finding {
                    Finding::Outdated { version } => format!(
                        "{server} has XWayland {version}; Newport needs {XWAYLAND_MINIMUM} or newer."
                    ),
                    _ => r.without(server),
                };
                format!("{problem} {}", install_advice(r.noun(), os, host, after))
            }
            Finding::Inactive {
                reason: Inactive::Root,
            } => format!(
                "PipeWire runs only for regular users, so apps you open as root on {server} are silent. Connect as a regular user to hear them."
            ),
            Finding::Inactive {
                reason: Inactive::Stopped,
            } => format!(
                "PipeWire is installed on {server} but its sound server isn’t running. Start it, {after}"
            ),
            Finding::Inactive {
                reason: Inactive::Failed,
            } => format!(
                "D-Bus is installed on {server}, but a session bus didn’t start. Check that `dbus-run-session -- true` works there, {after}"
            ),
            Finding::Unverified => format!(
                "Newport couldn’t check {} on {server}: `Xwayland -version` failed, or `timeout` is missing. Check that it reports {XWAYLAND_MINIMUM} or newer, {after}",
                r.name()
            ),
        }
    }

    /// How to fix it on its own.
    pub fn remedy(&self, os: Option<&ServerOs>, host: &Host) -> Remedy {
        match &self.finding {
            Finding::Missing | Finding::Outdated { .. } => {
                install_remedy(std::slice::from_ref(self), os, host)
            }
            Finding::Inactive {
                reason: Inactive::Stopped,
            } if host.user_services => {
                Remedy::Command("systemctl --user enable --now pipewire-pulse.socket".into())
            }
            Finding::Inactive { .. } | Finding::Unverified => Remedy::Manual,
        }
    }
}

/// One fix for every gap a package install fixes.
pub fn install_remedy(gaps: &[Gap], os: Option<&ServerOs>, host: &Host) -> Remedy {
    let gaps: Vec<_> = gaps.iter().filter(|gap| gap.installs()).collect();
    let Some(manager) = os.and_then(ServerOs::package_manager) else {
        return Remedy::Manual;
    };
    if gaps.is_empty() {
        return Remedy::Manual;
    }
    if manager == PackageManager::Nix {
        let settings: Vec<_> = gaps.iter().map(|gap| gap.requirement.nixos()).collect();
        return Remedy::Configure(settings.join("\n"));
    }
    let names: Vec<_> = gaps
        .iter()
        .map(|gap| gap.requirement.packages(os, host))
        .collect();
    let Some(names) = names
        .iter()
        .map(|names| names.get(manager))
        .collect::<Option<Vec<_>>>()
    else {
        return Remedy::Manual;
    };
    // Debian's recommendations pull in systemd, even as init, on a server
    // that runs without it. Where systemd runs, PipeWire's recommended
    // session manager is what makes it play sound.
    manager
        .install(&names, host.elevation, host.updater, host.user_services)
        .map_or(Remedy::Manual, Remedy::Command)
}

/// What to do about missing or outdated software, matching its remedy:
/// "Install {what}, then check again."
pub fn install_advice(what: &str, os: Option<&ServerOs>, host: &Host, after: &str) -> String {
    let named = os
        .filter(|os| !os.name.is_empty())
        .map(|os| os.name.as_str());
    match os.and_then(ServerOs::package_manager) {
        Some(PackageManager::Nix) => {
            format!("Add {what} to your NixOS configuration and rebuild, {after}")
        }
        Some(_) if host.elevation == Elevation::Unavailable => {
            format!("Install {what} as root, {after}")
        }
        Some(_) if host.updater.is_some() => {
            format!("Install {what} and restart the server, {after}")
        }
        Some(_) => format!("Install {what}, {after}"),
        None => match named {
            Some(name) => format!("Install {what} with {name}’s package manager, {after}"),
            None => format!("Install {what} with the server’s package manager, {after}"),
        },
    }
}

/// The software several gaps need, for one alert: "D-Bus and a font".
pub fn listed(gaps: &[Gap]) -> String {
    let nouns: Vec<_> = gaps.iter().map(|gap| gap.requirement.noun()).collect();
    match nouns.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
#[path = "requirements_tests.rs"]
mod tests;
