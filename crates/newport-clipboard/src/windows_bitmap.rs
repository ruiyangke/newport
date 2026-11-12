//! Bounded Windows DIB conversion. No external color profile is consulted.
const V5_HEADER: usize = 124;
fn rgba_size(width: u32, height: u32, limit: usize) -> Option<usize> {
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return None;
    }
    let size = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    (size <= limit).then_some(size)
}
fn rgba_dib(width: u32, height: u32, rgba: Vec<u8>, limit: usize) -> Option<Vec<u8>> {
    let size = rgba_size(width, height, limit)?;
    if rgba.len() != size || size.checked_add(V5_HEADER)? > limit {
        return None;
    }
    let mut dib = rgba;
    dib.try_reserve_exact(V5_HEADER).ok()?;
    dib.resize(size + V5_HEADER, 0);
    dib.copy_within(..size, V5_HEADER);
    dib[..V5_HEADER].fill(0);
    for (offset, value) in [
        (0, V5_HEADER as u32),
        (4, width),
        (8, (-(height as i32)) as u32),
        (16, 3),
        (20, u32::try_from(size).ok()?),
        (40, 0x00ff0000),
        (44, 0x0000ff00),
        (48, 0x000000ff),
        (52, 0xff000000),
        (56, 0x73524742),
        (108, 4),
    ] {
        dib[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    for pixel in dib[V5_HEADER..].as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(dib)
}
fn dib_file(bytes: &[u8], limit: usize) -> Option<Vec<u8>> {
    if bytes.len() > limit {
        return None;
    }
    let u16_at = |offset| {
        Some(u16::from_le_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    };
    let u32_at = |offset| {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let header = u32_at(0)? as usize;
    let (width, height, depth, palette, palette_width, masks) = match header {
        12 => {
            if u16_at(8)? != 1 {
                return None;
            }
            let depth = u16_at(10)?;
            (
                u32::from(u16_at(4)?),
                u32::from(u16_at(6)?),
                depth,
                if depth <= 8 { 1usize << depth } else { 0 },
                3,
                0,
            )
        }
        40 | 52 | 56 | 108 | 124 => {
            if u16_at(12)? != 1 {
                return None;
            }
            let width = i32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
            let height = i32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?);
            let depth = u16_at(14)?;
            let compression = u32_at(16)?;
            if width <= 0 || height == i32::MIN || !matches!(compression, 0..=3) {
                return None;
            }
            if height < 0 && !matches!(compression, 0 | 3) {
                return None;
            }
            // Linked and embedded profiles have separate offsets and are not part of the pixels.
            if header >= 108 && !matches!(u32_at(56)?, 0 | 0x73524742 | 0x57696e20) {
                return None;
            }
            if header == 124 && (u32_at(112)? != 0 || u32_at(116)? != 0) {
                return None;
            }
            let used = u32_at(32)? as usize;
            let palette = if depth <= 8 {
                if used == 0 { 1usize << depth } else { used }
            } else {
                used
            };
            if palette > 256 {
                return None;
            }
            (
                width as u32,
                height.unsigned_abs(),
                depth,
                palette,
                4,
                if header == 40 && compression == 3 {
                    12
                } else {
                    0
                },
            )
        }
        _ => return None,
    };
    if !matches!(depth, 1 | 4 | 8 | 16 | 24 | 32) {
        return None;
    }
    rgba_size(width, height, limit)?;
    let offset = header
        .checked_add(masks)?
        .checked_add(palette.checked_mul(palette_width)?)?;
    if offset >= bytes.len() {
        return None;
    }
    let length = bytes.len().checked_add(14)?;
    let mut file = Vec::with_capacity(length);
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&u32::try_from(length).ok()?.to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&u32::try_from(offset.checked_add(14)?).ok()?.to_le_bytes());
    file.extend_from_slice(bytes);
    Some(file)
}
#[cfg(windows)]
fn decode(
    bytes: &[u8],
    format: image::ImageFormat,
    input_limit: usize,
    limit: usize,
) -> Option<image::DynamicImage> {
    use image::ImageDecoder;
    if bytes.len() > input_limit {
        return None;
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(limit as u64);
    reader.limits(limits);
    let decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    rgba_size(width, height, limit)?;
    if decoder.total_bytes() > limit as u64 {
        return None;
    }
    image::DynamicImage::from_decoder(decoder).ok()
}
#[cfg(windows)]
pub(super) fn png_to_dib(bytes: &[u8], wire_limit: usize, native_limit: usize) -> Option<Vec<u8>> {
    let image = decode(
        bytes,
        image::ImageFormat::Png,
        wire_limit,
        native_limit.checked_sub(V5_HEADER)?,
    )?
    .into_rgba8();
    rgba_dib(
        image.width(),
        image.height(),
        image.into_raw(),
        native_limit,
    )
}
#[cfg(windows)]
pub(super) fn dib_to_png(bytes: &[u8], native_limit: usize, wire_limit: usize) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let file = dib_file(bytes, native_limit)?;
    let image = decode(
        &file,
        image::ImageFormat::Bmp,
        native_limit.checked_add(14)?,
        native_limit,
    )?
    .into_rgba8();
    struct Output {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("Clipboard image exceeds byte limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Output {
        bytes: Vec::new(),
        limit: wire_limit,
    };
    image::codecs::png::PngEncoder::new(&mut output)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(output.bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dib_preserves_top_down_rows_and_straight_alpha() {
        let dib = rgba_dib(1, 2, vec![255, 0, 0, 128, 0, 255, 0, 0], 1024).unwrap();
        assert_eq!(&dib[8..12], &(-2i32).to_le_bytes());
        assert_eq!(&dib[52..56], &0xff000000u32.to_le_bytes());
        assert_eq!(&dib[V5_HEADER..], &[0, 0, 255, 128, 0, 255, 0, 0]);
        let file = dib_file(&dib, 1024).unwrap();
        assert_eq!(&file[10..14], &138u32.to_le_bytes());
        assert_eq!(&file[138..], &dib[124..]);
    }
    #[test]
    fn dib_rejects_oversized_dimensions_profiles_and_truncation() {
        assert!(rgba_dib(u32::MAX, 2, vec![], 1024).is_none());
        assert!(rgba_dib(1, 1, vec![0; 4], 127).is_none());
        let mut dib = rgba_dib(1, 1, vec![0; 4], 1024).unwrap();
        dib[112..116].copy_from_slice(&124u32.to_le_bytes());
        assert!(dib_file(&dib, 1024).is_none());
        assert!(dib_file(&[0; 3], 1024).is_none());
        assert!(dib_file(&rgba_dib(1, 1, vec![0; 4], 1024).unwrap()[..124], 1024).is_none());
    }
    #[cfg(windows)]
    #[test]
    fn dib_png_round_trip_preserves_alpha_and_pixel_order() {
        let pixels = [255, 0, 20, 128, 0, 255, 30, 255];
        let dib = rgba_dib(1, 2, pixels.to_vec(), 4096).unwrap();
        let png = dib_to_png(&dib, 4096, 4096).unwrap();
        assert_eq!(
            decode(&png, image::ImageFormat::Png, 4096, 4096)
                .unwrap()
                .into_rgba8()
                .as_raw(),
            &pixels
        );
        assert_eq!(png_to_dib(&png, 4096, 4096).unwrap(), dib);
    }
    #[cfg(windows)]
    #[test]
    fn a_compressed_large_image_has_separate_native_and_wire_limits() {
        use image::ImageEncoder;
        let width = 3840;
        let height = 2160;
        let pixels = vec![255u8; width as usize * height as usize * 4];
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        drop(pixels);
        let wire_limit = 8 * 1024 * 1024;
        let native_limit = super::super::windows_limits::MAX_IMAGE_BYTES;
        assert!(png.len() < wire_limit);
        let dib = png_to_dib(&png, wire_limit, native_limit).unwrap();
        assert!(dib.len() > wire_limit);
        assert!(png_to_dib(&png, png.len() - 1, native_limit).is_none());
        assert!(png_to_dib(&png, wire_limit, dib.len() - 1).is_none());
        let roundtrip = dib_to_png(&dib, native_limit, wire_limit).unwrap();
        assert!(roundtrip.len() <= wire_limit);
        assert!(dib_to_png(&dib, dib.len() - 1, wire_limit).is_none());
        assert!(dib_to_png(&dib, native_limit, 32).is_none());
    }
}
