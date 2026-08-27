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

use anyhow::Result;
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

/// Read image dimensions from the header without decoding pixels, with EXIF
/// orientation applied. Cheap enough to call per face.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    dec.read_info().ok()?;
    let info = dec.info()?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    Some(if matches!(orientation(path), 5..=8) { (h, w) } else { (w, h) })
}

/// Decode to RGB with EXIF orientation applied.
pub fn load_rgb(path: &Path) -> Result<RgbImage> {
    let img: DynamicImage = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
    Ok(apply_rgb(img.to_rgb8(), orientation(path)))
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
    fn unknown_orientation_is_a_no_op() {
        let img = RgbImage::new(3, 5);
        assert_eq!(apply_rgb(img, 99).dimensions(), (3, 5));
    }
}
