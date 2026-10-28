//! Pinned, non-V8 PDFium. Every native call is serialized; no PDF actions run.
use super::{Glyph, Page, PdfBytes, TextLayer, json};
use libloading::Library;
use std::{
    ffi::{c_char, c_void},
    sync::{Arc, Mutex, OnceLock},
};
type Handle = *mut c_void;
pub(super) const MAX_TEXT_UNITS: usize = 262_144;

pub(super) fn check_text_limit(length: usize) -> Result<(), String> {
    if length > MAX_TEXT_UNITS {
        Err(
            "This page exceeds the 262,144-character text limit. Text selection is unavailable."
                .into(),
        )
    } else {
        Ok(())
    }
}

macro_rules! bindings {
    ($($name:ident: $ty:ty => $symbol:literal),* $(,)?) => {
        struct Engine { _library: Library, $($name: $ty,)* }
        impl Engine {
            fn load() -> Result<Self, String> {
                let executable = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut directory = executable.parent().ok_or("Missing application directory.")?;
                if directory.file_name().is_some_and(|name| name == "deps") {
                    directory = directory.parent().ok_or("Missing test application directory.")?;
                }
                let path = directory.join("pdfium/pdfium.dll");
                // SAFETY: the absolute bundled path identifies the pinned library.
                // Its dependencies may resolve only alongside it or in trusted OS
                // locations. Function signatures match the shipped PDFium headers.
                unsafe {
                    let library: Library = libloading::os::windows::Library::load_with_flags(
                        path, 0x100 | 0x1000,
                    ).map_err(|e| format!("Cannot load the bundled PDF renderer: {e}"))?.into();
                    let engine = Self { $($name: *library.get::<$ty>($symbol)
                        .map_err(|e| format!("Incompatible PDF renderer: {e}"))?,)* _library: library };
                    (engine.init)();
                    Ok(engine)
                }
            }
        }
    }
}
bindings! {
    init: unsafe extern "system" fn() -> () => b"FPDF_InitLibrary\0",
    load: unsafe extern "system" fn(*const c_void, usize, *const c_char) -> Handle => b"FPDF_LoadMemDocument64\0",
    close: unsafe extern "system" fn(Handle) -> () => b"FPDF_CloseDocument\0",
    error: unsafe extern "system" fn() -> u32 => b"FPDF_GetLastError\0",
    permissions: unsafe extern "system" fn(Handle) -> u32 => b"FPDF_GetDocPermissions\0",
    count: unsafe extern "system" fn(Handle) -> i32 => b"FPDF_GetPageCount\0",
    page: unsafe extern "system" fn(Handle, i32) -> Handle => b"FPDF_LoadPage\0",
    close_page: unsafe extern "system" fn(Handle) -> () => b"FPDF_ClosePage\0",
    width: unsafe extern "system" fn(Handle) -> f32 => b"FPDF_GetPageWidthF\0",
    height: unsafe extern "system" fn(Handle) -> f32 => b"FPDF_GetPageHeightF\0",
    bitmap: unsafe extern "system" fn(i32, i32, i32) -> Handle => b"FPDFBitmap_Create\0",
    close_bitmap: unsafe extern "system" fn(Handle) -> () => b"FPDFBitmap_Destroy\0",
    fill: unsafe extern "system" fn(Handle, i32, i32, i32, i32, u32) -> i32 => b"FPDFBitmap_FillRect\0",
    draw: unsafe extern "system" fn(Handle, Handle, i32, i32, i32, i32, i32, i32) -> () => b"FPDF_RenderPageBitmap\0",
    buffer: unsafe extern "system" fn(Handle) -> *const u8 => b"FPDFBitmap_GetBuffer\0",
    stride: unsafe extern "system" fn(Handle) -> i32 => b"FPDFBitmap_GetStride\0",
    text: unsafe extern "system" fn(Handle) -> Handle => b"FPDFText_LoadPage\0",
    close_text: unsafe extern "system" fn(Handle) -> () => b"FPDFText_ClosePage\0",
    chars: unsafe extern "system" fn(Handle) -> i32 => b"FPDFText_CountChars\0",
    unicode: unsafe extern "system" fn(Handle, i32) -> u32 => b"FPDFText_GetUnicode\0",
    bounds: unsafe extern "system" fn(Handle, i32, *mut f64, *mut f64, *mut f64, *mut f64) -> i32 => b"FPDFText_GetCharBox\0",
    device: unsafe extern "system" fn(Handle, i32, i32, i32, i32, i32, f64, f64, *mut i32, *mut i32) -> i32 => b"FPDF_PageToDevice\0",
}

struct Owned(Handle, unsafe extern "system" fn(Handle));
impl Owned {
    fn new(handle: Handle, close: unsafe extern "system" fn(Handle)) -> Result<Self, String> {
        if handle.is_null() {
            Err("Cannot read this PDF page.".into())
        } else {
            Ok(Self(handle, close))
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is uniquely owned and drops under the engine lock,
        // before its parent document and library. No handle escapes this worker.
        unsafe { (self.1)(self.0) }
    }
}

pub fn render(bytes: PdfBytes, page_number: usize) -> Result<Page, String> {
    static ENGINE: OnceLock<Result<Mutex<Engine>, String>> = OnceLock::new();
    let engine = ENGINE
        .get_or_init(|| Engine::load().map(Mutex::new))
        .as_ref()
        .map_err(Clone::clone)?;
    let engine = engine
        .lock()
        .map_err(|_| "The PDF renderer stopped unexpectedly.")?;
    // SAFETY: immutable PDF bytes outlive all document/page/text handles. The
    // engine lock covers opening, rendering, extraction and every native drop.
    unsafe {
        let document = (engine.load)(bytes.as_ptr().cast(), bytes.len(), std::ptr::null());
        if document.is_null() {
            return Err(if (engine.error)() == 4 {
                "This PDF is password-protected."
            } else {
                "Cannot read this PDF document."
            }
            .into());
        }
        let document = Owned::new(document, engine.close)?;
        let pages = (engine.count)(document.0);
        if page_number == 0 || page_number > pages.max(0) as usize {
            return Err("PDF page is out of range.".into());
        }
        let page = Owned::new(
            (engine.page)(document.0, (page_number - 1) as i32),
            engine.close_page,
        )?;
        let (w, h) = (
            (engine.width)(page.0) as f64,
            (engine.height)(page.0) as f64,
        );
        if !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
            return Err("PDF page has invalid dimensions.".into());
        }
        let scale = (2400. / w.max(h)).min(2.);
        let width = (w * scale).ceil().clamp(1., 2400.) as i32;
        let height = (h * scale).ceil().clamp(1., 2400.) as i32;
        let bitmap = Owned::new((engine.bitmap)(width, height, 1), engine.close_bitmap)?;
        if (engine.fill)(bitmap.0, 0, 0, width, height, u32::MAX) == 0 {
            return Err("Cannot initialize PDF preview.".into());
        }
        (engine.draw)(bitmap.0, page.0, 0, 0, width, height, 0, 0);
        let pointer = (engine.buffer)(bitmap.0);
        let stride = (engine.stride)(bitmap.0);
        if pointer.is_null() || stride < width * 4 {
            return Err("Invalid PDF pixel buffer.".into());
        }
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for row in 0..height as usize {
            pixels.extend_from_slice(std::slice::from_raw_parts(
                pointer.add(row * stride as usize),
                width as usize * 4,
            ));
        }
        let mut data = json!({"kind":"pdf", "page":page_number,"pages":pages,
            "width":width,"height":height,"naturalWidth":w,"naturalHeight":h});
        let text = match engine.extract(document.0, page.0, width, height) {
            Ok(text) => text,
            Err(error) => {
                data["textError"] = error.into();
                TextLayer::default()
            }
        };
        Ok(Page {
            data,
            pixels,
            bytes,
            text: Arc::new(text),
        })
    }
}

impl Engine {
    unsafe fn extract(
        &self,
        document: Handle,
        page: Handle,
        width: i32,
        height: i32,
    ) -> Result<TextLayer, String> {
        // SAFETY: called under the engine lock with live document and page.
        unsafe {
            if (self.permissions)(document) & 0x10 == 0 {
                return Err(
                    "This PDF does not permit copying. Text selection is unavailable.".into(),
                );
            }
            let text = Owned::new((self.text)(page), self.close_text)?;
            let count = (self.chars)(text.0);
            if count < 0 {
                return Err("Cannot read PDF text. Text selection is unavailable.".into());
            }
            check_text_limit(count as usize)?;
            let mut layer = TextLayer::default();
            // PageToDevice returns integers. Preserve subpixel glyph geometry
            // before normalizing, especially for wide pages scaled down to fit.
            let (width, height) = (width * 1024, height * 1024);
            let mut index = 0;
            while index < count {
                let i = index;
                let mut scalar = (self.unicode)(text.0, index);
                index += 1;
                // Some encodings expose surrogate pairs as two source indices;
                // combine their text and boxes into one UTF-8 glyph range.
                if (0xd800..=0xdbff).contains(&scalar) && index < count {
                    let low = (self.unicode)(text.0, index);
                    if (0xdc00..=0xdfff).contains(&low) {
                        scalar = 0x10000 + ((scalar - 0xd800) << 10) + low - 0xdc00;
                        index += 1;
                    }
                }
                // PDFium generates CRLF; match the platform-independent text layer.
                if scalar == 13 && i + 1 < count && (self.unicode)(text.0, i + 1) == 10 {
                    continue;
                }
                let scalar = char::from_u32(scalar)
                    .filter(|&c| c != '\0')
                    .unwrap_or(char::REPLACEMENT_CHARACTER);
                let start = layer.text.len();
                layer.text.push(scalar);
                if scalar.is_control() {
                    continue;
                }
                let mut bounds = [width, height, 0, 0];
                for source in i..index {
                    let (mut left, mut right, mut bottom, mut top) = (0., 0., 0., 0.);
                    if (self.bounds)(text.0, source, &mut left, &mut right, &mut bottom, &mut top)
                        == 0
                        || ![left, right, bottom, top].iter().all(|v| v.is_finite())
                    {
                        continue;
                    }
                    for (x, y) in [(left, bottom), (left, top), (right, bottom), (right, top)] {
                        let (mut dx, mut dy) = (0, 0);
                        if (self.device)(page, 0, 0, width, height, 0, x, y, &mut dx, &mut dy) == 0
                        {
                            return Err(
                                "Cannot position PDF text. Text selection is unavailable.".into()
                            );
                        }
                        bounds[0] = bounds[0].min(dx.clamp(0, width));
                        bounds[1] = bounds[1].min(dy.clamp(0, height));
                        bounds[2] = bounds[2].max(dx.clamp(0, width));
                        bounds[3] = bounds[3].max(dy.clamp(0, height));
                    }
                }
                if bounds[2] > bounds[0] && bounds[3] > bounds[1] {
                    layer.glyphs.push(Glyph {
                        range: start..layer.text.len(),
                        bounds: [
                            bounds[0] as f32 / width as f32,
                            bounds[1] as f32 / height as f32,
                            (bounds[2] - bounds[0]) as f32 / width as f32,
                            (bounds[3] - bounds[1]) as f32 / height as f32,
                        ],
                    });
                }
            }
            Ok(layer)
        }
    }
}
