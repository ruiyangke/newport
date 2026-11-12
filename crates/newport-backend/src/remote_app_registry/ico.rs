//! An app icon as a Windows `.ico` file, the format a taskbar pin's icon
//! resource names: one PNG image per size, which Windows reads since Vista.
use image::{GenericImageView, imageops};
use std::io::Cursor;

/// The sizes Windows picks from, from small icons at 100% to large ones at
/// high scale.
pub const SIZES: [u32; 7] = [16, 20, 24, 32, 48, 64, 256];
/// Larger images are refused before they are decoded.
pub const MAX_SIDE: u32 = 1024;
const HEADER: usize = 6;
const DIRECTORY_ENTRY: usize = 16;

/// The image, any format the `image` crate reads (PNG in practice), as an
/// `.ico` file holding each of [`SIZES`]: fitted into a transparent square,
/// centred.
pub fn encode(image: &[u8]) -> Result<Vec<u8>, String> {
    let reader = image::ImageReader::new(Cursor::new(image))
        .with_guessed_format()
        .map_err(|error| format!("Cannot read the app icon: {error}"))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("Cannot read the app icon: {error}"))?;
    if width.max(height) > MAX_SIDE {
        return Err(format!("The app icon is too large: {width}x{height}"));
    }
    let decoded = image::load_from_memory(image)
        .map_err(|error| format!("Cannot read the app icon: {error}"))?;
    let mut images = Vec::with_capacity(SIZES.len());
    for side in SIZES {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(square(&decoded, side))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|error| format!("Cannot encode the app icon: {error}"))?;
        images.push((side, png));
    }
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    // An icon, not a cursor.
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = HEADER + DIRECTORY_ENTRY * images.len();
    for (side, png) in &images {
        // 256 is written as 0.
        let dimension = if *side >= 256 { 0 } else { *side as u8 };
        ico.extend_from_slice(&[dimension, dimension, 0, 0]);
        // Colour planes and bits per pixel.
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in images {
        ico.extend_from_slice(&png);
    }
    Ok(ico)
}

/// The largest image of an `.ico` file [`encode`] wrote, the PNG of its
/// last size; `None` for any other file.
pub fn largest(ico: &[u8]) -> Option<&[u8]> {
    let count = usize::from(u16::from_le_bytes(ico.get(4..6)?.try_into().ok()?));
    let entry = ico.get(HEADER + DIRECTORY_ENTRY * count.checked_sub(1)?..)?;
    let field = |at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(entry.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let (length, offset) = (field(8)?, field(12)?);
    ico.get(offset..offset.checked_add(length)?)
}

fn square(image: &image::DynamicImage, side: u32) -> image::RgbaImage {
    let (width, height) = image.dimensions();
    let scale = f64::from(side) / f64::from(width.max(height).max(1));
    let fitted_width = ((f64::from(width) * scale).round() as u32).clamp(1, side);
    let fitted_height = ((f64::from(height) * scale).round() as u32).clamp(1, side);
    let fitted = imageops::resize(
        &image.to_rgba8(),
        fitted_width,
        fitted_height,
        imageops::FilterType::Lanczos3,
    );
    let mut canvas = image::RgbaImage::new(side, side);
    imageops::overlay(
        &mut canvas,
        &fitted,
        i64::from((side - fitted_width) / 2),
        i64::from((side - fitted_height) / 2),
    );
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn an_icon_holds_every_size_as_png() {
        let ico = encode(&png(96, 48)).unwrap();
        assert_eq!(u16_at(&ico, 0), 0);
        assert_eq!(u16_at(&ico, 2), 1);
        assert_eq!(u16_at(&ico, 4) as usize, SIZES.len());
        let mut end = HEADER + DIRECTORY_ENTRY * SIZES.len();
        for (index, side) in SIZES.into_iter().enumerate() {
            let entry = &ico[HEADER + DIRECTORY_ENTRY * index..][..DIRECTORY_ENTRY];
            let written = if side == 256 { 0 } else { side as u8 };
            assert_eq!(&entry[..4], &[written, written, 0, 0]);
            assert_eq!(u16_at(entry, 4), 1);
            assert_eq!(u16_at(entry, 6), 32);
            let (length, offset) = (u32_at(entry, 8) as usize, u32_at(entry, 12) as usize);
            assert_eq!(offset, end, "images follow each other");
            end += length;
            let image = image::load_from_memory_with_format(
                &ico[offset..offset + length],
                image::ImageFormat::Png,
            )
            .unwrap()
            .to_rgba8();
            assert_eq!(image.dimensions(), (side, side));
            // A wide image is centred, with transparent bands above and below.
            assert_eq!(image.get_pixel(side / 2, 0)[3], 0);
            assert_eq!(image.get_pixel(side / 2, side / 2)[3], 255);
        }
        assert_eq!(end, ico.len());
        let largest = largest(&ico).unwrap();
        let image = image::load_from_memory_with_format(largest, image::ImageFormat::Png).unwrap();
        assert_eq!(image.dimensions(), (256, 256));
        assert_eq!(super::largest(b"not an icon"), None);
        assert_eq!(super::largest(&ico[..ico.len() - 1]), None);
    }

    #[test]
    fn large_or_unreadable_images_are_refused() {
        assert!(encode(&png(MAX_SIDE, 8)).is_ok());
        let error = encode(&png(MAX_SIDE + 1, 8)).unwrap_err();
        assert!(error.contains("too large"), "{error}");
        assert!(encode(b"not an image").is_err());
        assert!(encode(&[]).is_err());
    }
}
