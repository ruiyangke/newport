//! Every package the dependency advice names exists in its distribution's
//! repositories, asked of each package index in a container.
use newport_domain::requirements::{Host, Requirement};
use newport_domain::server_os::{PackageManager, ServerOs};
use std::process::Command;

const REQUIREMENTS: [Requirement; 6] = [
    Requirement::Dbus,
    Requirement::Xwayland,
    Requirement::Fonts,
    Requirement::Gles,
    Requirement::Portal,
    Requirement::Audio,
];

/// An image, its os-release, and a shell loop printing every name its index
/// lacks; `$names` holds the names.
const DISTRIBUTIONS: [(&str, &str, &str); 7] = [
    (
        "debian:trixie-slim",
        "ID=debian",
        "apt-get -qq update >/dev/null; for n in $names; do apt-cache show \"$n\" >/dev/null 2>&1 || echo \"$n\"; done",
    ),
    (
        "ubuntu:24.04",
        "ID=ubuntu\nID_LIKE=debian",
        "apt-get -qq update >/dev/null; for n in $names; do apt-cache show \"$n\" >/dev/null 2>&1 || echo \"$n\"; done",
    ),
    (
        "fedora:43",
        "ID=fedora",
        "for n in $names; do dnf -q info \"$n\" >/dev/null 2>&1 || echo \"$n\"; done",
    ),
    (
        "alpine:3.22",
        "ID=alpine",
        "apk update -q >/dev/null; for n in $names; do apk search -qx \"$n\" | grep -qx \"$n\" || echo \"$n\"; done",
    ),
    (
        "archlinux:latest",
        "ID=arch",
        "pacman -Sy --disable-sandbox >/dev/null 2>&1; for n in $names; do pacman -Si \"$n\" >/dev/null 2>&1 || echo \"$n\"; done",
    ),
    (
        "opensuse/leap:15.6",
        "ID=\"opensuse-leap\"\nVERSION_ID=\"15.6\"",
        "zypper -q refresh >/dev/null 2>&1; for n in $names; do zypper -q info \"$n\" 2>/dev/null | grep -q '^Name' || echo \"$n\"; done",
    ),
    (
        "opensuse/tumbleweed:latest",
        "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"",
        "zypper -q refresh >/dev/null 2>&1; for n in $names; do zypper -q info \"$n\" 2>/dev/null | grep -q '^Name' || echo \"$n\"; done",
    ),
];

#[test]
#[ignore = "needs Docker and network: cargo test -p newport-domain --test packages_exist -- --ignored"]
fn every_advised_package_exists() {
    let mut absent = Vec::new();
    for (image, release, check) in DISTRIBUTIONS {
        let os = ServerOs::parse(release);
        let manager: PackageManager = os.package_manager().unwrap();
        let mut names: Vec<String> = Vec::new();
        for user_services in [false, true] {
            let host = Host {
                user_services,
                ..Host::default()
            };
            for requirement in REQUIREMENTS {
                let name = requirement
                    .packages(Some(&os), &host)
                    .get(manager)
                    .unwrap()
                    .to_owned();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        let output = Command::new("docker")
            // Names don't differ by architecture; some images are amd64 only.
            .args(["run", "--rm", "--platform", "linux/amd64", "--env"])
            .arg(format!("names={}", names.join(" ")))
            .args([image, "sh", "-c", check])
            .output()
            .expect("docker runs");
        assert!(output.status.success(), "{image}: {output:?}");
        absent.extend(
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .map(|name| format!("{image}: {name}")),
        );
    }
    assert!(absent.is_empty(), "not in their repositories: {absent:#?}");
}
