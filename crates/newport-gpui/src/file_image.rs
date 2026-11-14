//! Bounded raster decoding on the existing request worker, never in render().
use gpui_kit::RenderImage;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader};
use newport_protocol::desktop::files;
use std::{io::Cursor, sync::Arc};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODED: u64 = 64 * 1024 * 1024;
const MAX_FRAMES: usize = 200;
#[path = "file_image_gif.rs"]
mod gif_metadata;

/// A file preview ready to draw. An image keeps its decoded raster instead of
/// its encoded bytes, so renders never copy them.
#[derive(Clone)]
pub struct Prepared {
    pub preview: files::Preview,
    pub image: Option<Image>,
}

impl Prepared {
    /// A text preview's content.
    pub fn text(&self) -> Option<&str> {
        match &self.preview.content {
            files::Content::Text(text) => Some(text),
            files::Content::Binary(_) => None,
        }
    }
}

/// A decoded image preview.
#[derive(Clone)]
pub struct Image {
    pub raster: Arc<RenderImage>,
    /// The displayed raster's size. Rotations can exchange width and height,
    /// and this size drives Actual size and zoom.
    pub width: u32,
    pub height: u32,
    /// How many times an animation plays; `None` loops forever.
    pub plays: Option<u32>,
}

pub fn prepare(mut preview: files::Preview) -> Result<Prepared, String> {
    if preview.kind != files::PreviewKind::Image {
        return Ok(Prepared {
            preview,
            image: None,
        });
    }
    let format = match preview.mime.as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => return Err("This image format cannot be previewed.".into()),
    };
    let files::Content::Binary(bytes) = &preview.content else {
        return Err("Image preview data is missing.".into());
    };
    if bytes.len() > MAX_BYTES {
        return Err("Image preview exceeds 16 MiB.".into());
    }
    let bytes = (**bytes).clone();
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
    let (width, height) = (frames[0].buffer().width(), frames[0].buffer().height());
    preview.content = files::Content::Binary(Box::default());
    Ok(Prepared {
        preview,
        image: Some(Image {
            raster: Arc::new(RenderImage::new(frames)),
            width,
            height,
            plays,
        }),
    })
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
    use super::{files, fixture};
    use gpui_kit::RenderImage;
    use std::sync::Arc;
    fn preview(mime: &str, bytes: Vec<u8>) -> files::Preview {
        files::Preview {
            kind: files::PreviewKind::Image,
            mime: mime.into(),
            content: files::Content::Binary(Box::new(bytes.into())),
            truncated: false,
        }
    }
    /// The raster and play count an image preview decodes to.
    fn prepare(preview: files::Preview) -> Result<(Arc<RenderImage>, super::Image), String> {
        let image = super::prepare(preview)?.image.unwrap();
        Ok((image.raster.clone(), image))
    }
    #[test]
    fn gif_absent_infinite_and_finite_repeat_counts_match_browser_semantics() {
        for (repeats, expected) in [
            (None, Some(1)),
            (Some(0), None),
            (Some(1), Some(2)),
            (Some(u16::MAX), Some(65536)),
        ] {
            let (raster, image) = prepare(preview("image/gif", fixture::gif(repeats))).unwrap();
            assert_eq!(raster.frame_count(), 2);
            assert_eq!(image.plays.map(u64::from), expected);
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
        let (_, image) = prepare(preview("image/gif", bytes.clone())).unwrap();
        assert_eq!(image.plays, Some(4));
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
                let (raster, image) = prepare(preview(
                    mime,
                    fixture::animation_with_plays(format, 2, plays),
                ))
                .unwrap();
                assert_eq!(raster.frame_count(), 2);
                assert_eq!(image.plays, (plays != 0).then_some(u32::from(plays)));
            }
        }
    }
    #[test]
    fn png_preserves_alpha_and_converts_to_renderer_channel_order() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 78]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let prepared = super::prepare(preview("image/png", bytes.into_inner())).unwrap();
        let image = prepared.image.unwrap();
        assert_eq!(image.raster.as_bytes(0).unwrap(), &[56, 34, 12, 78]);
        assert_eq!(image.width, 1);
        assert_eq!(
            prepared.preview.content,
            files::Content::Binary(Box::default())
        );
    }
    #[test]
    fn malformed_images_fail_and_text_is_untouched() {
        let text = files::Preview {
            kind: files::PreviewKind::Text,
            mime: "text/plain".into(),
            content: files::Content::Text(Box::new("hello".into())),
            truncated: false,
        };
        let prepared = super::prepare(text.clone()).unwrap();
        assert!(prepared.image.is_none());
        assert_eq!(prepared.preview, text);
        for (mime, content) in [
            ("image/png", &b"!!!"[..]),
            ("image/jpeg", b"abc"),
            ("image/svg+xml", b"abc"),
        ] {
            assert!(super::prepare(preview(mime, content.to_vec())).is_err());
        }
        let mut missing = preview("image/png", Vec::new());
        missing.content = files::Content::Text(Box::default());
        assert!(super::prepare(missing).is_err());
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
            let (image, prepared) =
                prepare(preview("image/jpeg", fixture::oriented_jpeg(orientation))).unwrap();
            let (width, height) = if orientation >= 5 { (40, 80) } else { (80, 40) };
            assert_eq!(prepared.width, width, "orientation {orientation}");
            assert_eq!(prepared.height, height, "orientation {orientation}");
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
            let prepared = super::prepare(preview(mime, fixture::animation(format, 2))).unwrap();
            let image = prepared.image.unwrap().raster;
            assert_eq!(image.frame_count(), 2, "{mime}");
            assert_eq!(&image.as_bytes(0).unwrap()[..4], &[20, 20, 240, 255]);
            assert_eq!(&image.as_bytes(1).unwrap()[..4], &[20, 220, 20, 255]);
            assert_eq!(std::time::Duration::from(image.delay(0)).as_millis(), 400);
            assert_eq!(std::time::Duration::from(image.delay(1)).as_millis(), 600);
            assert_eq!(
                prepared.preview.content,
                files::Content::Binary(Box::default())
            );
        }
    }
    #[test]
    fn animated_png_and_webp_enforce_frame_limit_and_reject_truncation() {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::WebP, "image/webp"),
        ] {
            let preview = |bytes: Vec<u8>| preview(mime, bytes);
            let (image, _) =
                prepare(preview(fixture::animation(format, super::MAX_FRAMES))).unwrap();
            assert_eq!(image.frame_count(), super::MAX_FRAMES);
            let error = prepare(preview(fixture::animation(format, super::MAX_FRAMES + 1)))
                .err()
                .unwrap();
            assert!(error.contains("frame limit"), "{mime}: {error}");
            let bytes = fixture::animation(format, 2);
            assert!(
                prepare(preview(bytes[..bytes.len() / 2].to_vec())).is_err(),
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
        let preview = |bytes: Vec<u8>| preview("image/gif", bytes);
        let (image, _) = prepare(preview(gif(2))).unwrap();
        assert_eq!(image.as_bytes(0).unwrap(), &[0, 0, 0, 255]);
        assert_eq!(image.as_bytes(1).unwrap(), &[0, 0, 1, 255]);
        assert!(prepare(preview(gif(super::MAX_FRAMES + 1))).is_err());
        let mut oversized = gif(1);
        oversized[6..10].fill(255);
        let error = prepare(preview(oversized)).err().unwrap();
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
        assert!(prepare(preview(oversized_frame)).is_err());
    }
}
