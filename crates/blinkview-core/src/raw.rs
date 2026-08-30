//! Camera RAW, read as the photograph the camera already made.
//!
//! Blinkview does not develop a RAW file. Every one of these containers carries a JPEG
//! the camera rendered at capture — the image on the back of the camera — and that is
//! what gets indexed, thumbnailed, clustered and displayed. Demosaicing a 45-megapixel
//! frame costs seconds per file; reading its preview costs milliseconds.
//!
//! **Finding it has to be structural.** Scanning a RAW for JPEG start markers picks the
//! wrong bytes, and does so silently. Measured on the reference samples: a Canon CR2
//! stores its sensor data as a 2238x2954 lossless JPEG — taller and wider than the real
//! 2496x1664 preview in one dimension, portrait where the photograph is landscape — and
//! a compressed-lossless DNG carries around 350 such tiles. So we follow the tags that
//! *declare* a preview, and then check the frame marker: sensor data is SOF3 (lossless
//! JPEG), a viewable preview is SOF0, SOF1 or SOF2. Nothing else is accepted.
//!
//! Reads are seeks, not slurps. A directory is a few hundred bytes and a preview is
//! about a megabyte, so a 50 MB RAF costs a fraction of itself to thumbnail.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The formats verified against real files from raw.pixls.us. Others are a table entry
/// away, but an unverified format is a claim we have not earned.
pub const RAW_EXT: &[&str] = &["cr2", "cr3", "dng", "nef", "arw", "raf"];

/// The smallest long edge worth preferring over decoding the file properly. Matches
/// [`crate::thumbs::THUMB_LONG`]: below it, a preview cannot fill a thumbnail.
const MIN_LONG: u32 = 512;

/// A directory could name an absurd length; a preview is a JPEG, not a disk image.
const MAX_PREVIEW: u64 = 64 << 20;

/// Cycles and bombs: a TIFF directory may point anywhere, including at itself.
const MAX_DIRS: usize = 48;

pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

pub struct Preview {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Where a candidate preview claims to live.
#[derive(Clone, Copy, PartialEq)]
struct Span {
    at: u64,
    len: u64,
}

/// The largest declared preview in a RAW file that survives validation.
///
/// `None` when the container declares none, when every candidate turns out to be sensor
/// data, or when the file is truncated — all of which are answers, not errors. The
/// caller falls back to decoding the file some other way.
pub fn preview(path: &Path) -> Option<Preview> {
    let mut f = File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let mut head = [0u8; 16];
    f.read_exact(&mut head).ok()?;

    let mut spans = if &head[..4] == b"FUJI" {
        raf_spans(&mut f)
    } else if &head[4..8] == b"ftyp" {
        cr3_spans(&mut f, size)
    } else {
        tiff_spans(&mut f, &head)
    };

    // Largest first: the camera's full-size render beats its 160x120 stub.
    spans.sort_by_key(|s| std::cmp::Reverse(s.len));
    spans.dedup_by_key(|s| s.at);
    for span in spans {
        if span.len < 1024 || span.len > MAX_PREVIEW || span.at.saturating_add(span.len) > size {
            continue;
        }
        let Some(bytes) = read_at(&mut f, span.at, span.len as usize) else { continue };
        let Some((w, h)) = viewable_jpeg(&bytes) else { continue };
        if w.max(h) >= MIN_LONG {
            return Some(Preview { jpeg: bytes, width: w, height: h });
        }
    }
    None
}

fn read_at(f: &mut File, at: u64, len: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// Dimensions of a JPEG that can actually be displayed.
///
/// The frame marker is the whole point. SOF3 is lossless JPEG, which inside a RAW means
/// sensor data: it decodes to nothing a person would recognise, and shipping it as a
/// thumbnail is worse than having none.
fn viewable_jpeg(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 || b[2] != 0xFF {
        return None;
    }
    let mut p = 2usize;
    while p + 4 <= b.len() {
        if b[p] != 0xFF {
            return None;
        }
        let marker = b[p + 1];
        // Standalone markers carry no length.
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            p += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return None; // reached the scan without a frame header
        }
        let seg = u16::from_be_bytes([b[p + 2], b[p + 3]]) as usize;
        match marker {
            0xC0..=0xC2 => {
                let (h, w) = (
                    u16::from_be_bytes([*b.get(p + 5)?, *b.get(p + 6)?]),
                    u16::from_be_bytes([*b.get(p + 7)?, *b.get(p + 8)?]),
                );
                return (w > 0 && h > 0).then_some((u32::from(w), u32::from(h)));
            }
            // SOF3 and the arithmetic-coded frames: not a picture anyone can look at.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            _ => {}
        }
        p = p.checked_add(2 + seg.max(2))?;
    }
    None
}

// ------------------------------------------------------------------ TIFF

// CR2, NEF, ARW and DNG are all TIFF underneath, differing in which directory holds the
// preview: Nikon hides it in a SubIFD, Canon points at it with strip offsets, Adobe uses
// the JPEG interchange pair. Collecting all three shapes and validating afterwards costs
// a few directory reads and avoids one branch per manufacturer.
const T_STRIP_OFFSETS: u16 = 0x0111;
const T_STRIP_BYTES: u16 = 0x0117;
const T_SUB_IFD: u16 = 0x014A;
const T_JPEG_OFFSET: u16 = 0x0201;
const T_JPEG_LENGTH: u16 = 0x0202;

fn tiff_spans(f: &mut File, head: &[u8]) -> Vec<Span> {
    let be = match &head[..2] {
        b"MM" => true,
        b"II" => false,
        _ => return Vec::new(),
    };
    let first = u32b(&head[4..8], be) as u64;
    let mut out = Vec::new();
    let mut seen = Vec::new();
    walk_ifd(f, be, first, 0, &mut out, &mut seen);
    out
}

fn walk_ifd(f: &mut File, be: bool, at: u64, depth: u8, out: &mut Vec<Span>, seen: &mut Vec<u64>) {
    if depth > 3 || seen.len() >= MAX_DIRS || at == 0 || seen.contains(&at) {
        return;
    }
    seen.push(at);
    let Some(count_bytes) = read_at(f, at, 2) else { return };
    let count = u16b(&count_bytes, be) as usize;
    if count == 0 || count > 512 {
        return;
    }
    let Some(entries) = read_at(f, at + 2, count * 12 + 4) else { return };

    let value = |tag: u16| -> Option<u32> {
        (0..count).find_map(|i| {
            let e = &entries[i * 12..i * 12 + 12];
            (u16b(e, be) == tag).then(|| u32b(&e[8..12], be))
        })
    };
    if let (Some(off), Some(len)) = (value(T_JPEG_OFFSET), value(T_JPEG_LENGTH)) {
        out.push(Span { at: u64::from(off), len: u64::from(len) });
    }
    // A single-strip image is a whole JPEG at one offset. Multi-strip means the picture
    // is cut into pieces, which is a tiled sensor image, never a preview.
    for i in 0..count {
        let e = &entries[i * 12..i * 12 + 12];
        if u16b(e, be) == T_STRIP_OFFSETS && u32b(&e[4..8], be) == 1 {
            if let Some(len) = value(T_STRIP_BYTES) {
                out.push(Span { at: u64::from(u32b(&e[8..12], be)), len: u64::from(len) });
            }
        }
    }

    // SubIFDs: where Nikon keeps the full-size preview.
    for i in 0..count {
        let e = &entries[i * 12..i * 12 + 12];
        if u16b(e, be) != T_SUB_IFD {
            continue;
        }
        let n = u32b(&e[4..8], be) as usize;
        if n == 1 {
            walk_ifd(f, be, u64::from(u32b(&e[8..12], be)), depth + 1, out, seen);
        } else if n > 1 && n <= 16 {
            let table = u64::from(u32b(&e[8..12], be));
            if let Some(bytes) = read_at(f, table, n * 4) {
                for c in bytes.chunks_exact(4) {
                    walk_ifd(f, be, u64::from(u32b(c, be)), depth + 1, out, seen);
                }
            }
        }
    }

    let next = u32b(&entries[count * 12..count * 12 + 4], be);
    walk_ifd(f, be, u64::from(next), depth, out, seen);
}

fn u16b(b: &[u8], be: bool) -> u16 {
    let v = [b[0], b[1]];
    if be { u16::from_be_bytes(v) } else { u16::from_le_bytes(v) }
}

fn u32b(b: &[u8], be: bool) -> u32 {
    let v = [b[0], b[1], b[2], b[3]];
    if be { u32::from_be_bytes(v) } else { u32::from_le_bytes(v) }
}

// ------------------------------------------------------------------ RAF

/// Fujifilm writes a fixed header: a big-endian offset and length at byte 84, pointing
/// at a whole JPEG. No directory to walk.
fn raf_spans(f: &mut File) -> Vec<Span> {
    let Some(b) = read_at(f, 84, 8) else { return Vec::new() };
    let at = u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let len = u64::from(u32::from_be_bytes([b[4], b[5], b[6], b[7]]));
    vec![Span { at, len }]
}

// ------------------------------------------------------------------ CR3

/// Canon's CR3 is ISO base media — the same box structure as an MP4.
///
/// Two places hold a picture. `moov/uuid/PRVW` carries a 1620x1080 preview, and the
/// first sample of the first track is the full-size JPEG — 6000x4000 on the reference
/// file, which is worth the extra boxes to reach. Sample location comes from the sample
/// table the way a player would read it: `stsz` for the size, `co64` or `stco` for the
/// offset.
fn cr3_spans(f: &mut File, size: u64) -> Vec<Span> {
    let mut out = Vec::new();
    walk_boxes(f, 0, size, 0, Want::Preview, &mut out, &mut (0usize));
    out
}

/// What a pass over the boxes is collecting. The tree is the same; only the leaves that
/// matter differ.
#[derive(Clone, Copy, PartialEq)]
enum Want {
    Preview,
    Exif,
}

fn walk_boxes(
    f: &mut File,
    start: u64,
    end: u64,
    depth: u8,
    want: Want,
    out: &mut Vec<Span>,
    n: &mut usize,
) {
    let mut p = start;
    while p + 8 <= end && *n < 512 && depth <= 6 {
        *n += 1;
        let Some(hdr) = read_at(f, p, 8) else { return };
        let mut size = u64::from(u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]));
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let mut body = p + 8;
        if size == 1 {
            let Some(big) = read_at(f, p + 8, 8) else { return };
            size = u64::from_be_bytes(big.try_into().unwrap_or([0; 8]));
            body = p + 16;
        } else if size == 0 {
            size = end - p;
        }
        if size < 8 || p + size > end {
            return;
        }
        match &typ {
            // A uuid box names itself with 16 bytes before its children start.
            b"uuid" => walk_boxes(f, body + 16, p + size, depth + 1, want, out, n),
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => {
                walk_boxes(f, body, p + size, depth + 1, want, out, n)
            }
            // Canon keeps the EXIF here, as a bare TIFF block rather than in a segment
            // any JPEG or TIFF reader would look in.
            b"CMT1" if want == Want::Exif => out.push(Span { at: body, len: p + size - body }),
            // Canon's preview and thumbnail boxes hold a JPEG after a short header.
            b"PRVW" | b"THMB" if want == Want::Preview => {
                if let Some(at) = jpeg_start_in(f, body, p + size) {
                    out.push(Span { at, len: p + size - at });
                }
            }
            b"stsz" if want == Want::Preview => {
                if let Some(len) = first_sample_size(f, body, p + size) {
                    // Paired with the offset found in this track's stco/co64 below; the
                    // sample table always carries both, in this order or the other.
                    out.push(Span { at: u64::MAX, len });
                }
            }
            b"stco" | b"co64" if want == Want::Preview => {
                if let Some(at) = first_chunk_offset(f, body, p + size, typ == *b"co64") {
                    if let Some(pending) = out.iter_mut().rev().find(|s| s.at == u64::MAX) {
                        pending.at = at;
                    }
                }
            }
            _ => {}
        }
        p += size;
    }
}

/// `stsz`: version+flags, a uniform sample size, a count, then a table when the uniform
/// size is zero. The first entry is the one we want either way.
fn first_sample_size(f: &mut File, body: u64, end: u64) -> Option<u64> {
    let b = read_at(f, body, 16.min((end - body) as usize))?;
    if b.len() < 12 {
        return None;
    }
    let uniform = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    if uniform != 0 {
        return Some(u64::from(uniform));
    }
    (b.len() >= 16).then(|| u64::from(u32::from_be_bytes([b[12], b[13], b[14], b[15]])))
}

fn first_chunk_offset(f: &mut File, body: u64, end: u64, wide: bool) -> Option<u64> {
    let want = if wide { 16 } else { 12 };
    let b = read_at(f, body, want.min((end - body) as usize))?;
    if b.len() < want {
        return None;
    }
    Some(if wide {
        u64::from_be_bytes(b[8..16].try_into().ok()?)
    } else {
        u64::from(u32::from_be_bytes(b[8..12].try_into().ok()?))
    })
}

/// The SOI inside one box, bounded by that box. Canon prefixes its preview with a short
/// header whose layout is undocumented, so the marker is found rather than assumed —
/// but only ever within the box that declares itself a preview.
fn jpeg_start_in(f: &mut File, body: u64, end: u64) -> Option<u64> {
    let want = 128usize.min((end - body) as usize);
    let b = read_at(f, body, want)?;
    b.windows(3)
        .position(|w| w == [0xFF, 0xD8, 0xFF])
        .map(|i| body + i as u64)
}

/// The TIFF block holding a RAW file's EXIF, for the containers that hide it.
///
/// A CR2, NEF, ARW or DNG *is* a TIFF, and an ordinary reader finds its EXIF unaided.
/// The other two do not: Canon's CR3 keeps it in a `CMT1` box, and Fujifilm's RAF keeps
/// it only in the APP1 segment of the preview it embeds. Without this, every Canon
/// R-series and Fujifilm frame is dated by its file timestamp — which for a copied file
/// is the day it was copied, and the timeline is ordered by exactly that.
pub fn exif_block(path: &Path) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let mut head = [0u8; 16];
    f.read_exact(&mut head).ok()?;

    if &head[..4] == b"FUJI" {
        let span = *raf_spans(&mut f).first()?;
        let len = span.len.min(4 << 20) as usize;
        if span.at.saturating_add(len as u64) > size {
            return None;
        }
        return app1_tiff(&read_at(&mut f, span.at, len)?);
    }
    if &head[4..8] == b"ftyp" {
        let mut spans = Vec::new();
        walk_boxes(&mut f, 0, size, 0, Want::Exif, &mut spans, &mut 0);
        let span = spans.into_iter().find(|s| s.len > 8 && s.len < (1 << 20))?;
        let block = read_at(&mut f, span.at, span.len as usize)?;
        return matches!(&block[..2], b"II" | b"MM").then_some(block);
    }
    None
}

/// The TIFF block inside a JPEG's `Exif\0\0` APP1 segment.
fn app1_tiff(jpeg: &[u8]) -> Option<Vec<u8>> {
    let mut p = 2usize;
    while p + 4 <= jpeg.len() && jpeg[p] == 0xFF {
        let marker = jpeg[p + 1];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            p += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let seg = u16::from_be_bytes([jpeg[p + 2], jpeg[p + 3]]) as usize;
        let body = jpeg.get(p + 4..p + 2 + seg)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(body[6..].to_vec());
        }
        p += 2 + seg.max(2);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_formats_it_claims() {
        assert!(is_raw(Path::new("/x/IMG_6310.CR3")));
        assert!(is_raw(Path::new("/x/a.nef")));
        assert!(is_raw(Path::new("/x/a.dng")));
        assert!(!is_raw(Path::new("/x/a.jpg")));
        assert!(!is_raw(Path::new("/x/a.heic")));
        // Not claimed until a real file has been through it.
        assert!(!is_raw(Path::new("/x/a.orf")));
    }

    /// The rule the whole module rests on: sensor data is lossless JPEG, and lossless
    /// JPEG is never a preview.
    #[test]
    fn lossless_frames_are_not_previews() {
        // SOI, then a frame header of the given marker, 8x8, one component.
        let frame = |marker: u8| {
            let mut v = vec![0xFF, 0xD8, 0xFF, marker, 0x00, 0x0B, 0x08];
            v.extend_from_slice(&[0x00, 0x08, 0x00, 0x08, 0x01, 0x00, 0x11, 0x00]);
            v
        };
        assert_eq!(viewable_jpeg(&frame(0xC0)), Some((8, 8)), "baseline is a picture");
        assert_eq!(viewable_jpeg(&frame(0xC2)), Some((8, 8)), "progressive is a picture");
        assert_eq!(viewable_jpeg(&frame(0xC3)), None, "SOF3 is sensor data");
        assert_eq!(viewable_jpeg(&frame(0xC9)), None, "arithmetic coding is not viewable");
        assert_eq!(viewable_jpeg(b"not a jpeg at all"), None);
        assert_eq!(viewable_jpeg(&[0xFF, 0xD8]), None, "truncated is not a picture");
    }

    #[test]
    fn a_truncated_file_is_an_answer_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("blinkview-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("broken.nef");
        // A TIFF header promising a directory that is not there.
        std::fs::write(&p, [b'M', b'M', 0, 42, 0, 0, 0x20, 0]).unwrap();
        assert!(preview(&p).is_none());
        std::fs::write(&p, []).unwrap();
        assert!(preview(&p).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
