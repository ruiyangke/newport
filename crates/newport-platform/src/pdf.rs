//! Inert PDF rasterization and text extraction. Native objects stay on one worker.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
#[cfg(any(target_os = "macos", windows))]
use serde_json::json;
use std::sync::Arc;

pub type PdfBytes = Arc<Vec<u8>>;
#[derive(Clone, Debug, Default)]
pub struct TextLayer {
    pub text: String,
    pub glyphs: Vec<Glyph>,
}

#[derive(Clone, Debug)]
pub struct Glyph {
    /// UTF-8 byte boundaries into `TextLayer::text` (one Unicode scalar).
    pub range: std::ops::Range<usize>,
    /// [x, y, width, height], normalized to the raster's top-left origin.
    /// Clipped to the visible page; whitespace may have no corresponding glyph.
    pub bounds: [f32; 4],
}

pub struct Page {
    pub data: Value,
    /// Premultiplied BGRA pixels, top row first.
    pub pixels: Vec<u8>,
    pub bytes: PdfBytes,
    pub text: Arc<TextLayer>,
}

pub fn decode(content: &str) -> Result<PdfBytes, String> {
    const LIMIT: usize = 16 * 1024 * 1024;
    if content.len() > LIMIT.div_ceil(3) * 4 {
        return Err("PDF preview exceeds 16 MiB.".into());
    }
    let bytes = STANDARD
        .decode(content)
        .map_err(|_| "Invalid PDF preview data.")?;
    if bytes.len() > LIMIT {
        return Err("PDF preview exceeds 16 MiB.".into());
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err("This file is not a PDF document.".into());
    }
    Ok(Arc::new(bytes))
}

/// Validate bundled PDF resources without reading a profile or contacting a server.
pub fn check_renderer() -> Result<(), String> {
    let page = render(
        Arc::new(include_bytes!("../tests/fixtures/pdf/plain.pdf").to_vec()),
        1,
    )?;
    if page.data["pages"] != 1
        || !page.text.text.contains("Newport PDF preview")
        || page.text.glyphs.is_empty()
        || page.pixels.is_empty()
    {
        return Err("The bundled PDF renderer failed its preview check.".into());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn render(_: PdfBytes, _: usize) -> Result<Page, String> {
    Err("PDF preview is unavailable on this platform.".into())
}

#[cfg(windows)]
mod windows;
#[cfg(all(windows, test))]
use windows as native;
#[cfg(windows)]
pub use windows::render;

#[cfg(target_os = "macos")]
pub fn render(bytes: PdfBytes, page_number: usize) -> Result<Page, String> {
    use native::*;
    // All borrowed pointers are scoped below. `bytes` and the bitmap outlive
    // their Core Graphics users, and each Create result has exactly one release.
    unsafe {
        let provider = Owned::new(
            CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                bytes.as_ptr().cast(),
                bytes.len(),
                None,
            ),
            CGDataProviderRelease,
        )?;
        let document = Owned::new(
            CGPDFDocumentCreateWithProvider(provider.0),
            CGPDFDocumentRelease,
        )?;
        if !CGPDFDocumentIsUnlocked(document.0) {
            return Err("This PDF is password-protected.".into());
        }
        let pages = CGPDFDocumentGetNumberOfPages(document.0);
        if page_number == 0 || page_number > pages {
            return Err("PDF page is out of range.".into());
        }
        let page = CGPDFDocumentGetPage(document.0, page_number);
        if page.is_null() {
            return Err("Cannot read this PDF page.".into());
        }
        let crop = CGPDFPageGetBoxRect(page, 1);
        let rotation = CGPDFPageGetRotationAngle(page);
        let (w, h) = if rotation.rem_euclid(180) == 90 {
            (crop.size.height, crop.size.width)
        } else {
            (crop.size.width, crop.size.height)
        };
        if !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
            return Err("PDF page has invalid dimensions.".into());
        }
        let scale = (2400. / w.max(h)).min(2.);
        let width = (w * scale).ceil().clamp(1., 2400.) as usize;
        let height = (h * scale).ceil().clamp(1., 2400.) as usize;
        let mut pixels = vec![255u8; width * height * 4];
        let colors = Owned::new(CGColorSpaceCreateDeviceRGB(), CGColorSpaceRelease)?;
        // 32-bit little-endian premultiplied-first is GPUI's BGRA layout.
        let context = Owned::new(
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                colors.0,
                2 | (2 << 12),
            ),
            CGContextRelease,
        )?;
        let target = Rect {
            origin: Point { x: 0., y: 0. },
            size: Size {
                width: w,
                height: h,
            },
        };
        // The PDF drawing transform scales down but does not enlarge a page.
        // Apply raster resolution explicitly before the crop/rotation transform.
        CGContextScaleCTM(context.0, scale, scale);
        let transform = CGPDFPageGetDrawingTransform(page, 1, target, 0, true);
        CGContextConcatCTM(context.0, transform);
        CGContextDrawPDFPage(context.0, page);
        drop(context);
        let mut data = json!({"kind":"pdf", "page":page_number,"pages":pages,
            "width":width,"height":height,"naturalWidth":w,"naturalHeight":h});
        let text = match extract_text(&bytes, page_number, transform, scale, width, height) {
            Ok(text) => text,
            Err(error) => {
                // A scanned page or restricted/oversized text layer must not
                // prevent viewing the PDF. Never return a truncated selection.
                data["textError"] = Value::String(error);
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

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod native {
    use super::{Glyph, TextLayer};
    use cocoa::{
        base::NO,
        foundation::{NSRange, NSRect},
    };
    use objc::{
        class, msg_send,
        rc::{StrongPtr, autoreleasepool},
        runtime::Object,
        sel, sel_impl,
    };
    use std::ffi::c_void;

    // Bounds returned to the UI occupy at most 8 MiB, and UTF-8 text at most
    // 1 MiB. This also bounds the number of native per-character lookups.
    pub const MAX_TEXT_UNITS: usize = 262_144;

    #[link(name = "PDFKit", kind = "framework")]
    unsafe extern "C" {}

    pub fn extract_text(
        bytes: &[u8],
        page_number: usize,
        transform: Transform,
        scale: f64,
        width: usize,
        height: usize,
    ) -> Result<TextLayer, String> {
        // This pool is local to the rendering worker, including on error paths.
        // NSData borrows `bytes` only until the owning PDFDocument is released.
        // Merely reading PDFPage text does not create a view, follow actions,
        // or execute the document's scripts, links, or annotations.
        autoreleasepool(|| unsafe {
            let data: *mut Object = msg_send![class!(NSData), alloc];
            let data: *mut Object = msg_send![data, initWithBytesNoCopy:bytes.as_ptr()
                length:bytes.len() freeWhenDone:NO];
            if data.is_null() {
                return Err("Cannot allocate PDF text data. Text selection is unavailable.".into());
            }
            let data = StrongPtr::new(data);
            let document: *mut Object = msg_send![class!(PDFDocument), alloc];
            let document: *mut Object = msg_send![document, initWithData:*data];
            if document.is_null() {
                return Err("Cannot read PDF text. Text selection is unavailable.".into());
            }
            let document = StrongPtr::new(document);
            let allows_copying: objc::runtime::BOOL = msg_send![*document, allowsCopying];
            if allows_copying == NO {
                return Err(
                    "This PDF does not permit copying. Text selection is unavailable.".into(),
                );
            }
            let page: *mut Object = msg_send![*document, pageAtIndex:page_number - 1];
            if page.is_null() {
                return Err("Cannot read this page's text. Text selection is unavailable.".into());
            }
            let count: usize = msg_send![page, numberOfCharacters];
            check_text_limit(count)?;
            if count == 0 {
                return Ok(TextLayer::default());
            }
            let string: *mut Object = msg_send![page, string];
            if string.is_null() {
                return Err("Cannot read this page's text. Text selection is unavailable.".into());
            }
            let length: usize = msg_send![string, length];
            check_text_limit(length)?;
            if length != count {
                return Err(
                    "This page's text positions are inconsistent. Text selection is unavailable."
                        .into(),
                );
            }
            let mut units = vec![0u16; length];
            let _: () = msg_send![string, getCharacters:units.as_mut_ptr()
                range:NSRange::new(0, length as u64)];
            let mut layer = TextLayer {
                text: String::with_capacity(length),
                glyphs: Vec::with_capacity(length),
            };
            let mut utf16_index = 0;
            for scalar in char::decode_utf16(units.iter().copied()) {
                // Malformed UTF-16 is represented by U+FFFD; an invalid unit
                // consumes one source position, so later ranges stay aligned.
                let (scalar, unit_count) = match scalar {
                    Ok(scalar) => (scalar, scalar.len_utf16()),
                    Err(_) => (char::REPLACEMENT_CHARACTER, 1),
                };
                let start = layer.text.len();
                layer.text.push(scalar);
                if !scalar.is_control() {
                    // Character-index bounds can diverge from NSString's
                    // UTF-16 indexing after linefeeds or ToUnicode expansions.
                    // Selection ranges use precisely the NSString positions.
                    // Drain each temporary selection immediately, so a large
                    // page does not accumulate native objects in the pool.
                    let bounds = autoreleasepool(|| {
                        let selection: *mut Object = msg_send![page, selectionForRange:
                            NSRange::new(utf16_index as u64, unit_count as u64)];
                        if selection.is_null() {
                            return None;
                        }
                        let rect: NSRect = msg_send![selection, boundsForPage:page];
                        normalized_bounds(rect, transform, scale, width, height)
                    });
                    if let Some(bounds) = bounds {
                        layer.glyphs.push(Glyph {
                            range: start..layer.text.len(),
                            bounds,
                        });
                    }
                }
                utf16_index += unit_count;
            }
            Ok(layer)
        })
    }

    pub fn check_text_limit(length: usize) -> Result<(), String> {
        if length > MAX_TEXT_UNITS {
            Err("This page exceeds the 262,144-character text limit. Text selection is unavailable.".into())
        } else {
            Ok(())
        }
    }

    fn normalized_bounds(
        rect: NSRect,
        transform: Transform,
        scale: f64,
        width: usize,
        height: usize,
    ) -> Option<[f32; 4]> {
        let x0 = rect.origin.x;
        let y0 = rect.origin.y;
        let x1 = x0 + rect.size.width;
        let y1 = y0 + rect.size.height;
        if ![x0, y0, x1, y1].iter().all(|n| n.is_finite()) || x1 <= x0 || y1 <= y0 {
            return None;
        }
        let mut left = f64::INFINITY;
        let mut top = f64::INFINITY;
        let mut right = f64::NEG_INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            // PDFKit reports unrotated page coordinates. Reuse the exact crop
            // and rotation transform used to draw, including raster rounding.
            let px = (transform.a * x + transform.c * y + transform.tx) * scale;
            let py = height as f64 - (transform.b * x + transform.d * y + transform.ty) * scale;
            left = left.min(px);
            top = top.min(py);
            right = right.max(px);
            bottom = bottom.max(py);
        }
        let x = (left / width as f64).clamp(0., 1.);
        let y = (top / height as f64).clamp(0., 1.);
        let right = (right / width as f64).clamp(0., 1.);
        let bottom = (bottom / height as f64).clamp(0., 1.);
        (right > x && bottom > y).then_some([
            x as f32,
            y as f32,
            (right - x) as f32,
            (bottom - y) as f32,
        ])
    }
    type Ptr = *mut c_void;
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Point {
        pub x: f64,
        pub y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Size {
        pub width: f64,
        pub height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Rect {
        pub origin: Point,
        pub size: Size,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Transform {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        tx: f64,
        ty: f64,
    }
    pub struct Owned(pub Ptr, unsafe extern "C" fn(Ptr));
    impl Owned {
        pub fn new(pointer: Ptr, release: unsafe extern "C" fn(Ptr)) -> Result<Self, String> {
            if pointer.is_null() {
                Err("Cannot open or render this PDF document.".into())
            } else {
                Ok(Self(pointer, release))
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                (self.1)(self.0);
            }
        }
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGDataProviderCreateWithData(
            info: Ptr,
            data: *const c_void,
            size: usize,
            release: Option<unsafe extern "C" fn(Ptr, *const c_void, usize)>,
        ) -> Ptr;
        pub fn CGDataProviderRelease(p: Ptr);
        pub fn CGPDFDocumentCreateWithProvider(p: Ptr) -> Ptr;
        pub fn CGPDFDocumentRelease(p: Ptr);
        pub fn CGPDFDocumentIsUnlocked(p: Ptr) -> bool;
        pub fn CGPDFDocumentGetNumberOfPages(p: Ptr) -> usize;
        pub fn CGPDFDocumentGetPage(p: Ptr, page: usize) -> Ptr;
        pub fn CGPDFPageGetBoxRect(p: Ptr, kind: i32) -> Rect;
        pub fn CGPDFPageGetRotationAngle(p: Ptr) -> i32;
        pub fn CGPDFPageGetDrawingTransform(
            p: Ptr,
            kind: i32,
            target: Rect,
            rotation: i32,
            aspect: bool,
        ) -> Transform;
        pub fn CGColorSpaceCreateDeviceRGB() -> Ptr;
        pub fn CGColorSpaceRelease(p: Ptr);
        pub fn CGBitmapContextCreate(
            data: Ptr,
            w: usize,
            h: usize,
            bits: usize,
            row: usize,
            space: Ptr,
            info: u32,
        ) -> Ptr;
        pub fn CGContextRelease(p: Ptr);
        pub fn CGContextScaleCTM(p: Ptr, x: f64, y: f64);
        pub fn CGContextConcatCTM(p: Ptr, t: Transform);
        pub fn CGContextDrawPDFPage(p: Ptr, page: Ptr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", windows))]
    fn pdf(objects: &[String]) -> PdfBytes {
        let mut pdf = "%PDF-1.4\n".to_string();
        let mut offsets = vec![0];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", i + 1, object));
        }
        let xref = pdf.len();
        let size = objects.len() + 1;
        pdf.push_str(&format!("xref\n0 {size}\n0000000000 65535 f \n"));
        for offset in offsets.iter().skip(1) {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        ));
        Arc::new(pdf.into_bytes())
    }

    #[cfg(any(target_os = "macos", windows))]
    fn stream(content: &str) -> String {
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
    }

    #[cfg(any(target_os = "macos", windows))]
    fn text_pdf(content: &str, boxes: &str, rotation: i32, cmap: Option<&str>) -> PdfBytes {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            format!(
                "<< /Type /Page /Parent 2 0 R {boxes} /Rotate {rotation} /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            ),
            stream(content),
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding {} >>",
                if cmap.is_some() {
                    "/ToUnicode 6 0 R"
                } else {
                    ""
                }
            ),
        ];
        if let Some(cmap) = cmap {
            objects.push(stream(cmap));
        }
        pdf(&objects)
    }
    #[test]
    fn invalid_transport_is_rejected() {
        assert!(decode("!!!").is_err());
        assert!(decode(&STANDARD.encode("not pdf")).is_err());
    }
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn packaged_renderer_and_encrypted_documents() {
        check_renderer().unwrap();
        let restricted = render(
            Arc::new(include_bytes!("../tests/fixtures/pdf/copy-restricted.pdf").to_vec()),
            1,
        )
        .unwrap();
        assert!(restricted.text.text.is_empty());
        assert!(restricted.text.glyphs.is_empty());
        assert!(
            restricted.data["textError"]
                .as_str()
                .unwrap()
                .contains("does not permit copying")
        );
        assert!(!restricted.pixels.is_empty());
        let protected = render(
            Arc::new(include_bytes!("../tests/fixtures/pdf/password.pdf").to_vec()),
            1,
        );
        assert!(protected.err().unwrap().contains("password-protected"));
    }
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn renders_pages_with_correct_orientation_and_bounds() {
        let content = b"1 0 0 rg 0 50 100 50 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                content.len(),
                String::from_utf8_lossy(content)
            ),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_string(),
        ];
        let bytes = pdf(&objects);
        let first = render(bytes.clone(), 1).unwrap();
        assert_eq!(first.data["pages"], 2);
        assert_eq!(first.data["width"], 200);
        assert_eq!(first.data["naturalWidth"], 100.);
        assert_eq!(first.data["naturalHeight"], 100.);
        assert!(first.text.text.is_empty());
        assert!(first.text.glyphs.is_empty());
        assert!(first.data.get("textError").is_none());
        let pixels = first.pixels.as_slice();
        assert_eq!(&pixels[0..4], &[0, 0, 255, 255]);
        assert_eq!(&pixels[pixels.len() - 4..], &[255, 255, 255, 255]);
        assert_eq!(render(bytes.clone(), 2).unwrap().data["width"], 400);
        assert!(render(bytes.clone(), 0).is_err());
        assert!(render(bytes, 3).is_err());
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn text_rectangles_track_crop_rotation_and_raster_ink() {
        for rotation in [0, 90, 180, 270] {
            // Nonzero MediaBox and CropBox origins exercise both translations.
            let page = render(
                text_pdf(
                    "BT /F1 20 Tf 40 110 Td (HI) Tj ET",
                    "/MediaBox [10 15 210 215] /CropBox [20 30 180 150]",
                    rotation,
                    None,
                ),
                1,
            )
            .unwrap();
            assert!(page.data.get("textError").is_none(), "{:?}", page.data);
            assert_eq!(page.text.text, "HI");
            assert_eq!(page.text.glyphs.len(), 2);
            let (w, h) = if rotation % 180 == 0 {
                (160., 120.)
            } else {
                (120., 160.)
            };
            assert_eq!(page.data["naturalWidth"], w);
            assert_eq!(page.data["naturalHeight"], h);
            let width = page.data["width"].as_u64().unwrap() as usize;
            let height = page.data["height"].as_u64().unwrap() as usize;
            let pixels = page.pixels.as_slice();
            let mut ink = [width, height, 0, 0];
            for y in 0..height {
                for x in 0..width {
                    if pixels[(y * width + x) * 4] < 128 {
                        ink[0] = ink[0].min(x);
                        ink[1] = ink[1].min(y);
                        ink[2] = ink[2].max(x + 1);
                        ink[3] = ink[3].max(y + 1);
                        assert!(
                            page.text.glyphs.iter().any(|glyph| {
                                let [gx, gy, gw, gh] = glyph.bounds;
                                x as f32 >= gx * width as f32 - 2.
                                    && x as f32 <= (gx + gw) * width as f32 + 2.
                                    && y as f32 >= gy * height as f32 - 2.
                                    && y as f32 <= (gy + gh) * height as f32 + 2.
                            }),
                            "rotation {rotation}: ink at ({x}, {y}) outside {:?}",
                            page.text.glyphs
                        );
                    }
                }
            }
            assert!(ink[0] < ink[2] && ink[1] < ink[3]);
            for glyph in &page.text.glyphs {
                assert!(
                    glyph
                        .bounds
                        .iter()
                        .all(|value| value.is_finite() && (0. ..=1.).contains(value))
                );
                assert!(page.text.text.is_char_boundary(glyph.range.start));
                assert!(page.text.text.is_char_boundary(glyph.range.end));
            }
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn unicode_ranges_survive_surrogates_ligatures_and_combining_marks() {
        let cmap = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /TestUnicode def\n/CMapType 2 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n4 beginbfchar\n<41> <4E2D>\n<42> <D83DDE00>\n<43> <00660069>\n<44> <00650301>\nendbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend";
        let page = render(
            text_pdf(
                "BT /F1 20 Tf 20 100 Td (ABCD) Tj ET",
                "/MediaBox [0 0 200 150]",
                0,
                Some(cmap),
            ),
            1,
        )
        .unwrap();
        assert!(page.data.get("textError").is_none(), "{:?}", page.data);
        assert_eq!(page.text.text, "中😀fie\u{301}");
        let spans: Vec<_> = page
            .text
            .glyphs
            .iter()
            .map(|glyph| &page.text.text[glyph.range.clone()])
            .collect();
        assert_eq!(spans, ["中", "😀", "f", "i", "e", "\u{301}"]);
        assert_eq!(page.text.glyphs[1].range, 3..7);
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn multiline_text_retains_linebreaks_and_positions() {
        let page = render(
            text_pdf(
                "BT /F1 20 Tf 20 100 Td (First) Tj 0 -30 Td (Second) Tj ET",
                "/MediaBox [0 0 200 150]",
                0,
                None,
            ),
            1,
        )
        .unwrap();
        assert_eq!(page.text.text, "First\nSecond");
        assert_eq!(page.text.glyphs.len(), 11);
        let first = &page.text.glyphs[0];
        let second = &page.text.glyphs[5];
        assert_eq!(&page.text.text[second.range.clone()], "S");
        assert!(second.bounds[1] > first.bounds[1] + 0.15);
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn text_limit_rejects_without_truncation() {
        assert!(native::check_text_limit(native::MAX_TEXT_UNITS).is_ok());
        assert!(
            native::check_text_limit(native::MAX_TEXT_UNITS + 1)
                .unwrap_err()
                .contains("Text selection is unavailable")
        );
        // Ordinary advances keep every character visible. Overlapping glyphs
        // or off-page text may legitimately be omitted by a PDF engine.
        let mut content = "BT /F1 1 Tf 0 990 Td 1.2 TL ".to_owned();
        for chunk in "A"
            .repeat(native::MAX_TEXT_UNITS + 1)
            .as_bytes()
            .chunks(1000)
        {
            content.push_str(&format!("({}) Tj T* ", std::str::from_utf8(chunk).unwrap()));
        }
        content.push_str("ET");
        let page = render(text_pdf(&content, "/MediaBox [0 0 1000 1000]", 0, None), 1).unwrap();
        assert!(page.text.text.is_empty());
        assert!(page.text.glyphs.is_empty());
        assert!(page.data["textError"].as_str().unwrap().contains("262,144"));
        assert!(!page.pixels.is_empty());
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn image_only_page_has_an_empty_selectable_layer() {
        let page = render(pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /Scan 5 0 R >> >> /Contents 4 0 R >>".into(),
            stream("q 100 0 0 100 0 0 cm /Scan Do Q"),
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length 3 >>\nstream\n00>\nendstream".into(),
        ]), 1).unwrap();
        assert!(page.text.text.is_empty());
        assert!(page.text.glyphs.is_empty());
        assert!(page.data.get("textError").is_none());
        assert_eq!(&page.pixels.as_slice()[0..4], &[0, 0, 0, 255]);
    }

    #[cfg(target_os = "macos")]
    #[allow(unexpected_cfgs)]
    #[test]
    fn copy_restricted_pdf_still_renders_without_exposing_text() {
        use cocoa::{base::nil, foundation::NSString};
        use objc::{
            class, msg_send,
            rc::{StrongPtr, autoreleasepool},
            runtime::Object,
            sel, sel_impl,
        };
        unsafe extern "C" {
            static PDFDocumentOwnerPasswordOption: *mut Object;
            static PDFDocumentAccessPermissionsOption: *mut Object;
        }
        let source = text_pdf(
            "BT /F1 20 Tf 20 100 Td (Restricted) Tj ET",
            "/MediaBox [0 0 200 150]",
            0,
            None,
        );
        let encrypted = autoreleasepool(|| unsafe {
            let data: *mut Object =
                msg_send![class!(NSData), dataWithBytes:source.as_ptr() length:source.len()];
            let document: *mut Object = msg_send![class!(PDFDocument), alloc];
            let document = StrongPtr::new(msg_send![document, initWithData:data]);
            assert!(!document.is_null());
            let owner = StrongPtr::new(NSString::alloc(nil).init_str("test-owner-password"));
            let permissions: *mut Object =
                msg_send![class!(NSNumber), numberWithUnsignedInteger:0usize];
            let keys = [
                PDFDocumentOwnerPasswordOption,
                PDFDocumentAccessPermissionsOption,
            ];
            let values = [*owner, permissions];
            let options: *mut Object = msg_send![class!(NSDictionary), dictionaryWithObjects:values.as_ptr()
                forKeys:keys.as_ptr() count:keys.len()];
            let encrypted: *mut Object =
                msg_send![*document, dataRepresentationWithOptions:options];
            assert!(!encrypted.is_null());
            let length: usize = msg_send![encrypted, length];
            let pointer: *const u8 = msg_send![encrypted, bytes];
            Arc::new(std::slice::from_raw_parts(pointer, length).to_vec())
        });
        let page = render(encrypted, 1).unwrap();
        assert!(page.text.text.is_empty());
        assert!(page.text.glyphs.is_empty());
        assert!(
            page.data["textError"]
                .as_str()
                .unwrap()
                .contains("does not permit copying")
        );
        assert!(!page.pixels.is_empty());
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn large_selectable_page_keeps_the_complete_text() {
        let content = format!("BT /F1 1 Tf 0 10 Td ({}) Tj ET", "A".repeat(10_000));
        let bytes = text_pdf(&content, "/MediaBox [0 0 10000 100]", 0, None);
        for attempt in 0..3 {
            let started = std::time::Instant::now();
            let page = render(bytes.clone(), 1).unwrap();
            eprintln!(
                "PDF render + extraction for 10,000 characters (attempt {attempt}): {:?}",
                started.elapsed()
            );
            assert!(page.data.get("textError").is_none(), "{:?}", page.data);
            assert_eq!(page.text.text.len(), 10_000);
            assert_eq!(page.text.glyphs.len(), 10_000);
        }
    }
}
