//! Small, deterministic image files for decoder and live preview checks.
pub fn quadrants() -> image::RgbImage {
    image::RgbImage::from_fn(80, 40, |x, y| {
        image::Rgb(match (x < 40, y < 20) {
            (true, true) => [240, 20, 20],
            (false, true) => [20, 220, 20],
            (true, false) => [20, 20, 240],
            (false, false) => [240, 220, 20],
        })
    })
}

pub fn oriented_jpeg(orientation: u16) -> Vec<u8> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode_image(&quadrants())
        .unwrap();
    // One little-endian TIFF IFD entry: Orientation (0x112), SHORT, count=1.
    let mut exif = b"Exif\0\0II".to_vec();
    exif.extend(42u16.to_le_bytes());
    exif.extend(8u32.to_le_bytes());
    exif.extend(1u16.to_le_bytes());
    exif.extend(0x112u16.to_le_bytes());
    exif.extend(3u16.to_le_bytes());
    exif.extend(1u32.to_le_bytes());
    exif.extend(orientation.to_le_bytes());
    exif.extend(0u16.to_le_bytes());
    exif.extend(0u32.to_le_bytes());
    let mut result = vec![0xff, 0xd8, 0xff, 0xe1];
    result.extend(((exif.len() + 2) as u16).to_be_bytes());
    result.extend(exif);
    result.extend(&jpeg[2..]);
    result
}

pub fn gif(repeats: Option<u16>) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        if let Some(repeats) = repeats {
            encoder
                .set_repeat(if repeats == 0 {
                    image::codecs::gif::Repeat::Infinite
                } else {
                    image::codecs::gif::Repeat::Finite(repeats)
                })
                .unwrap();
        }
        for index in 0..2 {
            let mut pixels = quadrants();
            if index == 1 {
                image::imageops::flip_horizontal_in_place(&mut pixels);
            }
            encoder
                .encode_frame(image::Frame::from_parts(
                    image::DynamicImage::ImageRgb8(pixels).into_rgba8(),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(if index == 0 { 400 } else { 600 }, 1),
                ))
                .unwrap();
        }
    }
    bytes
}

/// Full-canvas, lossless alternating frames with distinct delays. Container
/// layouts follow W3C PNG 3 and Google's WebP RIFF specifications; the existing
/// image encoders supply compressed pixels. No additional test dependencies.
pub fn animation(format: image::ImageFormat, count: usize) -> Vec<u8> {
    animation_with_plays(format, count, 0)
}

pub fn animation_with_plays(format: image::ImageFormat, count: usize, plays: u16) -> Vec<u8> {
    use image::ImageEncoder;
    let mut result = if format == image::ImageFormat::Png {
        b"\x89PNG\r\n\x1a\n".to_vec()
    } else {
        b"RIFF\0\0\0\0WEBP".to_vec()
    };
    let mut sequence = 0u32;
    for index in 0..count {
        let mut pixels = quadrants();
        if index % 2 == 1 {
            image::imageops::flip_horizontal_in_place(&mut pixels);
        }
        let mut encoded = Vec::new();
        let delay = if index % 2 == 0 { 400u32 } else { 600 };
        if format == image::ImageFormat::Png {
            image::codecs::png::PngEncoder::new(&mut encoded)
                .write_image(pixels.as_raw(), 80, 40, image::ExtendedColorType::Rgb8)
                .unwrap();
            let mut chunks = Vec::new();
            let mut offset = 8;
            while offset < encoded.len() {
                let len =
                    u32::from_be_bytes(encoded[offset..offset + 4].try_into().unwrap()) as usize;
                chunks.push((
                    &encoded[offset + 4..offset + 8],
                    &encoded[offset + 8..offset + 8 + len],
                ));
                offset += len + 12;
            }
            if index == 0 {
                png_chunk(&mut result, b"IHDR", chunks[0].1);
                let mut control = (count as u32).to_be_bytes().to_vec();
                control.extend(u32::from(plays).to_be_bytes());
                png_chunk(&mut result, b"acTL", &control);
            }
            let mut control = sequence.to_be_bytes().to_vec();
            sequence += 1;
            for value in [80u32, 40, 0, 0] {
                control.extend(value.to_be_bytes());
            }
            control.extend((delay as u16).to_be_bytes());
            control.extend(1000u16.to_be_bytes());
            control.extend([0, 0]); // no disposal, source blend
            png_chunk(&mut result, b"fcTL", &control);
            for (_, data) in chunks.into_iter().filter(|(kind, _)| *kind == b"IDAT") {
                if index == 0 {
                    png_chunk(&mut result, b"IDAT", data);
                } else {
                    let mut frame = sequence.to_be_bytes().to_vec();
                    sequence += 1;
                    frame.extend(data);
                    png_chunk(&mut result, b"fdAT", &frame);
                }
            }
        } else {
            assert_eq!(format, image::ImageFormat::WebP);
            image::codecs::webp::WebPEncoder::new_lossless(&mut encoded)
                .write_image(pixels.as_raw(), 80, 40, image::ExtendedColorType::Rgb8)
                .unwrap();
            if index == 0 {
                riff_chunk(&mut result, b"VP8X", &[2, 0, 0, 0, 79, 0, 0, 39, 0, 0]);
                let mut control = vec![0; 4];
                control.extend(plays.to_le_bytes());
                riff_chunk(&mut result, b"ANIM", &control);
            }
            let mut frame = vec![0, 0, 0, 0, 0, 0, 79, 0, 0, 39, 0, 0];
            frame.extend(&delay.to_le_bytes()[..3]);
            frame.push(2); // replace, no disposal
            frame.extend(&encoded[12..]);
            riff_chunk(&mut result, b"ANMF", &frame);
        }
    }
    if format == image::ImageFormat::Png {
        png_chunk(&mut result, b"IEND", &[]);
    } else {
        let length = (result.len() - 8) as u32;
        result[4..8].copy_from_slice(&length.to_le_bytes());
    }
    result
}

fn png_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend((data.len() as u32).to_be_bytes());
    output.extend(kind);
    output.extend(data);
    let mut crc = !0u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    output.extend((!crc).to_be_bytes());
}

fn riff_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend(kind);
    output.extend((data.len() as u32).to_le_bytes());
    output.extend(data);
    if data.len() % 2 == 1 {
        output.push(0);
    }
}
