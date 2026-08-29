//! Loading images the way the rest of the world sees them.
//!
//! Phone cameras store photos in the sensor's native orientation and record how to
//! rotate them in EXIF tag 274, rather than rotating the pixels. On the reference
//! library **59 of 60** sampled photos carry a non-upright tag. OpenCV's `imread`
//! applies it; the `image` crate deliberately does not.
//!
//! Skipping this step is quietly catastrophic for face work: YuNet detects upright
//! faces, so sideways input dropped detection from ~120 faces across 120 person photos
//! to 22. Every decode in this crate goes through here.

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage, RgbImage};
use std::path::Path;

/// EXIF orientation, defaulting to 1 (upright) when absent or unreadable.
pub fn orientation(path: &Path) -> u16 {
    let Ok(file) = std::fs::File::open(path) else { return 1 };
    let mut r = std::io::BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut r) else { return 1 };
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map(|v| v as u16)
        .filter(|v| (1..=8).contains(v))
        .unwrap_or(1)
}

pub fn apply_rgb(img: RgbImage, o: u16) -> RgbImage {
    use image::imageops::*;
    match o {
        2 => flip_horizontal(&img),
        3 => rotate180(&img),
        4 => flip_vertical(&img),
        5 => rotate90(&flip_horizontal(&img)),
        6 => rotate90(&img),
        7 => rotate270(&flip_horizontal(&img)),
        8 => rotate270(&img),
        _ => img,
    }
}

pub fn apply_luma(img: GrayImage, o: u16) -> GrayImage {
    use image::imageops::*;
    match o {
        2 => flip_horizontal(&img),
        3 => rotate180(&img),
        4 => flip_vertical(&img),
        5 => rotate90(&flip_horizontal(&img)),
        6 => rotate90(&img),
        7 => rotate270(&flip_horizontal(&img)),
        8 => rotate270(&img),
        _ => img,
    }
}

/// Formats the `image` crate cannot decode, which macOS can.
pub fn needs_conversion(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "heic" | "heif"))
}

/// Decode a HEIC/HEIF by asking macOS to transcode it first.
///
/// iPhones shoot HEIC by default, so a photo tool that cannot read it is missing most
/// of a modern camera roll. There is no pure-Rust decoder worth depending on, and
/// libheif would add a system library; `sips` ships with macOS and handles it. The
/// cost is a process per image, which is why callers cache the result rather than
/// converting on every view.
pub fn convert_to_jpeg(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = std::process::Command::new("sips")
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "90"])
        .arg(src)
        .arg("--out")
        .arg(dst)
        .output()
        .context("running sips")?;
    if !out.status.success() || !dst.exists() {
        anyhow::bail!(
            "sips could not convert {}: {}",
            src.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Read image dimensions from the header without decoding pixels, with EXIF
/// orientation applied. Cheap enough to call per face.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    if needs_conversion(path) {
        // No cheap header path for HEIC; fall back to a full load.
        return load_rgb(path).ok().map(|i| (i.width(), i.height()));
    }
    let file = std::fs::File::open(path).ok()?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    dec.read_info().ok()?;
    let info = dec.info()?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    Some(if matches!(orientation(path), 5..=8) { (h, w) } else { (w, h) })
}

/// Decode to RGB with EXIF orientation applied, transcoding first when the format
/// needs it (HEIC/HEIF).
/// The JPEG preview a camera already wrote into the file, if it is good enough to be
/// our thumbnail.
///
/// Phone cameras embed a preview in the EXIF APP1 segment — on the Samsung backup this
/// was measured at 96% of JPEGs, every one of them exactly 512px on the long edge, which
/// is the size we want, and not one with an aspect ratio that disagreed with its photo.
///
/// Using it turns a thumbnail from "decode 12 megapixels and throw 99% away" into
/// "read 37 KB and decode a small image". Decoding the full frame costs ~60 ms and
/// cannot be avoided by asking for a scaled decode, because a JPEG's entropy coding has
/// to be walked in full either way — measured: turbojpeg at 1/8 scale saved only 22%.
///
/// Returns `None` whenever anything is off — no preview, too small, or an aspect ratio
/// that disagrees with the real image. A thumbnail that does not match its photograph
/// is worse than a slow one, so every doubt falls back to decoding properly.
pub fn embedded_preview(bytes: &[u8], min_long: u32) -> Option<RgbImage> {
    let (fw, fh) = jpeg_size(bytes)?;
    let thumb = find_app1_jpeg(bytes)?;
    let (tw, th) = jpeg_size(thumb)?;
    if tw.max(th) < min_long {
        return None;
    }
    // The preview must be the same photograph, not a differently cropped one.
    let (fa, ta) = (fw as f32 / fh as f32, tw as f32 / th as f32);
    if (fa - ta).abs() > 0.02 * fa.max(ta) {
        return None;
    }
    let img = image::load_from_memory_with_format(thumb, image::ImageFormat::Jpeg).ok()?;
    Some(img.to_rgb8())
}

/// Width and height from the first SOF marker, without decoding anything.
fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        // SOF0/1/2 carry the frame size; SOF4 (DHT) and friends must not be read as one.
        if matches!(marker, 0xC0..=0xC2) {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return Some((w, h));
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

/// The complete JPEG embedded inside the EXIF APP1 segment.
fn find_app1_jpeg(b: &[u8]) -> Option<&[u8]> {
    let mut i = 2usize;
    while i + 4 < b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if matches!(marker, 0xC0..=0xC2 | 0xDA) {
            return None; // reached image data; there is no preview
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if len < 2 || i + 2 + len > b.len() {
            return None;
        }
        if marker == 0xE1 {
            let seg = &b[i + 4..i + 2 + len];
            let start = seg.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])?;
            let end = seg[start..].windows(2).rposition(|w| w == [0xFF, 0xD9])?;
            return Some(&seg[start..start + end + 2]);
        }
        i += 2 + len;
    }
    None
}

pub fn load_rgb(path: &Path) -> Result<RgbImage> {
    if needs_conversion(path) {
        let tmp = std::env::temp_dir().join(format!(
            "openfoto-heic-{}-{}.jpg",
            std::process::id(),
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
        ));
        convert_to_jpeg(path, &tmp)?;
        let img: DynamicImage = image::ImageReader::open(&tmp)?.with_guessed_format()?.decode()?;
        // `sips` *carries the EXIF orientation tag across* rather than baking the
        // rotation into the pixels — verified: a 4032x3024 HEIC with tag 6 converts to
        // a 4032x3024 JPEG still tagged 6. Browsers honour the tag, so the full-size
        // view looked right while our thumbnails came out rotated. Read it from the
        // converted file and apply it ourselves.
        let o = orientation(&tmp);
        let _ = std::fs::remove_file(&tmp);
        return Ok(apply_rgb(img.to_rgb8(), o));
    }
    let img: DynamicImage = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
    Ok(apply_rgb(img.to_rgb8(), orientation(path)))
}

/// As [`load_rgb`], but without applying EXIF orientation.
///
/// For callers that are about to shrink the image: rotating twelve megapixels costs
/// 14 ms and rotating the 512-pixel result costs 0.2 ms, for the same picture.
/// Callers must apply [`orientation`] themselves — see `thumbs::render_one`, which does
/// it after shrinking. Not for HEIC, whose conversion rotates on the way through.
pub fn load_rgb_unrotated(path: &Path) -> Result<RgbImage> {
    debug_assert!(!needs_conversion(path), "HEIC is rotated during conversion");
    let img: DynamicImage = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
    Ok(img.to_rgb8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_swaps_dimensions() {
        let img = RgbImage::new(40, 10);
        assert_eq!(apply_rgb(img.clone(), 6).dimensions(), (10, 40));
        assert_eq!(apply_rgb(img.clone(), 8).dimensions(), (10, 40));
        assert_eq!(apply_rgb(img.clone(), 3).dimensions(), (40, 10));
        assert_eq!(apply_rgb(img, 1).dimensions(), (40, 10));
    }

    #[test]
    fn orientation_six_moves_the_top_left_pixel_to_the_top_right() {
        let mut img = RgbImage::new(4, 2);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        let r = apply_rgb(img, 6); // rotate 90 CW: (0,0) -> (height-1, 0)
        assert_eq!(r.dimensions(), (2, 4));
        assert_eq!(*r.get_pixel(1, 0), image::Rgb([255, 0, 0]));
    }

    #[test]
    fn recognises_formats_needing_conversion() {
        assert!(needs_conversion(Path::new("/x/IMG_1234.HEIC")));
        assert!(needs_conversion(Path::new("/x/a.heif")));
        assert!(!needs_conversion(Path::new("/x/a.jpg")));
        assert!(!needs_conversion(Path::new("/x/a.png")));
    }

    #[test]
    fn unknown_orientation_is_a_no_op() {
        let img = RgbImage::new(3, 5);
        assert_eq!(apply_rgb(img, 99).dimensions(), (3, 5));
    }
}
