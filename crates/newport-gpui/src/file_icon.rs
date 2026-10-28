//! Filename hints affect presentation only; preview decoding inspects bytes.
use super::*;

fn asset(name: &str, kind: &str) -> Option<&'static str> {
    if kind == "symlink" {
        return Some("file-icons/symlink.svg");
    }
    if kind != "file" {
        return None;
    }
    let name = name.to_lowercase();
    let key = name.strip_suffix(".pub").unwrap_or(&name);
    if matches!(
        key,
        "id_rsa" | "id_ed25519" | "id_ecdsa" | "id_dsa" | "authorized_keys" | "known_hosts"
    ) {
        return Some("file-icons/ssh-key.svg");
    }
    let name = if name.starts_with(".env.") {
        ".env"
    } else {
        &name
    };
    file_icon_assets::lookup(name).or_else(|| {
        name.rfind('.')
            .and_then(|index| file_icon_assets::lookup(&name[index..]))
    })
}
pub(super) fn icon(name: &str, kind: &str) -> AnyElement {
    if let Some(path) = asset(name, kind) {
        img(path).size(px(16.)).flex_shrink_0().into_any_element()
    } else {
        Icon::new(if kind == "directory" {
            IconName::Folder
        } else {
            IconName::File
        })
        .size(px(16.))
        .into_any_element()
    }
}

/// Explorer icons use shape for file type; Git status owns the color channel.
pub(super) fn tree_icon(name: &str, kind: &str, color: Hsla) -> AnyElement {
    let icon = if let Some(path) = asset(name, kind) {
        Icon::default().path(path)
    } else {
        Icon::new(IconName::File)
    };
    icon.size(px(16.))
        .text_color(color)
        .flex_shrink_0()
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::asset;
    #[test]
    fn kinds_and_special_names_precede_extensions() {
        assert_eq!(asset("image.png", "directory"), None);
        assert_eq!(asset("main.rs", "symlink"), Some("file-icons/symlink.svg"));
        assert_eq!(
            asset("id_ed25519.pub", "file"),
            Some("file-icons/ssh-key.svg")
        );
        assert_eq!(asset(".env.production", "file"), asset(".env", "file"));
        assert_eq!(asset("MAIN.RS", "file"), asset("main.rs", "file"));
        assert_ne!(asset("cargo.toml", "file"), asset("other.toml", "file"));
        assert_eq!(asset("unknown.zzzz", "file"), None);
    }
}
