//! A server's operating system, and install commands for its package manager.

/// A server's distribution, from `/etc/os-release`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerOs {
    /// `ID`, for example `ubuntu`.
    pub id: String,
    /// `ID_LIKE`, for example `["debian"]`.
    pub id_like: Vec<String>,
    /// `VERSION_ID`, for example `24.04`.
    pub version_id: String,
    /// `PRETTY_NAME`, for example `Ubuntu 24.04 LTS`.
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackageManager {
    Apt,
    Dnf,
    Yum,
    Zypper,
    Pacman,
    Apk,
    /// Void Linux.
    Xbps,
    /// Gentoo. Names are package atoms; `atom[flag]` also enables a USE flag.
    Emerge,
    Nix,
}

impl ServerOs {
    /// Parse the contents of `/etc/os-release`.
    pub fn parse(os_release: &str) -> Self {
        let mut os = Self::default();
        for line in os_release.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unquote(value.trim());
            match key.trim() {
                "ID" => os.id = value.to_lowercase(),
                "ID_LIKE" => os.id_like = value.split_whitespace().map(str::to_lowercase).collect(),
                "VERSION_ID" => os.version_id = value,
                "PRETTY_NAME" => os.name = value,
                "NAME" if os.name.is_empty() => os.name = value,
                _ => {}
            }
        }
        os
    }

    /// The distribution's own package manager, when known.
    pub fn package_manager(&self) -> Option<PackageManager> {
        let major = self
            .version_id
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok());
        std::iter::once(self.id.as_str())
            .chain(self.id_like.iter().map(String::as_str))
            .find_map(|id| match id {
                "debian" | "ubuntu" => Some(PackageManager::Apt),
                "fedora" | "openeuler" => Some(PackageManager::Dnf),
                // Before version 8, Red Hat family systems ship yum only.
                "rhel" | "centos" | "rocky" | "almalinux" | "ol" => {
                    Some(if major.is_some_and(|major| major < 8) {
                        PackageManager::Yum
                    } else {
                        PackageManager::Dnf
                    })
                }
                "amzn" => Some(if major == Some(2) {
                    PackageManager::Yum
                } else {
                    PackageManager::Dnf
                }),
                "suse" | "opensuse" | "sles" => Some(PackageManager::Zypper),
                "arch" => Some(PackageManager::Pacman),
                "alpine" => Some(PackageManager::Apk),
                "void" => Some(PackageManager::Xbps),
                "gentoo" => Some(PackageManager::Emerge),
                "nixos" => Some(PackageManager::Nix),
                id if id.starts_with("opensuse") => Some(PackageManager::Zypper),
                _ => None,
            })
    }
}

fn unquote(value: &str) -> String {
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    value.replace("\\\"", "\"").replace("\\\\", "\\")
}

/// A package's name in each package manager that has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageNames(Vec<(PackageManager, String)>);

impl PackageNames {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, manager: PackageManager, name: impl Into<String>) -> Self {
        self.0.retain(|(existing, _)| *existing != manager);
        self.0.push((manager, name.into()));
        self
    }
    /// The name for a package manager. Yum shares dnf's package names.
    pub fn get(&self, manager: PackageManager) -> Option<&str> {
        let find = |manager| {
            self.0
                .iter()
                .find(|(existing, _)| *existing == manager)
                .map(|(_, name)| name.as_str())
        };
        find(manager).or_else(|| match manager {
            PackageManager::Yum => find(PackageManager::Dnf),
            _ => None,
        })
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl PackageManager {
    /// The command a user runs to install packages.
    pub fn install_command(self, packages: &[&str]) -> String {
        let list = packages
            .iter()
            .map(|p| quote(p))
            .collect::<Vec<_>>()
            .join(" ");
        match self {
            // Each refreshes the package index first, so a stale one can't
            // point at packages the mirrors no longer have.
            Self::Apt => format!("sudo apt update && sudo apt install {list}"),
            Self::Dnf => format!("sudo dnf install --refresh {list}"),
            Self::Yum => format!("sudo yum makecache && sudo yum install {list}"),
            Self::Zypper => format!("sudo zypper refresh && sudo zypper install {list}"),
            // Arch supports no partial upgrades: sync means upgrading too.
            Self::Pacman => format!("sudo pacman -Syu --needed {list}"),
            Self::Apk => format!("sudo apk update && sudo apk add {list}"),
            Self::Xbps => format!("sudo xbps-install -S {list}"),
            Self::Emerge => {
                // A needed USE flag goes in package.use first; emerge then
                // installs the package, or rebuilds it with the flag.
                // package.use may be one file or a directory of files.
                let mut command = String::new();
                let mut atoms = Vec::new();
                for package in packages {
                    match package.split_once('[') {
                        Some((atom, flags)) => {
                            if command.is_empty() {
                                command.push_str(
                                    "f=/etc/portage/package.use; [ -f \"$f\" ] || { sudo mkdir -p \"$f\" && f=\"$f/newport-remote\"; }; ",
                                );
                            }
                            let line =
                                format!("{atom} {}", flags.trim_end_matches(']').replace(',', " "));
                            command.push_str(&format!(
                                "echo {} | sudo tee -a \"$f\" >/dev/null && ",
                                quote(&line)
                            ));
                            atoms.push(quote(atom));
                        }
                        None => atoms.push(quote(package)),
                    }
                }
                command.push_str(&format!(
                    "sudo emerge --update --changed-use {}",
                    atoms.join(" ")
                ));
                command
            }
            Self::Nix => {
                let attributes: Vec<_> = packages
                    .iter()
                    .map(|p| quote(&format!("nixos.{p}")))
                    .collect();
                format!("nix-env -iA {}", attributes.join(" "))
            }
        }
    }
}

/// A word for the user's shell: as is when it is plain, single-quoted otherwise.
fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:+@=".contains(&b))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The install command for these packages on this OS, when both are known.
pub fn install_command(os: Option<&ServerOs>, packages: &PackageNames) -> Option<String> {
    let manager = os?.package_manager()?;
    let name = packages.get(manager)?;
    Some(manager.install_command(&[name]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributions_map_to_their_package_manager() {
        let cases = [
            (
                "ID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"",
                Some(PackageManager::Apt),
            ),
            (
                "ID=linuxmint\nID_LIKE=\"ubuntu debian\"",
                Some(PackageManager::Apt),
            ),
            ("ID=fedora\nVERSION_ID=41", Some(PackageManager::Dnf)),
            (
                "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\nVERSION_ID=\"9.4\"",
                Some(PackageManager::Dnf),
            ),
            (
                "ID=\"centos\"\nID_LIKE=\"rhel fedora\"\nVERSION_ID=\"7\"",
                Some(PackageManager::Yum),
            ),
            ("ID=\"amzn\"\nVERSION_ID=\"2\"", Some(PackageManager::Yum)),
            (
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"",
                Some(PackageManager::Zypper),
            ),
            ("ID=endeavouros\nID_LIKE=arch", Some(PackageManager::Pacman)),
            ("ID=alpine\nVERSION_ID=3.20.0", Some(PackageManager::Apk)),
            (
                "ID=\"void\"\nNAME=\"Void Linux\"",
                Some(PackageManager::Xbps),
            ),
            ("ID=gentoo\nNAME=Gentoo", Some(PackageManager::Emerge)),
            ("ID=funtoo\nID_LIKE=gentoo", Some(PackageManager::Emerge)),
            (
                "ID=\"openEuler\"\nVERSION_ID=\"24.03\"",
                Some(PackageManager::Dnf),
            ),
            ("ID=manjaro\nID_LIKE=arch", Some(PackageManager::Pacman)),
            (
                "ID=pop\nID_LIKE=\"ubuntu debian\"",
                Some(PackageManager::Apt),
            ),
            ("ID=nixos\nID_LIKE=\"\"", Some(PackageManager::Nix)),
            ("ID=slackware", None),
        ];
        for (release, manager) in cases {
            assert_eq!(
                ServerOs::parse(release).package_manager(),
                manager,
                "{release}"
            );
        }
    }

    #[test]
    fn commands_use_the_os_package_name() {
        let names = PackageNames::new()
            .with(PackageManager::Apt, "pipewire-bin")
            .with(PackageManager::Dnf, "pipewire-utils")
            .with(PackageManager::Nix, "pipewire");
        let os = |release| Some(ServerOs::parse(release));
        assert_eq!(
            install_command(os("ID=debian").as_ref(), &names).as_deref(),
            Some("sudo apt update && sudo apt install pipewire-bin")
        );
        assert_eq!(
            install_command(os("ID=centos\nVERSION_ID=7").as_ref(), &names).as_deref(),
            Some("sudo yum makecache && sudo yum install pipewire-utils")
        );
        assert_eq!(
            install_command(os("ID=nixos").as_ref(), &names).as_deref(),
            Some("nix-env -iA nixos.pipewire")
        );
        assert_eq!(install_command(os("ID=alpine").as_ref(), &names), None);
        assert_eq!(install_command(None, &names), None);
        let xvfb = PackageNames::new()
            .with(PackageManager::Xbps, "xorg-server-xvfb")
            .with(PackageManager::Emerge, "x11-base/xorg-server[xvfb]")
            .with(PackageManager::Nix, "xorg.xvfb");
        assert_eq!(
            install_command(os("ID=void").as_ref(), &xvfb).as_deref(),
            Some("sudo xbps-install -S xorg-server-xvfb")
        );
        assert_eq!(
            install_command(os("ID=gentoo").as_ref(), &xvfb).as_deref(),
            Some(
                "f=/etc/portage/package.use; [ -f \"$f\" ] || { sudo mkdir -p \"$f\" && f=\"$f/newport-remote\"; }; echo 'x11-base/xorg-server xvfb' | sudo tee -a \"$f\" >/dev/null && sudo emerge --update --changed-use x11-base/xorg-server"
            )
        );
        assert_eq!(
            install_command(
                os("ID=gentoo").as_ref(),
                &PackageNames::new().with(PackageManager::Emerge, "sys-apps/dbus")
            )
            .as_deref(),
            Some("sudo emerge --update --changed-use sys-apps/dbus")
        );
        assert_eq!(
            install_command(os("ID=nixos").as_ref(), &xvfb).as_deref(),
            Some("nix-env -iA nixos.xorg.xvfb")
        );
        // Every manager with an index refreshes it before installing.
        for (manager, command) in [
            (PackageManager::Apt, "sudo apt update && sudo apt install x"),
            (PackageManager::Dnf, "sudo dnf install --refresh x"),
            (
                PackageManager::Yum,
                "sudo yum makecache && sudo yum install x",
            ),
            (
                PackageManager::Zypper,
                "sudo zypper refresh && sudo zypper install x",
            ),
            (PackageManager::Pacman, "sudo pacman -Syu --needed x"),
            (PackageManager::Apk, "sudo apk update && sudo apk add x"),
            (PackageManager::Xbps, "sudo xbps-install -S x"),
        ] {
            assert_eq!(manager.install_command(&["x"]), command);
        }
        // Unusual names reach the shell as one quoted word.
        assert_eq!(
            PackageManager::Apt.install_command(&["a b; rm"]),
            "sudo apt update && sudo apt install 'a b; rm'"
        );
        let parsed = ServerOs::parse("PRETTY_NAME=\"NixOS 26.05 (Yarara)\"\nNAME=NixOS");
        assert_eq!(parsed.name, "NixOS 26.05 (Yarara)");
    }
}
