//! Perceptual signatures: what makes two photos "the same shot".
//!
//! Deliberately two-stage, because one stage is not enough. A difference hash is a
//! cheap *candidate* generator with a high false-positive rate; on the reference
//! library dHash alone grouped 85 photos taken across six different days. The second
//! stage — a contrast-normalized pixel comparison — is what makes the answer true.
//! See ADR-0003.

use anyhow::{Context, Result};
use image::imageops::FilterType;
use std::path::Path;

/// Side of the cached thumbnail used for pixel comparison.
pub const THUMB: usize = 32;

#[derive(Debug, Clone)]
pub struct Signature {
    /// 64-bit difference hash, for candidate generation only.
    pub dhash: u64,
    /// 32x32 grayscale, stored raw. Normalized at compare time.
    pub thumb: Vec<u8>,
    /// Variance of the Laplacian: higher is sharper. Picks the keeper in a burst.
    pub sharpness: f64,
    pub width: u32,
    pub height: u32,
}

/// Decode a JPEG at reduced scale using the decoder's own DCT downscaling.
///
/// This matters more than it looks. Signatures only need a 32x32 thumbnail, but a
/// full 12MP decode costs ~1.3s per photo — around 50 minutes for a 2500-photo
/// library. Asking the JPEG decoder for a 1/4 or 1/8 scale image instead makes it
/// roughly an order of magnitude cheaper for an identical result at 32x32.
fn decode_jpeg_scaled(path: &Path, target: u16) -> Result<image::GrayImage> {
    let file = std::fs::File::open(path)?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    dec.read_info()?;
    let info = dec.info().context("jpeg has no frame info")?;
    dec.scale(target, target)?;
    let pixels = dec.decode()?;
    let info2 = dec.info().context("jpeg has no frame info after scale")?;
    let (w, h) = (info2.width as u32, info2.height as u32);

    let gray: Vec<u8> = match info2.pixel_format {
        jpeg_decoder::PixelFormat::L8 => pixels,
        jpeg_decoder::PixelFormat::RGB24 => pixels
            .chunks_exact(3)
            .map(|p| {
                ((u16::from(p[0]) * 77 + u16::from(p[1]) * 150 + u16::from(p[2]) * 29) >> 8) as u8
            })
            .collect(),
        other => anyhow::bail!("unsupported jpeg pixel format {other:?} (orig {}x{})", info.width, info.height),
    };
    image::GrayImage::from_raw(w, h, gray).context("assembling decoded jpeg")
}

fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
}

pub fn compute(path: &Path) -> Result<Signature> {
    let (gray, width, height) = if is_jpeg(path) {
        // Full dimensions come from the header, so they stay accurate despite scaling.
        let file = std::fs::File::open(path)?;
        let mut probe = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
        probe.read_info()?;
        let info = probe.info().context("jpeg has no frame info")?;
        let g = decode_jpeg_scaled(path, 512)?;
        // Orientation must be applied before hashing, or the same scene shot in
        // portrait and landscape would never match.
        let o = crate::imageio::orientation(path);
        let (fw, fh) = if matches!(o, 5..=8) {
            (u32::from(info.height), u32::from(info.width))
        } else {
            (u32::from(info.width), u32::from(info.height))
        };
        (crate::imageio::apply_luma(g, o), fw, fh)
    } else {
        let img = image::ImageReader::open(path)
            .with_context(|| format!("opening {}", path.display()))?
            .with_guessed_format()?
            .decode()
            .with_context(|| format!("decoding {}", path.display()))?;
        let g = crate::imageio::apply_luma(img.into_luma8(), crate::imageio::orientation(path));
        let (w, h) = (g.width(), g.height());
        (g, w, h)
    };

    let dh = image::imageops::resize(&gray, (THUMB + 1) as u32, THUMB as u32, FilterType::Triangle);
    let mut dhash = 0u64;
    for y in 0..8u32 {
        for x in 0..8u32 {
            // Sample the 9x8 grid at 1/4 scale to keep the classic 8x8 dHash shape.
            let sx = x * (THUMB as u32 + 1) / 8;
            let sy = y * THUMB as u32 / 8;
            let a = dh.get_pixel(sx, sy)[0];
            let b = dh.get_pixel(sx + 1, sy)[0];
            dhash = (dhash << 1) | u64::from(a > b);
        }
    }

    let t = image::imageops::resize(&gray, THUMB as u32, THUMB as u32, FilterType::Triangle);
    let thumb = t.into_raw();

    let sharp = image::imageops::resize(&gray, 256, 256, FilterType::Triangle);
    let sharpness = laplacian_variance(&sharp, 256, 256);

    Ok(Signature { dhash, thumb, sharpness, width, height })
}

/// Variance of a 4-neighbour Laplacian. A blurred frame has little edge energy.
fn laplacian_variance(px: &[u8], w: usize, h: usize) -> f64 {
    let at = |x: usize, y: usize| f64::from(px[y * w + x]);
    let mut vals = Vec::with_capacity((w - 2) * (h - 2));
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            vals.push(at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y));
        }
    }
    let n = vals.len() as f64;
    let mean = vals.iter().sum::<f64>() / n;
    vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Contrast-normalized RMSE between two thumbnails. Normalizing per image makes the
/// comparison insensitive to exposure differences between frames of one burst, so a
/// genuine re-shoot still matches while a different scene does not.
pub fn rmse(a: &[u8], b: &[u8]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let norm = |v: &[u8]| {
        let n = v.len() as f32;
        let mean = v.iter().map(|&x| f32::from(x)).sum::<f32>() / n;
        let sd = (v.iter().map(|&x| (f32::from(x) - mean).powi(2)).sum::<f32>() / n)
            .sqrt()
            .max(1e-6);
        v.iter().map(move |&x| (f32::from(x) - mean) / sd).collect::<Vec<f32>>()
    };
    let (na, nb) = (norm(a), norm(b));
    let sum: f32 = na.iter().zip(&nb).map(|(x, y)| (x - y).powi(2)).sum();
    (sum / na.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_thumbs_have_zero_rmse() {
        let a: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        assert!(rmse(&a, &a) < 1e-5);
    }

    #[test]
    fn rmse_ignores_exposure_shift() {
        // Same scene, brighter. Normalization must see through it.
        let a: Vec<u8> = (0..1024).map(|i| (i % 200) as u8).collect();
        let b: Vec<u8> = a.iter().map(|&x| x.saturating_add(30)).collect();
        assert!(rmse(&a, &b) < 0.30, "got {}", rmse(&a, &b));
    }

    #[test]
    fn rmse_separates_different_scenes() {
        let a: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        let b: Vec<u8> = (0..1024).map(|i| ((i * 7 + 91) % 256) as u8).collect();
        assert!(rmse(&a, &b) > 0.45, "got {}", rmse(&a, &b));
    }

    #[test]
    fn hamming_counts_bits() {
        assert_eq!(hamming(0b1011, 0b1000), 2);
    }
}
