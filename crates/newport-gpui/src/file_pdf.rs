//! GPUI image adapter over the shared native PDF renderer.
use gpui_kit::RenderImage;
pub use newport_platform::pdf::{PdfBytes, TextLayer, decode};
use serde_json::Value;
use std::sync::Arc;

pub struct Page {
    pub data: Value,
    pub image: Arc<RenderImage>,
    pub bytes: PdfBytes,
    pub text: Arc<TextLayer>,
}

pub fn render(bytes: PdfBytes, page_number: usize) -> Result<Page, String> {
    let page = newport_platform::pdf::render(bytes, page_number)?;
    let width = page.data["width"].as_u64().ok_or("Missing PDF width.")? as u32;
    let height = page.data["height"].as_u64().ok_or("Missing PDF height.")? as u32;
    let frame = image::RgbaImage::from_raw(width, height, page.pixels)
        .ok_or("Cannot allocate PDF preview.")?;
    Ok(Page {
        data: page.data,
        image: Arc::new(RenderImage::new(vec![image::Frame::new(frame)])),
        bytes: page.bytes,
        text: page.text,
    })
}
