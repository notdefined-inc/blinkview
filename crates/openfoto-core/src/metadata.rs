//! Reading, and removing, the metadata a photograph carries.
//!
//! Two different jobs behind one word. *Reading* answers "what does this file say about
//! where and how it was taken" — camera, lens, exposure, and above all whether there
//! are coordinates in it. *Removing* answers "take that out before I send it", which is
//! the reason the reading exists.
//!
//! Removal is a **segment rewrite, never a re-encode**. The entropy-coded image data is
//! copied byte for byte, so the pixels that come out of the decoder are bit-identical
//! and nothing is recompressed. Re-encoding through an image crate would lose quality
//! to remove data that is not in the pixels, and would be slower by orders of
//! magnitude.
//!
//! What is kept is as deliberate as what goes. JFIF (APP0), ICC colour profiles (APP2)
//! and Adobe's colour-transform marker (APP14) are structure and colour, not a record
//! of the photographer; dropping them would change how the photograph *looks*, which is
//! not what "strip metadata" means. EXIF and XMP (APP1), IPTC (APP13), maker notes in
//! the other APPn slots, and free-text comments (COM) all go.
//!
//! Stripping a photograph loses its EXIF timestamp, and `taken_at` comes from EXIF
//! first (ADR-0003). See ADR-0015: the original is kept by default for exactly that
//! reason.

use anyhow::{bail, Result};
use serde::Serialize;
use std::path::Path;

/// What a photograph says about how it was taken.
///
/// Every field is optional because every field genuinely is: a screenshot has none of
/// them, and a phone photograph run through a messaging app has been stripped already.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Exif {
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<String>,
    pub exposure: Option<String>,
    pub aperture: Option<String>,
    pub focal: Option<String>,
    /// Latitude and longitude as written, human-readable. Reported as text rather than
    /// numbers because the question being answered is "does this say where I live",
    /// and the answer is the reading itself.
    pub gps: Option<String>,
    /// True when the file carried any EXIF at all, so "nothing here" can be told apart
    /// from "not read".
    pub present: bool,
}

/// Read what a photograph says about itself. A file with no EXIF is not an error.
pub fn read(path: &Path) -> Exif {
    let Ok(file) = std::fs::File::open(path) else { return Exif::default() };
    let mut buf = std::io::BufReader::new(file);
    let Ok(ex) = exif::Reader::new().read_from_container(&mut buf) else {
        return Exif::default();
    };
    let get = |tag: exif::Tag| {
        ex.get_field(tag, exif::In::PRIMARY)
            .map(|f| f.display_value().with_unit(&ex).to_string())
            .map(|s| s.trim_matches('"').trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let camera = match (get(exif::Tag::Make), get(exif::Tag::Model)) {
        (Some(make), Some(model)) => {
            // Phone models usually repeat the maker ("Apple iPhone 15"), and printing
            // it twice reads as a bug.
            if model.to_lowercase().starts_with(&make.to_lowercase()) {
                Some(model)
            } else {
                Some(format!("{make} {model}"))
            }
        }
        (a, b) => a.or(b),
    };
    // `with_unit` already appends N/S/E/W from the reference tags, so adding them
    // again reads as "51 deg 30 min 0 sec N N".
    let gps = match (get(exif::Tag::GPSLatitude), get(exif::Tag::GPSLongitude)) {
        (Some(lat), Some(lon)) => Some(format!("{lat}, {lon}")),
        _ => None,
    };
    Exif {
        camera,
        lens: get(exif::Tag::LensModel),
        iso: get(exif::Tag::PhotographicSensitivity),
        exposure: get(exif::Tag::ExposureTime),
        aperture: get(exif::Tag::FNumber),
        focal: get(exif::Tag::FocalLength),
        gps,
        present: true,
    }
}

/// Whether this file is a format whose metadata can be removed without re-encoding.
pub fn strippable(path: &Path) -> bool {
    matches!(ext(path).as_str(), "jpg" | "jpeg" | "png")
}

fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The same bytes without their metadata segments.
pub fn strip_bytes(path: &Path, bytes: &[u8]) -> Result<Vec<u8>> {
    match ext(path).as_str() {
        "jpg" | "jpeg" => strip_jpeg(bytes),
        "png" => strip_png(bytes),
        other => bail!("openfoto cannot strip {} files", other.to_uppercase()),
    }
}

/// Markers that carry a record of the photographer rather than of the image.
fn drops_jpeg(marker: u8) -> bool {
    match marker {
        0xFE => true,                // COM — a free-text comment
        0xE0 | 0xE2 | 0xEE => false, // JFIF, ICC profile, Adobe colour transform
        0xE1..=0xEF => true,         // EXIF, XMP, IPTC, maker notes
        _ => false,
    }
}

fn strip_jpeg(b: &[u8]) -> Result<Vec<u8>> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        bail!("not a JPEG");
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[..2]);
    let mut i = 2;
    while i + 1 < b.len() {
        if b[i] != 0xFF {
            bail!("expected a JPEG marker at byte {i}");
        }
        let marker = b[i + 1];
        // Standalone markers carry no length word.
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            out.extend_from_slice(&b[i..i + 2]);
            i += 2;
            if marker == 0xD9 {
                break;
            }
            continue;
        }
        if i + 4 > b.len() {
            bail!("truncated JPEG");
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if len < 2 || i + 2 + len > b.len() {
            bail!("bad JPEG segment length");
        }
        if marker == 0xDA {
            // Start of scan: everything after this is entropy-coded image data, and it
            // is copied whole. This is what makes the operation lossless.
            out.extend_from_slice(&b[i..]);
            return Ok(out);
        }
        if !drops_jpeg(marker) {
            out.extend_from_slice(&b[i..i + 2 + len]);
        }
        i += 2 + len;
    }
    Ok(out)
}

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn strip_png(b: &[u8]) -> Result<Vec<u8>> {
    if b.len() < 8 || b[..8] != PNG_SIG {
        bail!("not a PNG");
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&PNG_SIG);
    let mut i = 8;
    while i + 12 <= b.len() {
        let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let kind = &b[i + 4..i + 8];
        let end = i
            .checked_add(12)
            .and_then(|e| e.checked_add(len))
            .filter(|e| *e <= b.len())
            .ok_or_else(|| anyhow::anyhow!("truncated PNG chunk"))?;
        // iCCP and gAMA stay: they are colour, not a record of the photographer.
        let drop = matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME");
        if !drop {
            out.extend_from_slice(&b[i..end]);
        }
        i = end;
        if kind == b"IEND" {
            break;
        }
    }
    Ok(out)
}

/// Strip one photograph in the library, keeping the original by default.
///
/// The bytes are read before anything moves, and the replacement is written beside the
/// target and swapped in, so a failure can never leave a truncated file where the
/// photograph used to be.
pub fn strip_file(lib: &crate::Library, rel_path: &str, keep: bool) -> Result<Stripped> {
    let src = lib.abs(rel_path);
    if !strippable(&src) {
        bail!(
            "openfoto cannot strip {} files",
            ext(&src).to_uppercase()
        );
    }
    let bytes = std::fs::read(&src)?;
    let out = strip_bytes(&src, &bytes)?;
    let original = if keep { crate::edit::keep_original(lib, rel_path)? } else { None };
    let tmp = src.with_extension("openfoto-tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, &src)?;
    Ok(Stripped { original, hash: crate::scan::hash_file(&src)? })
}

/// What a strip did.
#[derive(Debug, Clone)]
pub struct Stripped {
    /// Where the untouched original ended up, when it was kept.
    pub original: Option<String>,
    /// The rewritten file's content hash — everything the user authored is keyed by it.
    pub hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal JPEG: SOI, an APP0, an APP1 pretending to be EXIF, a comment, an ICC
    /// APP2, then SOS and some scan data.
    fn jpeg() -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8];
        let seg = |b: &mut Vec<u8>, marker: u8, body: &[u8]| {
            b.extend_from_slice(&[0xFF, marker]);
            b.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
            b.extend_from_slice(body);
        };
        seg(&mut b, 0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0");
        seg(&mut b, 0xE1, b"Exif\0\0IIsecrets and coordinates");
        seg(&mut b, 0xFE, b"a comment naming the photographer");
        seg(&mut b, 0xE2, b"ICC_PROFILE\0the colour profile");
        seg(&mut b, 0xEE, b"Adobe\0transform");
        seg(&mut b, 0xDA, b"\x01\x01\0\0");
        b.extend_from_slice(&[0x12, 0x34, 0x56, 0x78, 0xFF, 0xD9]);
        b
    }

    #[test]
    fn stripping_a_jpeg_takes_the_record_and_leaves_the_image() {
        let src = jpeg();
        let out = strip_bytes(Path::new("a.jpg"), &src).unwrap();

        // Gone: EXIF and the comment.
        assert!(!contains(&out, b"secrets and coordinates"), "EXIF survived");
        assert!(!contains(&out, b"naming the photographer"), "the comment survived");
        // Kept: the things that decide how it looks.
        assert!(contains(&out, b"ICC_PROFILE"), "the colour profile must stay");
        assert!(contains(&out, b"JFIF"), "JFIF must stay");
        assert!(contains(&out, b"Adobe"), "the Adobe transform must stay");
        // Kept byte for byte: the scan data and the end marker.
        assert!(out.ends_with(&[0x12, 0x34, 0x56, 0x78, 0xFF, 0xD9]), "image data changed");
        assert!(out.len() < src.len());
    }

    #[test]
    fn the_scan_data_is_copied_not_re_encoded() {
        // The bytes after SOS must appear unchanged and in one piece — that is the
        // whole claim of a lossless strip.
        let src = jpeg();
        let out = strip_bytes(Path::new("a.jpg"), &src).unwrap();
        let sos = out.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        let src_sos = src.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        assert_eq!(&out[sos..], &src[src_sos..]);
    }

    #[test]
    fn stripping_a_png_drops_text_and_keeps_the_pixels() {
        let mut b = PNG_SIG.to_vec();
        let chunk = |b: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]| {
            b.extend_from_slice(&(body.len() as u32).to_be_bytes());
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b.extend_from_slice(&[0, 0, 0, 0]); // CRC, not checked here
        };
        chunk(&mut b, b"IHDR", &[0; 13]);
        chunk(&mut b, b"tEXt", b"Author\0someone");
        chunk(&mut b, b"eXIf", b"IIcoordinates");
        chunk(&mut b, b"iCCP", b"profile");
        chunk(&mut b, b"IDAT", b"pixels");
        chunk(&mut b, b"IEND", b"");

        let out = strip_bytes(Path::new("a.png"), &b).unwrap();
        assert!(!contains(&out, b"someone"));
        assert!(!contains(&out, b"coordinates"));
        assert!(contains(&out, b"iCCP"), "the colour profile must stay");
        assert!(contains(&out, b"pixels"), "the image data must stay");
        assert!(contains(&out, b"IEND"));
    }

    #[test]
    fn a_format_that_cannot_be_stripped_says_so_by_name() {
        let e = strip_bytes(Path::new("clip.mov"), &[0; 8]).unwrap_err().to_string();
        assert!(e.contains("MOV"), "{e}");
        assert!(!strippable(Path::new("a.heic")));
        assert!(strippable(Path::new("a.JPG")));
    }

    #[test]
    fn rubbish_is_refused_rather_than_half_written() {
        assert!(strip_bytes(Path::new("a.jpg"), b"not a jpeg at all").is_err());
        assert!(strip_bytes(Path::new("a.png"), b"not a png at all").is_err());
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
}
