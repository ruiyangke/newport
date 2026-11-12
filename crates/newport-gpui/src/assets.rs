use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = crate::connections_process_icon::asset(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = crate::file_icon_assets::asset(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if path == "newport.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../resources/icons/icon.png"
            ))));
        }
        gpui_kit::assets::AllAssets.load(path)
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A debug build that read icons from this machine's source would show
    /// none on another computer, as a debug package copied to a VM does.
    #[test]
    fn icons_are_built_into_the_binary() {
        let icon = Assets.load("icons/terminal.svg").unwrap();
        assert!(matches!(icon, Some(Cow::Borrowed(_))), "read from disk");
    }
}
