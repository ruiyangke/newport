//! Bounded raster decoding on the existing request worker, never in render().
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui_kit::RenderImage;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader};
use serde_json::Value;
use std::{io::Cursor, sync::Arc};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODED: u64 = 64 * 1024 * 1024;
const MAX_FRAMES: usize = 200;
#[path = "file_image_gif.rs"]
mod gif_metadata;

pub fn prepare(preview: &mut Value) -> Result<Option<Arc<RenderImage>>, String> {
    if preview["kind"] != "image" {
        return Ok(None);
    }
    let format = match preview["mime"].as_str() {
        Some("image/png") => ImageFormat::Png,
        Some("image/jpeg") => ImageFormat::Jpeg,
        Some("image/gif") => ImageFormat::Gif,
        Some("image/webp") => ImageFormat::WebP,
        _ => return Err("This image format cannot be previewed.".into()),
    };
    let content = preview["content"]
        .as_str()
        .ok_or("Image preview data is missing.")?;
    if content.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err("Image preview exceeds 16 MiB.".into());
    }
    let bytes = STANDARD
        .decode(content)
        .map_err(|_| "Image preview data is invalid.".to_owned())?;
    if bytes.len() > MAX_BYTES {
        return Err("Image preview exceeds 16 MiB.".into());
    }
    let (width, height) = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .map_err(|_| "The image header cannot be decoded.".to_owned())?;
    let frame_bytes = u64::from(width) * u64::from(height) * 4;
    if width == 0 || height == 0 || frame_bytes > MAX_DECODED {
        return Err("Image dimensions exceed the 64 MiB preview limit.".into());
    }
    let mut plays = None;
    let mut frames = if format == ImageFormat::Gif {
        plays = gif_metadata::plays(&bytes)?;
        let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(&bytes))
            .map_err(|_| "The GIF cannot be decoded.".to_owned())?;
        // Frame rectangles can exceed the logical screen dimensions. Bound the
        // decoder's working allocation as well as the retained output below.
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODED);
        decoder
            .set_limits(limits)
            .map_err(|_| "The GIF exceeds preview decoding limits.".to_owned())?;
        collect_frames(decoder.into_frames(), frame_bytes)?
    } else if format == ImageFormat::Png {
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODED);
        let decoder = image::codecs::png::PngDecoder::with_limits(Cursor::new(&bytes), limits)
            .map_err(|_| "The PNG cannot be decoded within preview limits.".to_owned())?;
        if decoder
            .is_apng()
            .map_err(|_| "The PNG animation header is invalid.".to_owned())?
        {
            let animation = decoder
                .apng()
                .map_err(|_| "The PNG animation cannot be decoded.".to_owned())?;
            plays = finite_plays(animation.loop_count());
            collect_frames(animation.into_frames(), frame_bytes)?
        } else {
            vec![decode_still(decoder)?]
        }
    } else if format == ImageFormat::WebP {
        // Validate canvas dimensions above before allocating animation frames.
        // The codec composites subframes; retained full canvases share GIF's cap.
        let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(&bytes))
            .map_err(|_| "The WebP cannot be decoded.".to_owned())?;
        if decoder.has_animation() {
            plays = finite_plays(decoder.loop_count());
            let orientation = decoder
                .orientation()
                .unwrap_or(image::metadata::Orientation::NoTransforms);
            let mut frames = collect_frames(decoder.into_frames(), frame_bytes)?;
            if orientation != image::metadata::Orientation::NoTransforms {
                for frame in &mut frames {
                    let delay = frame.delay();
                    let mut decoded =
                        image::DynamicImage::ImageRgba8(std::mem::take(frame.buffer_mut()));
                    decoded.apply_orientation(orientation);
                    *frame = image::Frame::from_parts(decoded.into_rgba8(), 0, 0, delay);
                }
            }
            frames
        } else {
            vec![decode_still(decoder)?]
        }
    } else {
        let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODED);
        reader.limits(limits);
        let decoder = reader
            .into_decoder()
            .map_err(|_| "The image cannot be decoded within preview limits.".to_owned())?;
        vec![decode_still(decoder)?]
    };
    if frames.is_empty() {
        return Err("The image contains no frames.".into());
    }
    for frame in &mut frames {
        for pixel in frame.buffer_mut().as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
    }
    // No repeated base64 copies in UI snapshots/renders after the worker decoded it.
    preview["content"] = Value::Null;
    // Rotations/transpositions can exchange width and height. These dimensions
    // also drive Actual size and zoom, so expose the displayed raster's size.
    preview["width"] = frames[0].buffer().width().into();
    preview["height"] = frames[0].buffer().height().into();
    preview["animationPlays"] = plays.into();
    Ok(Some(Arc::new(RenderImage::new(frames))))
}

fn finite_plays(count: image::metadata::LoopCount) -> Option<u32> {
    match count {
        image::metadata::LoopCount::Infinite => None,
        image::metadata::LoopCount::Finite(count) => Some(count.get()),
    }
}

fn decode_still(mut decoder: impl ImageDecoder) -> Result<image::Frame, String> {
    // Browsers honor camera orientation. Tolerate malformed metadata while
    // retaining the decoder's pixel and allocation limits.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| "The image cannot be decoded within preview limits.".to_owned())?;
    decoded.apply_orientation(orientation);
    Ok(image::Frame::new(decoded.into_rgba8()))
}

fn collect_frames(
    frames: image::Frames<'_>,
    frame_bytes: u64,
) -> Result<Vec<image::Frame>, String> {
    let mut result = Vec::new();
    for frame in frames {
        if result.len() >= MAX_FRAMES || (result.len() as u64 + 1) * frame_bytes > MAX_DECODED {
            return Err("Animated image exceeds the preview memory or frame limit.".into());
        }
        result.push(frame.map_err(|_| "An image frame cannot be decoded.".to_owned())?);
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../tests/fixtures/image_preview.rs"]
mod fixture;

#[cfg(test)]
mod tests {
    use super::{STANDARD, fixture, prepare};
    use base64::Engine as _;
    use serde_json::json;
    #[test]
    fn gif_absent_infinite_and_finite_repeat_counts_match_browser_semantics() {
        for (repeats, expected) in [
            (None, Some(1)),
            (Some(0), None),
            (Some(1), Some(2)),
            (Some(u16::MAX), Some(65536)),
        ] {
            let mut preview = json!({"kind":"image", "mime":"image/gif",
                "content":STANDARD.encode(fixture::gif(repeats))});
            assert_eq!(prepare(&mut preview).unwrap().unwrap().frame_count(), 2);
            assert_eq!(preview["animationPlays"].as_u64(), expected);
        }
    }

    #[test]
    fn gif_repeat_metadata_handles_late_extensions_and_ignores_comment_lookalikes() {
        let mut bytes = fixture::gif(Some(3));
        let marker = b"\x21\xff\x0bNETSCAPE2.0";
        let at = bytes
            .windows(marker.len())
            .position(|part| part == marker)
            .unwrap();
        let extension: Vec<_> = bytes.drain(at..at + 19).collect();
        assert_eq!(super::gif_metadata::plays(&bytes).unwrap(), Some(1));
        let mut comment = vec![0x21, 0xfe, extension.len() as u8];
        comment.extend(&extension);
        comment.push(0);
        bytes.splice(bytes.len() - 1..bytes.len() - 1, comment);
        assert_eq!(super::gif_metadata::plays(&bytes).unwrap(), Some(1));
        bytes.splice(bytes.len() - 1..bytes.len() - 1, extension);
        let mut preview =
            json!({"kind":"image", "mime":"image/gif", "content":STANDARD.encode(&bytes)});
        prepare(&mut preview).unwrap();
        assert_eq!(preview["animationPlays"], 4);
        for end in 0..bytes.len() {
            assert!(
                super::gif_metadata::plays(&bytes[..end]).is_err(),
                "prefix {end}"
            );
        }
    }
    #[test]
    fn animated_png_and_webp_preserve_finite_play_counts() {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::WebP, "image/webp"),
        ] {
            for plays in [0, 1, 2, u16::MAX] {
                let mut preview = json!({"kind":"image", "mime":mime,
                    "content": STANDARD.encode(fixture::animation_with_plays(format, 2, plays))});
                assert_eq!(prepare(&mut preview).unwrap().unwrap().frame_count(), 2);
                assert_eq!(
                    preview["animationPlays"].as_u64(),
                    (plays != 0).then_some(u64::from(plays))
                );
            }
        }
    }
    #[test]
    fn png_preserves_alpha_and_converts_to_renderer_channel_order() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 78]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let mut preview = json!({"kind":"image","mime":"image/png","content":STANDARD.encode(bytes.into_inner())});
        let image = prepare(&mut preview).unwrap().unwrap();
        assert_eq!(image.as_bytes(0).unwrap(), &[56, 34, 12, 78]);
        assert_eq!(preview["width"], 1);
        assert!(preview["content"].is_null());
    }
    #[test]
    fn malformed_images_fail_and_text_is_untouched() {
        let mut text = json!({"kind":"text","content":"hello"});
        assert!(prepare(&mut text).unwrap().is_none());
        assert_eq!(text["content"], "hello");
        for (mime, content) in [
            ("image/png", "!!!"),
            ("image/jpeg", "YWJj"),
            ("image/svg+xml", "YWJj"),
        ] {
            assert!(prepare(&mut json!({"kind":"image","mime":mime,"content":content})).is_err());
        }
    }

    #[test]
    fn camera_orientation_transforms_pixels_and_reported_dimensions() {
        for (orientation, corner) in [
            (1, [240, 20, 20]),
            (2, [20, 220, 20]),
            (3, [240, 220, 20]),
            (4, [20, 20, 240]),
            (5, [240, 20, 20]),
            (6, [20, 20, 240]),
            (7, [240, 220, 20]),
            (8, [20, 220, 20]),
        ] {
            let mut preview = json!({"kind":"image", "mime":"image/jpeg",
                "content":STANDARD.encode(fixture::oriented_jpeg(orientation))});
            let image = prepare(&mut preview).unwrap().unwrap();
            let (width, height) = if orientation >= 5 { (40, 80) } else { (80, 40) };
            assert_eq!(preview["width"], width, "orientation {orientation}");
            assert_eq!(preview["height"], height, "orientation {orientation}");
            let pixels = image.as_bytes(0).unwrap();
            let at = (5 * width as usize + 5) * 4;
            for (actual, expected) in [pixels[at + 2], pixels[at + 1], pixels[at]]
                .into_iter()
                .zip(corner)
            {
                assert!(
                    (i32::from(actual) - expected).abs() < 12,
                    "orientation {orientation}"
                );
            }
        }
    }
    #[test]
    fn animated_png_and_webp_preserve_frames_and_delays() {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::WebP, "image/webp"),
        ] {
            let mut preview = json!({"kind":"image", "mime":mime,
                "content":STANDARD.encode(fixture::animation(format, 2))});
            let image = prepare(&mut preview).unwrap().unwrap();
            assert_eq!(image.frame_count(), 2, "{mime}");
            assert_eq!(&image.as_bytes(0).unwrap()[..4], &[20, 20, 240, 255]);
            assert_eq!(&image.as_bytes(1).unwrap()[..4], &[20, 220, 20, 255]);
            assert_eq!(std::time::Duration::from(image.delay(0)).as_millis(), 400);
            assert_eq!(std::time::Duration::from(image.delay(1)).as_millis(), 600);
            assert!(preview["content"].is_null());
        }
    }
    #[test]
    fn animated_png_and_webp_enforce_frame_limit_and_reject_truncation() {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::WebP, "image/webp"),
        ] {
            let preview = |bytes| {
                json!({"kind":"image", "mime":mime,
                "content":STANDARD.encode(bytes)})
            };
            let image = prepare(&mut preview(fixture::animation(format, super::MAX_FRAMES)))
                .unwrap()
                .unwrap();
            assert_eq!(image.frame_count(), super::MAX_FRAMES);
            let error = prepare(&mut preview(fixture::animation(
                format,
                super::MAX_FRAMES + 1,
            )))
            .err()
            .unwrap();
            assert!(error.contains("frame limit"), "{mime}: {error}");
            let bytes = fixture::animation(format, 2);
            assert!(
                prepare(&mut preview(bytes[..bytes.len() / 2].to_vec())).is_err(),
                "{mime}"
            );
        }
    }
    #[test]
    fn animation_retained_memory_is_bounded_independently_of_frame_count() {
        let frames = || {
            image::Frames::new(Box::new(
                (0..3).map(|_| Ok(image::Frame::new(image::RgbaImage::new(1, 1)))),
            ))
        };
        assert_eq!(
            super::collect_frames(frames(), super::MAX_DECODED / 3)
                .unwrap()
                .len(),
            3
        );
        assert!(super::collect_frames(frames(), super::MAX_DECODED / 2).is_err());
    }
    #[test]
    fn gif_frames_keep_order_and_animation_is_bounded() {
        fn gif(count: usize) -> Vec<u8> {
            let mut bytes = Vec::new();
            {
                let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
                for index in 0..count {
                    encoder
                        .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                            1,
                            1,
                            image::Rgba([index as u8, 0, 0, 255]),
                        )))
                        .unwrap();
                }
            }
            bytes
        }
        let preview =
            |bytes| json!({"kind":"image","mime":"image/gif","content":STANDARD.encode(bytes)});
        let image = prepare(&mut preview(gif(2))).unwrap().unwrap();
        assert_eq!(image.as_bytes(0).unwrap(), &[0, 0, 0, 255]);
        assert_eq!(image.as_bytes(1).unwrap(), &[0, 0, 1, 255]);
        assert!(prepare(&mut preview(gif(super::MAX_FRAMES + 1))).is_err());
        let mut oversized = gif(1);
        oversized[6..10].fill(255);
        let error = prepare(&mut preview(oversized)).err().unwrap();
        assert!(error.contains("dimensions"));
        // A small logical screen must not permit an enormous frame allocation.
        let mut oversized_frame = gif(1);
        let descriptor = oversized_frame
            .iter()
            .position(|byte| *byte == 0x2c)
            .unwrap();
        assert_eq!(
            &oversized_frame[descriptor + 5..descriptor + 9],
            &[1, 0, 1, 0]
        );
        oversized_frame[descriptor + 5..descriptor + 9].fill(255);
        assert!(prepare(&mut preview(oversized_frame)).is_err());
    }
}
