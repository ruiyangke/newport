use super::*;
use newport_protocol::desktop::applications::Updater;

fn os(release: &str) -> Option<ServerOs> {
    Some(ServerOs::parse(release))
}

fn gap(requirement: Requirement, finding: Finding) -> Gap {
    Gap {
        requirement,
        finding,
    }
}

fn missing(requirement: Requirement) -> Gap {
    gap(requirement, Finding::Missing(Box::default()))
}

fn inactive(requirement: Requirement, reason: Inactive) -> Gap {
    gap(requirement, Finding::Inactive(Box::new(reason)))
}

/// An account that elevates with sudo, on a system without an image
/// updater or a systemd user manager.
fn sudo() -> Host {
    host(Elevation::Sudo)
}

fn host(elevation: Elevation) -> Host {
    Host {
        elevation,
        updater: None,
        user_services: false,
    }
}

#[test]
fn every_gap_a_package_fixes_installs_in_one_command() {
    let debian = os("ID=debian\nPRETTY_NAME=\"Debian GNU/Linux 13\"");
    let gaps = [
        missing(Requirement::Dbus),
        missing(Requirement::Fonts),
        inactive(Requirement::Audio, Inactive::Root),
    ];
    assert_eq!(
        install_remedy(&gaps, debian.as_ref(), &host(Elevation::Sudo)),
        Remedy::Command(
            "sudo apt update && sudo apt install --no-install-recommends dbus fonts-dejavu-core"
                .into()
        )
    );
    assert_eq!(listed(&gaps[..2]), "D-Bus and a font");
    assert_eq!(listed(&gaps), "D-Bus, a font and a sound server");
}

#[test]
fn commands_follow_the_account_and_the_system() {
    let cases = [
        // A root login on Arch has no sudo.
        (
            "ID=arch",
            host(Elevation::Root),
            Requirement::Dbus,
            Remedy::Command("pacman -Syu --needed dbus".into()),
            "Install D-Bus, then check again.",
        ),
        (
            "ID=alpine",
            host(Elevation::Doas),
            Requirement::Gles,
            Remedy::Command("doas apk update && doas apk add mesa-gles".into()),
            "Install OpenGL, then check again.",
        ),
        (
            "ID=debian",
            host(Elevation::Unavailable),
            Requirement::Dbus,
            Remedy::Manual,
            "Install D-Bus as root, then check again.",
        ),
        (
            "ID=fedora\nVARIANT_ID=silverblue",
            Host {
                updater: Some(Updater::RpmOstree),
                ..sudo()
            },
            Requirement::Xwayland,
            Remedy::Command("sudo rpm-ostree install xorg-x11-server-Xwayland".into()),
            "Install XWayland and restart the server, then check again.",
        ),
        (
            "ID=\"opensuse-microos\"\nID_LIKE=\"suse opensuse\"",
            Host {
                updater: Some(Updater::TransactionalUpdate),
                ..sudo()
            },
            Requirement::Fonts,
            Remedy::Command("sudo transactional-update pkg install dejavu-fonts".into()),
            "Install a font and restart the server, then check again.",
        ),
        (
            "ID=\"opensuse-leap\"\nVERSION_ID=\"15.6\"",
            sudo(),
            Requirement::Dbus,
            Remedy::Command("sudo zypper refresh && sudo zypper install dbus-1".into()),
            "Install D-Bus, then check again.",
        ),
        (
            "ID=nixos\nPRETTY_NAME=\"NixOS 26.05 (Yarara)\"",
            sudo(),
            Requirement::Xwayland,
            Remedy::Configure("programs.xwayland.enable = true;".into()),
            "Add XWayland to your NixOS configuration and rebuild, then check again.",
        ),
        (
            "ID=slackware\nPRETTY_NAME=\"Slackware 15.0\"",
            sudo(),
            Requirement::Dbus,
            Remedy::Manual,
            "Install D-Bus with Slackware 15.0’s package manager, then check again.",
        ),
    ];
    for (release, host, requirement, expected, advice) in cases {
        let os = os(release);
        let gap = missing(requirement);
        assert_eq!(remedy(&gap, os.as_ref(), &host), expected, "{release}");
        let detail = detail(&gap, "box", os.as_ref(), &host);
        assert!(detail.ends_with(advice), "{release}: {detail}");
    }
}

#[test]
fn installed_software_that_doesnt_run_is_never_installed_again() {
    let arch = os("ID=arch");
    let root = inactive(Requirement::Audio, Inactive::Root);
    assert_eq!(title(&root), "No sound for root");
    assert_eq!(
        remedy(&root, arch.as_ref(), &host(Elevation::Root)),
        Remedy::Manual
    );
    assert!(optional(&root) && !installs(&root));
    assert_eq!(
        install_remedy(
            std::slice::from_ref(&root),
            arch.as_ref(),
            &host(Elevation::Root)
        ),
        Remedy::Manual
    );
    let stopped = inactive(Requirement::Audio, Inactive::Stopped);
    let systemd = Host {
        user_services: true,
        ..sudo()
    };
    assert_eq!(
        remedy(&stopped, arch.as_ref(), &systemd),
        Remedy::Command("systemctl --user enable --now pipewire-pulse.socket".into())
    );
    assert_eq!(remedy(&stopped, arch.as_ref(), &sudo()), Remedy::Manual);
    let failed = inactive(Requirement::Dbus, Inactive::Failed);
    assert_eq!(title(&failed), "D-Bus didn’t start");
    assert!(detail(&failed, "box", arch.as_ref(), &sudo()).contains("dbus-run-session"));
}

#[test]
fn sound_without_systemd_comes_from_a_pulseaudio_needle_starts() {
    let alpine = os("ID=alpine");
    let audio = [missing(Requirement::Audio)];
    assert_eq!(
        install_remedy(&audio, alpine.as_ref(), &sudo()),
        Remedy::Command("sudo apk update && sudo apk add pulseaudio".into())
    );
    let systemd = Host {
        user_services: true,
        ..sudo()
    };
    assert_eq!(
        install_remedy(&audio, os("ID=debian").as_ref(), &systemd),
        Remedy::Command("sudo apt update && sudo apt install pipewire-pulse".into())
    );
}

#[test]
fn opensuse_names_dbus_by_release() {
    let dbus = |release| {
        packages(Requirement::Dbus, os(release).as_ref(), &sudo())
            .get(PackageManager::Zypper)
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        dbus("ID=\"opensuse-tumbleweed\"\nVERSION_ID=\"20261018\""),
        "dbus-1-daemon"
    );
    assert_eq!(
        dbus("ID=\"opensuse-leap\"\nVERSION_ID=\"16.0\""),
        "dbus-1-daemon"
    );
    assert_eq!(dbus("ID=\"opensuse-leap\"\nVERSION_ID=\"15.6\""), "dbus-1");
    assert_eq!(dbus("ID=\"sles\"\nVERSION_ID=\"15.7\""), "dbus-1");
}

#[test]
fn an_old_xwayland_names_its_version() {
    let outdated = gap(
        Requirement::Xwayland,
        Finding::Outdated(Box::new("22.1.9".into())),
    );
    assert_eq!(title(&outdated), "XWayland is too old");
    assert!(installs(&outdated) && optional(&outdated));
    assert_eq!(
        detail(&outdated, "box", os("ID=ubuntu").as_ref(), &sudo()),
        "box has XWayland 22.1.9; Newport needs 23.1 or newer. Install XWayland, then check again."
    );
}

#[test]
fn xwayland_is_offered_never_required() {
    let missing = missing(Requirement::Xwayland);
    assert!(installs(&missing) && optional(&missing));
    assert_eq!(title(&missing), "XWayland is missing");
    assert_eq!(
        detail(&missing, "box", os("ID=ubuntu").as_ref(), &sudo()),
        "Some apps use X11, and box has no XWayland for them. Install XWayland, then check again."
    );
}

#[test]
fn every_requirement_has_a_package_for_every_manager() {
    use PackageManager::*;
    for requirement in [
        Requirement::Dbus,
        Requirement::Xwayland,
        Requirement::Fonts,
        Requirement::Gles,
        Requirement::Portal,
        Requirement::Audio,
    ] {
        for user_services in [false, true] {
            let host = Host {
                user_services,
                ..sudo()
            };
            let names = packages(requirement, None, &host);
            for manager in [Apt, Dnf, Yum, Zypper, Pacman, Apk, Xbps, Emerge, Nix] {
                assert!(names.get(manager).is_some(), "{requirement:?} {manager:?}");
            }
        }
        assert!(nixos(requirement).ends_with(';'));
    }
}
