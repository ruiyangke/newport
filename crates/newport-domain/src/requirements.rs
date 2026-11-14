//! What Remote apps need on a server, what a check found missing, and how
//! to fix it. The backend reports [`Gap`]s; the window words and fixes them
//! from here, so the checklist and its alerts say the same thing.
use crate::server_os::{PackageManager, PackageNames, ServerOs};
use newport_protocol::desktop::applications::{
    Elevation, Finding, Gap, Host, Inactive, Requirement,
};

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

pub fn name(requirement: Requirement) -> &'static str {
    match requirement {
        Requirement::Dbus => "D-Bus",
        Requirement::Xwayland => "XWayland",
        Requirement::Fonts => "Fonts",
        Requirement::Gles => "OpenGL",
        Requirement::Portal => "GTK portal",
        Requirement::Audio => "Sound",
    }
}

/// What needs `requirement`.
pub fn purpose(requirement: Requirement) -> &'static str {
    match requirement {
        Requirement::Dbus => "Starting apps",
        Requirement::Xwayland => "X11 apps and dialogs",
        Requirement::Fonts => "Text in apps",
        Requirement::Gles => "Apps that draw with OpenGL",
        Requirement::Portal => "File dialogs in Snap and Flatpak apps",
        Requirement::Audio => "Sound from apps",
    }
}

/// What to install, as advice names it: "Install {noun}".
pub fn noun(requirement: Requirement) -> &'static str {
    match requirement {
        Requirement::Dbus => "D-Bus",
        Requirement::Xwayland => "XWayland",
        Requirement::Fonts => "a font",
        Requirement::Gles => "OpenGL",
        Requirement::Portal => "the GTK portal",
        Requirement::Audio => "a sound server",
    }
}

/// Why a server without `requirement` falls short.
fn without(requirement: Requirement, server: &str) -> String {
    match requirement {
        Requirement::Dbus => format!("Remote apps need {} on {server}.", name(requirement)),
        Requirement::Xwayland => {
            format!("Some apps use X11, and {server} has no XWayland for them.")
        }
        Requirement::Fonts => format!("{server} has no fonts, so apps can’t show text."),
        Requirement::Gles => {
            format!("Apps that draw with OpenGL, such as GTK 4 apps, need it on {server}.")
        }
        Requirement::Portal => {
            format!("Snap and Flatpak apps need the GTK portal on {server} for file dialogs.")
        }
        Requirement::Audio => format!("{server} has no sound server, so apps are silent."),
    }
}

/// Without `requirement` apps don't start or can't show text.
pub fn blocks(requirement: Requirement) -> bool {
    matches!(requirement, Requirement::Dbus | Requirement::Fonts)
}

/// The package that provides `requirement` on `host`, in each package manager.
pub fn packages(requirement: Requirement, os: Option<&ServerOs>, host: &Host) -> PackageNames {
    use PackageManager::*;
    let names: [(PackageManager, &str); 8] = match requirement {
        Requirement::Dbus => [
            (Apt, "dbus"),
            (Dnf, "dbus-daemon"),
            (Zypper, zypper_dbus(os)),
            (Pacman, "dbus"),
            (Apk, "dbus"),
            (Xbps, "dbus"),
            (Emerge, "sys-apps/dbus"),
            (Nix, "dbus"),
        ],
        Requirement::Xwayland => [
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
        Requirement::Fonts => [
            (Apt, "fonts-dejavu-core"),
            (Dnf, "dejavu-sans-mono-fonts"),
            (Zypper, "dejavu-fonts"),
            (Pacman, "ttf-dejavu"),
            (Apk, "font-dejavu"),
            (Xbps, "dejavu-fonts-ttf"),
            (Emerge, "media-fonts/dejavu"),
            (Nix, "dejavu_fonts"),
        ],
        Requirement::Gles => [
            (Apt, "libgles2"),
            (Dnf, "libglvnd-gles"),
            (Zypper, "libglvnd"),
            (Pacman, "libglvnd"),
            (Apk, "mesa-gles"),
            (Xbps, "libglvnd"),
            (Emerge, "media-libs/libglvnd"),
            (Nix, "libglvnd"),
        ],
        Requirement::Portal => [
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
        Requirement::Audio if !host.user_services => [
            (Apt, "pulseaudio"),
            (Dnf, "pulseaudio"),
            (Zypper, "pulseaudio"),
            (Pacman, "pulseaudio"),
            (Apk, "pulseaudio"),
            (Xbps, "pulseaudio"),
            (Emerge, "media-sound/pulseaudio-daemon"),
            (Nix, "pulseaudio"),
        ],
        Requirement::Audio => [
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

/// The NixOS setting that provides `requirement`.
pub fn nixos(requirement: Requirement) -> &'static str {
    match requirement {
        Requirement::Dbus => "services.dbus.enable = true;",
        Requirement::Xwayland => "programs.xwayland.enable = true;",
        Requirement::Fonts => "fonts.packages = [ pkgs.dejavu_fonts ];",
        Requirement::Gles => "hardware.graphics.enable = true;",
        Requirement::Portal => {
            "xdg.portal = { enable = true; extraPortals = [ pkgs.xdg-desktop-portal-gtk ]; };"
        }
        Requirement::Audio => "services.pipewire = { enable = true; pulse.enable = true; };",
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

/// A package install fixes `gap`.
pub fn installs(gap: &Gap) -> bool {
    matches!(gap.finding, Finding::Missing(_) | Finding::Outdated(_))
}

/// Only some apps or features need it, so its alert can be dismissed.
pub fn optional(gap: &Gap) -> bool {
    !blocks(gap.requirement) || matches!(gap.finding, Finding::Inactive(_))
}

/// The alert's heading, for example "XWayland is missing".
pub fn title(gap: &Gap) -> String {
    let r = gap.requirement;
    match &gap.finding {
        Finding::Missing(_) => match r {
            Requirement::Fonts => "No fonts are installed".into(),
            Requirement::Audio => "No sound".into(),
            Requirement::Dbus | Requirement::Xwayland => {
                format!("{} is missing", name(r))
            }
            Requirement::Gles => "Apps that draw with OpenGL can’t start".into(),
            Requirement::Portal => "No file dialogs in Snap and Flatpak apps".into(),
        },
        Finding::Outdated(_) => format!("{} is too old", name(r)),
        Finding::Inactive(reason) => match **reason {
            Inactive::Root => "No sound for root".into(),
            Inactive::Stopped => format!("{} isn’t running", name(r)),
            Inactive::Failed => format!("{} didn’t start", name(r)),
        },
        Finding::Unverified(_) => format!("{} couldn’t be checked", name(r)),
    }
}

/// What's wrong on `server` and what to do, matching [`remedy`].
pub fn detail(gap: &Gap, server: &str, os: Option<&ServerOs>, host: &Host) -> String {
    let r = gap.requirement;
    let after = "then check again.";
    match &gap.finding {
        Finding::Missing(_) | Finding::Outdated(_) => {
            let problem = match &gap.finding {
                Finding::Outdated(version) => format!(
                    "{server} has XWayland {version}; Newport needs {XWAYLAND_MINIMUM} or newer."
                ),
                _ => without(r, server),
            };
            format!("{problem} {}", install_advice(noun(r), os, host, after))
        }
        Finding::Inactive(reason) => match **reason {
            Inactive::Root => format!(
                "PipeWire runs only for regular users, so apps you open as root on {server} are silent. Connect as a regular user to hear them."
            ),
            Inactive::Stopped => format!(
                "PipeWire is installed on {server} but its sound server isn’t running. Start it, {after}"
            ),
            Inactive::Failed => format!(
                "D-Bus is installed on {server}, but a session bus didn’t start. Check that `dbus-run-session -- true` works there, {after}"
            ),
        },
        Finding::Unverified(_) => format!(
            "Newport couldn’t check {} on {server}: `Xwayland -version` failed, or `timeout` is missing. Check that it reports {XWAYLAND_MINIMUM} or newer, {after}",
            name(r)
        ),
    }
}

/// How to fix `gap` on its own.
pub fn remedy(gap: &Gap, os: Option<&ServerOs>, host: &Host) -> Remedy {
    match &gap.finding {
        Finding::Missing(_) | Finding::Outdated(_) => {
            install_remedy(std::slice::from_ref(gap), os, host)
        }
        Finding::Inactive(reason) if **reason == Inactive::Stopped && host.user_services => {
            Remedy::Command("systemctl --user enable --now pipewire-pulse.socket".into())
        }
        Finding::Inactive(_) | Finding::Unverified(_) => Remedy::Manual,
    }
}

/// One fix for every gap a package install fixes.
pub fn install_remedy(gaps: &[Gap], os: Option<&ServerOs>, host: &Host) -> Remedy {
    let gaps: Vec<_> = gaps.iter().filter(|gap| installs(gap)).collect();
    let Some(manager) = os.and_then(ServerOs::package_manager) else {
        return Remedy::Manual;
    };
    if gaps.is_empty() {
        return Remedy::Manual;
    }
    if manager == PackageManager::Nix {
        let settings: Vec<_> = gaps.iter().map(|gap| nixos(gap.requirement)).collect();
        return Remedy::Configure(settings.join("\n"));
    }
    let names: Vec<_> = gaps
        .iter()
        .map(|gap| packages(gap.requirement, os, host))
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
    let nouns: Vec<_> = gaps.iter().map(|gap| noun(gap.requirement)).collect();
    match nouns.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
#[path = "requirements_tests.rs"]
mod tests;
