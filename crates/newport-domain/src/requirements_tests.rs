use super::*;
use serde_json::json;

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
    gap(requirement, Finding::Missing)
}

fn host(elevation: Elevation) -> Host {
    Host {
        elevation,
        ..Host::default()
    }
}

#[test]
fn every_gap_a_package_fixes_installs_in_one_command() {
    let debian = os("ID=debian\nPRETTY_NAME=\"Debian GNU/Linux 13\"");
    let gaps = [
        missing(Requirement::Dbus),
        missing(Requirement::Fonts),
        gap(
            Requirement::Audio,
            Finding::Inactive {
                reason: Inactive::Root,
            },
        ),
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
            Host::default().with(Elevation::Root),
            Requirement::Dbus,
            Remedy::Command("pacman -Syu --needed dbus".into()),
            "Install D-Bus, then check again.",
        ),
        (
            "ID=alpine",
            host(Elevation::Doas),
            Requirement::Gles,
            Remedy::Command("doas apk update && doas apk add mesa-gles".into()),
            "Install OpenGL ES, then check again.",
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
                ..Host::default()
            },
            Requirement::Xwayland,
            Remedy::Command("sudo rpm-ostree install xorg-x11-server-Xwayland".into()),
            "Install XWayland and restart the server, then check again.",
        ),
        (
            "ID=\"opensuse-microos\"\nID_LIKE=\"suse opensuse\"",
            Host {
                updater: Some(Updater::TransactionalUpdate),
                ..Host::default()
            },
            Requirement::Fonts,
            Remedy::Command("sudo transactional-update pkg install dejavu-fonts".into()),
            "Install a font and restart the server, then check again.",
        ),
        (
            "ID=\"opensuse-leap\"\nVERSION_ID=\"15.6\"",
            Host::default(),
            Requirement::Dbus,
            Remedy::Command("sudo zypper refresh && sudo zypper install dbus-1".into()),
            "Install D-Bus, then check again.",
        ),
        (
            "ID=nixos\nPRETTY_NAME=\"NixOS 26.05 (Yarara)\"",
            Host::default(),
            Requirement::Xwayland,
            Remedy::Configure("programs.xwayland.enable = true;".into()),
            "Add XWayland to your NixOS configuration and rebuild, then check again.",
        ),
        (
            "ID=slackware\nPRETTY_NAME=\"Slackware 15.0\"",
            Host::default(),
            Requirement::Dbus,
            Remedy::Manual,
            "Install D-Bus with Slackware 15.0’s package manager, then check again.",
        ),
    ];
    for (release, host, requirement, remedy, advice) in cases {
        let os = os(release);
        let gap = missing(requirement);
        assert_eq!(gap.remedy(os.as_ref(), &host), remedy, "{release}");
        let detail = gap.detail("box", os.as_ref(), &host);
        assert!(detail.ends_with(advice), "{release}: {detail}");
    }
}

#[test]
fn installed_software_that_doesnt_run_is_never_installed_again() {
    let arch = os("ID=arch");
    let root = gap(
        Requirement::Audio,
        Finding::Inactive {
            reason: Inactive::Root,
        },
    );
    assert_eq!(root.title(), "No sound for root");
    assert_eq!(
        root.remedy(arch.as_ref(), &host(Elevation::Root)),
        Remedy::Manual
    );
    assert!(root.optional() && !root.installs());
    assert_eq!(
        install_remedy(
            std::slice::from_ref(&root),
            arch.as_ref(),
            &host(Elevation::Root)
        ),
        Remedy::Manual
    );
    let stopped = gap(
        Requirement::Audio,
        Finding::Inactive {
            reason: Inactive::Stopped,
        },
    );
    let systemd = Host {
        user_services: true,
        ..Host::default()
    };
    assert_eq!(
        stopped.remedy(arch.as_ref(), &systemd),
        Remedy::Command("systemctl --user enable --now pipewire-pulse.socket".into())
    );
    assert_eq!(
        stopped.remedy(arch.as_ref(), &Host::default()),
        Remedy::Manual
    );
    let failed = gap(
        Requirement::Dbus,
        Finding::Inactive {
            reason: Inactive::Failed,
        },
    );
    assert_eq!(failed.title(), "D-Bus didn’t start");
    assert!(
        failed
            .detail("box", arch.as_ref(), &Host::default())
            .contains("dbus-run-session")
    );
}

#[test]
fn sound_without_systemd_comes_from_a_pulseaudio_needle_starts() {
    let alpine = os("ID=alpine");
    let audio = [missing(Requirement::Audio)];
    assert_eq!(
        install_remedy(&audio, alpine.as_ref(), &Host::default()),
        Remedy::Command("sudo apk update && sudo apk add pulseaudio".into())
    );
    let systemd = Host {
        user_services: true,
        ..Host::default()
    };
    assert_eq!(
        install_remedy(&audio, os("ID=debian").as_ref(), &systemd),
        Remedy::Command("sudo apt update && sudo apt install pipewire-pulse".into())
    );
}

#[test]
fn opensuse_names_dbus_by_release() {
    let dbus = |release| {
        Requirement::Dbus
            .packages(os(release).as_ref(), &Host::default())
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
        Finding::Outdated {
            version: "22.1.9".into(),
        },
    );
    assert_eq!(outdated.title(), "XWayland is too old");
    assert!(outdated.installs() && outdated.optional());
    assert_eq!(
        outdated.detail("box", os("ID=ubuntu").as_ref(), &Host::default()),
        "box has XWayland 22.1.9; Newport needs 23.1 or newer. Install XWayland, then check again."
    );
}

#[test]
fn xwayland_is_offered_never_required() {
    let missing = missing(Requirement::Xwayland);
    assert!(missing.installs() && missing.optional());
    assert_eq!(missing.title(), "XWayland is missing");
    assert_eq!(
        missing.detail("box", os("ID=ubuntu").as_ref(), &Host::default()),
        "Some apps use X11, and box has no XWayland for them. Install XWayland, then check again."
    );
}

#[test]
fn zenity_is_offered_never_required() {
    let missing = missing(Requirement::Dialogs);
    assert!(missing.installs() && missing.optional());
    assert_eq!(missing.title(), "Zenity is missing");
    assert_eq!(
        missing.detail("box", os("ID=debian").as_ref(), &Host::default()),
        "Apps such as Chromium show prompts with Zenity, and box has none, so they appear in a plain X11 window or not at all. Install Zenity, then check again."
    );
    assert_eq!(
        missing.remedy(os("ID=debian").as_ref(), &Host::default()),
        Remedy::Command(
            "sudo apt update && sudo apt install --no-install-recommends zenity".into()
        )
    );
    assert_eq!(
        serde_json::to_value(&missing).unwrap(),
        json!({"requirement": "dialogs", "state": "missing"})
    );
}

#[test]
fn gaps_cross_from_the_backend_as_tagged_json() {
    let gaps = vec![
        missing(Requirement::Fonts),
        gap(
            Requirement::Audio,
            Finding::Inactive {
                reason: Inactive::Root,
            },
        ),
        gap(
            Requirement::Xwayland,
            Finding::Outdated {
                version: "22.1.9".into(),
            },
        ),
    ];
    let value = serde_json::to_value(&gaps).unwrap();
    assert_eq!(
        value,
        json!([
            {"requirement": "fonts", "state": "missing"},
            {"requirement": "audio", "state": "inactive", "reason": "root"},
            {"requirement": "xwayland", "state": "outdated", "version": "22.1.9"},
        ])
    );
    assert_eq!(serde_json::from_value::<Vec<Gap>>(value).unwrap(), gaps);
    let host: Host = serde_json::from_value(json!({"elevation": "root", "updater": null})).unwrap();
    assert_eq!(host, Host::default().with(Elevation::Root));
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
        Requirement::Dialogs,
        Requirement::Audio,
    ] {
        for user_services in [false, true] {
            let host = Host {
                user_services,
                ..Host::default()
            };
            let names = requirement.packages(None, &host);
            for manager in [Apt, Dnf, Yum, Zypper, Pacman, Apk, Xbps, Emerge, Nix] {
                assert!(names.get(manager).is_some(), "{requirement:?} {manager:?}");
            }
        }
        assert!(requirement.nixos().ends_with(';'));
    }
}

impl Host {
    fn with(mut self, elevation: Elevation) -> Self {
        self.elevation = elevation;
        self
    }
}
