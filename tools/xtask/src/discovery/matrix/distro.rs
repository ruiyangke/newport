use clap::ValueEnum;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Distro {
    Debian12,
    Debian13,
    Ubuntu22,
    Ubuntu24,
    Alma9,
    Amazon2023,
    Arch,
    Alpine324,
    Fedora44,
    Opensuse16,
    Rocky9,
}

impl Distro {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Debian12 => "debian12",
            Self::Debian13 => "debian13",
            Self::Ubuntu22 => "ubuntu22",
            Self::Ubuntu24 => "ubuntu24",
            Self::Alma9 => "alma9",
            Self::Amazon2023 => "amazon2023",
            Self::Arch => "arch",
            Self::Alpine324 => "alpine324",
            Self::Fedora44 => "fedora44",
            Self::Opensuse16 => "opensuse16",
            Self::Rocky9 => "rocky9",
        }
    }

    pub(super) fn image(self) -> &'static str {
        match self {
            Self::Debian12 => "debian:12-slim",
            Self::Debian13 => "debian:13-slim",
            Self::Ubuntu22 => "ubuntu:22.04",
            Self::Ubuntu24 => "ubuntu:24.04",
            Self::Alma9 => "almalinux:9",
            Self::Amazon2023 => "amazonlinux:2023",
            Self::Arch => "archlinux:latest",
            Self::Alpine324 => "alpine:3.24",
            Self::Fedora44 => "fedora:44",
            Self::Opensuse16 => "opensuse/leap:16.0",
            Self::Rocky9 => "rockylinux:9-minimal",
        }
    }

    pub(super) fn packages(self) -> &'static str {
        match self {
            Self::Debian12 | Self::Debian13 | Self::Ubuntu22 | Self::Ubuntu24 => {
                "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends sudo passwd && apt-get clean && rm -rf /var/lib/apt/lists/*"
            }
            Self::Alma9 | Self::Amazon2023 | Self::Fedora44 => {
                "dnf install -y --setopt=install_weak_deps=False sudo shadow-utils && dnf clean all"
            }
            Self::Arch => {
                "pacman -Syu --disable-sandbox-syscalls --noconfirm --needed sudo shadow && pacman -Scc --noconfirm"
            }
            Self::Alpine324 => "apk add --no-cache sudo shadow",
            Self::Opensuse16 => {
                "zypper --non-interactive install --no-recommends sudo shadow && zypper clean --all"
            }
            Self::Rocky9 => "microdnf install -y sudo shadow-utils && microdnf clean all",
        }
    }

    pub(super) fn setup_note(self) -> Option<&'static str> {
        (self == Self::Arch).then_some(
            "Pacman download syscall filtering is disabled only during package setup: emulated seccomp can return EINVAL. Container security and Newport tests retain their defaults.",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
pub(super) enum Architecture {
    #[value(name = "x86_64")]
    X86_64,
    Aarch64,
}

impl Architecture {
    pub(super) fn platform(self) -> &'static str {
        match self {
            Self::X86_64 => "linux/amd64",
            Self::Aarch64 => "linux/arm64",
        }
    }

    pub(super) fn machine(self) -> u16 {
        match self {
            Self::X86_64 => 62,
            Self::Aarch64 => 183,
        }
    }
}
