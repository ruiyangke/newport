//! Filename hints affect presentation only; preview decoding inspects bytes.
use super::*;
use newport_protocol::desktop::files::EntryKind;

fn asset(name: &str, kind: EntryKind) -> Option<&'static str> {
    match kind {
        EntryKind::Symlink => return Some("file-icons/symlink.svg"),
        EntryKind::Directory | EntryKind::Other => return None,
        EntryKind::File => {}
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
pub(super) fn icon(name: &str, kind: EntryKind) -> AnyElement {
    if let Some(path) = asset(name, kind) {
        img(path).size(px(16.)).flex_shrink_0().into_any_element()
    } else {
        Icon::new(if kind == EntryKind::Directory {
            IconName::Folder
        } else {
            IconName::File
        })
        .size(px(16.))
        .into_any_element()
    }
}

/// Explorer icons use shape for file type; Git status owns the color channel.
pub(super) fn tree_icon(name: &str, kind: EntryKind, color: Hsla) -> AnyElement {
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
    use super::{EntryKind, asset};
    #[test]
    fn kinds_and_special_names_precede_extensions() {
        let file = EntryKind::File;
        assert_eq!(asset("image.png", EntryKind::Directory), None);
        assert_eq!(asset("image.png", EntryKind::Other), None);
        assert_eq!(
            asset("main.rs", EntryKind::Symlink),
            Some("file-icons/symlink.svg")
        );
        assert_eq!(
            asset("id_ed25519.pub", file),
            Some("file-icons/ssh-key.svg")
        );
        assert_eq!(asset(".env.production", file), asset(".env", file));
        assert_eq!(asset("MAIN.RS", file), asset("main.rs", file));
        assert_ne!(asset("cargo.toml", file), asset("other.toml", file));
        assert_eq!(asset("unknown.zzzz", file), None);
    }
}
