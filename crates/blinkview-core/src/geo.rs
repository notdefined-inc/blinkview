//! Where a photograph was taken, and how to say it in words.
//!
//! Three jobs, one bundled table. Turning coordinates into "Santorini, South Aegean,
//! Greece" (reverse), turning a typed city into coordinates (forward), and reading and
//! writing the EXIF that carries them.
//!
//! **Nothing here touches the network.** The place table ships inside the binary, and
//! the map draws bundled outlines rather than fetching tiles — because a tile request
//! tells a server where the user has been, on every pan, which is the one thing a
//! local-first photo library must not do. See docs/SPECS/done/2026-08-30-places-and-the-map.md.
//!
//! Place data: GeoNames cities1000, CC BY 4.0, packed by `tools/build-geodata.sh`.

use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use serde::Serialize;
use std::path::Path;

/// The packed place table. 170k places in 3.8 MB; see `tools/build-geodata.py` for the
/// layout, which is little-endian throughout.
static PACKED: &[u8] = include_bytes!("../data/places.bin");

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Place {
    pub name: String,
    pub region: String,
    pub country: String,
    pub lat: f64,
    pub lon: f64,
}

impl Place {
    /// "Santorini, South Aegean, Greece", skipping the parts that are empty or that
    /// merely repeat the name — plenty of city-states and small countries would
    /// otherwise read "Singapore, Singapore, Singapore".
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for p in [
            self.name.as_str(),
            self.region.as_str(),
            self.country.as_str(),
        ] {
            if !p.is_empty() && !parts.contains(&p) {
                parts.push(p);
            }
        }
        parts.join(", ")
    }
}

struct Row {
    name: String,
    /// GeoNames' ASCII spelling, kept only when it differs. People type "Reykjavik",
    /// and the place is called "Reykjavík".
    ascii: String,
    lat: f32,
    lon: f32,
    country: u16,
    region: u16,
}

/// The table, plus a coarse grid so a lookup touches a few dozen rows rather than
/// 170,000. One-degree cells: at the equator that is ~111 km, so the search widens by
/// whole cells until it has looked at least as far as the best candidate so far.
struct Table {
    rows: Vec<Row>,
    countries: Vec<(String, String)>,
    regions: Vec<String>,
    grid: std::collections::HashMap<(i16, i16), Vec<u32>>,
}

fn cell(lat: f64, lon: f64) -> (i16, i16) {
    (lat.floor() as i16, lon.floor() as i16)
}

static TABLE: std::sync::LazyLock<Table> = std::sync::LazyLock::new(|| {
    parse(PACKED).unwrap_or_else(|e| panic!("the bundled place table is unreadable: {e}"))
});

fn parse(b: &[u8]) -> Result<Table> {
    let mut p = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        if p + n > b.len() {
            bail!("place table ends early");
        }
        let s = &b[p..p + n];
        p += n;
        Ok(s)
    };
    if take(7)? != b"OFGEO2\n" {
        bail!("not a place table");
    }
    let u32at = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize;

    let n = u32at(take(4)?);
    let mut countries = Vec::with_capacity(n);
    for _ in 0..n {
        let len = take(1)?[0] as usize;
        let code = String::from_utf8_lossy(take(len)?).to_string();
        let len = take(1)?[0] as usize;
        let name = String::from_utf8_lossy(take(len)?).to_string();
        countries.push((code, name));
    }

    let n = u32at(take(4)?);
    let mut regions = Vec::with_capacity(n);
    for _ in 0..n {
        let s = take(2)?;
        let len = u16::from_le_bytes([s[0], s[1]]) as usize;
        regions.push(String::from_utf8_lossy(take(len)?).to_string());
    }

    let n = u32at(take(4)?);
    let mut rows = Vec::with_capacity(n);
    let mut grid: std::collections::HashMap<(i16, i16), Vec<u32>> =
        std::collections::HashMap::new();
    for i in 0..n {
        let len = take(1)?[0] as usize;
        let name = String::from_utf8_lossy(take(len)?).to_string();
        let len = take(1)?[0] as usize;
        let ascii = String::from_utf8_lossy(take(len)?).to_string();
        let s = take(12)?;
        let lat = i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 1e4;
        let lon = i32::from_le_bytes([s[4], s[5], s[6], s[7]]) as f32 / 1e4;
        let country = u16::from_le_bytes([s[8], s[9]]);
        let region = u16::from_le_bytes([s[10], s[11]]);
        grid.entry(cell(lat as f64, lon as f64))
            .or_default()
            .push(i as u32);
        rows.push(Row {
            name,
            ascii,
            lat,
            lon,
            country,
            region,
        });
    }
    Ok(Table {
        rows,
        countries,
        regions,
        grid,
    })
}

fn place_at(t: &Table, i: usize) -> Place {
    let r = &t.rows[i];
    Place {
        name: r.name.clone(),
        region: t
            .regions
            .get(r.region as usize)
            .cloned()
            .unwrap_or_default(),
        country: t
            .countries
            .get(r.country as usize)
            .map(|(_, n)| n.clone())
            .unwrap_or_default(),
        lat: r.lat as f64,
        lon: r.lon as f64,
    }
}

/// Great-circle distance in kilometres.
pub fn haversine(a: (f64, f64), b: (f64, f64)) -> f64 {
    const R: f64 = 6371.0088;
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let (dlat, dlon) = (lat2 - lat1, lon2 - lon1);
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * h.sqrt().asin()
}

/// How far a named place may be before naming it would be a lie.
///
/// A photograph taken 150 km out at sea is not "in" the nearest town, and saying so is
/// worse than saying nothing — the whole point of the label is that someone recognises
/// where they were.
pub const MAX_KM: f64 = 120.0;

/// The nearest known place, or `None` when the nearest is too far to mean anything.
pub fn nearest(lat: f64, lon: f64) -> Option<Place> {
    let t = &*TABLE;
    let (clat, clon) = cell(lat, lon);
    let mut best: Option<(f64, usize)> = None;
    // Widen a ring of whole cells at a time, stopping once the ring's own inner edge is
    // further than the best candidate — anything beyond cannot beat it.
    for ring in 0..=3i16 {
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                if ring > 0 && dy.abs() != ring && dx.abs() != ring {
                    continue; // interior of the ring: already searched
                }
                let key = (clat + dy, wrap_lon_cell(clon + dx));
                let Some(ids) = t.grid.get(&key) else {
                    continue;
                };
                for &i in ids {
                    let r = &t.rows[i as usize];
                    let d = haversine((lat, lon), (r.lat as f64, r.lon as f64));
                    if best.is_none() || d < best.unwrap().0 {
                        best = Some((d, i as usize));
                    }
                }
            }
        }
        // One degree of latitude is ~111 km; a ring of n cells has searched at least
        // (n) degrees in every direction.
        if let Some((d, _)) = best {
            if d < (ring as f64) * 111.0 {
                break;
            }
        }
    }
    best.filter(|(d, _)| *d <= MAX_KM)
        .map(|(_, i)| place_at(t, i))
}

fn wrap_lon_cell(x: i16) -> i16 {
    if x < -180 {
        x + 360
    } else if x >= 180 {
        x - 360
    } else {
        x
    }
}

/// Places whose name matches `query`, most populous first.
///
/// The table is already ordered by population, so the first match is the place people
/// usually mean by a bare name — "Paris" is the French one.
pub fn search(query: &str, limit: usize) -> Vec<Place> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let t = &*TABLE;
    let mut exact = Vec::new();
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    for (i, r) in t.rows.iter().enumerate() {
        // Both spellings are searched, so an accent is never a reason not to find a
        // place: "Fira" must reach Firá before it reaches Firavitoba.
        let name = r.name.to_lowercase();
        let ascii = r.ascii.to_lowercase();
        let hit = |f: &dyn Fn(&str) -> bool| f(&name) || (!ascii.is_empty() && f(&ascii));
        let bucket = if hit(&|n: &str| n == q) {
            &mut exact
        } else if hit(&|n: &str| n.starts_with(&q)) {
            &mut starts
        } else if hit(&|n: &str| n.contains(&q)) {
            &mut contains
        } else {
            continue;
        };
        bucket.push(i);
        if exact.len() + starts.len() + contains.len() > limit * 8 {
            break;
        }
    }
    exact
        .into_iter()
        .chain(starts)
        .chain(contains)
        .take(limit)
        .map(|i| place_at(t, i))
        .collect()
}

// ---------------------------------------------------------------- EXIF GPS

/// The coordinates a photograph carries, if any.
///
/// Exactly `(0, 0)` is treated as absent. Cameras and phones write it when they have
/// no fix, and a photograph genuinely taken at Null Island — open Atlantic, 600 km off
/// Ghana — is vanishingly less likely than the bug.
pub fn read_gps(path: &Path) -> Option<(f64, f64)> {
    let file = std::fs::File::open(path).ok()?;
    let mut buf = std::io::BufReader::new(file);
    let ex = exif::Reader::new().read_from_container(&mut buf).ok()?;

    let dms = |tag| match &ex.get_field(tag, exif::In::PRIMARY)?.value {
        exif::Value::Rational(v) if v.len() >= 3 => {
            Some(v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0)
        }
        _ => None,
    };
    let refc = |tag| {
        ex.get_field(tag, exif::In::PRIMARY)
            .map(|f| f.display_value().to_string().trim_matches('"').to_string())
    };
    let lat = dms(exif::Tag::GPSLatitude)?;
    let lon = dms(exif::Tag::GPSLongitude)?;
    let lat = if refc(exif::Tag::GPSLatitudeRef)?.starts_with('S') {
        -lat
    } else {
        lat
    };
    let lon = if refc(exif::Tag::GPSLongitudeRef)?.starts_with('W') {
        -lon
    } else {
        lon
    };
    if lat == 0.0 && lon == 0.0 {
        return None;
    }
    (lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0)
        .then_some((lat, lon))
}

/// One TIFF directory entry, with its value already materialised.
///
/// Keeping the bytes rather than an offset is what makes re-serialising possible: every
/// offset in a TIFF is relative to the header, so anything that moves invalidates them,
/// and the only safe rewrite is to rebuild the whole block.
#[derive(Clone)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn type_size(typ: u16) -> usize {
    match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

const T_EXIF_IFD: u16 = 0x8769;
const T_GPS_IFD: u16 = 0x8825;
const T_INTEROP: u16 = 0xA005;
const T_THUMB_OFFSET: u16 = 0x0201;
const T_THUMB_LENGTH: u16 = 0x0202;

struct Tiff {
    big_endian: bool,
    ifd0: Vec<Entry>,
    exif: Vec<Entry>,
    gps: Vec<Entry>,
    interop: Vec<Entry>,
    ifd1: Vec<Entry>,
    thumbnail: Vec<u8>,
}

fn rd16(b: &[u8], at: usize, be: bool) -> Result<u16> {
    let s = b.get(at..at + 2).context("TIFF ends early")?;
    Ok(if be {
        u16::from_be_bytes([s[0], s[1]])
    } else {
        u16::from_le_bytes([s[0], s[1]])
    })
}

fn rd32(b: &[u8], at: usize, be: bool) -> Result<u32> {
    let s = b.get(at..at + 4).context("TIFF ends early")?;
    Ok(if be {
        u32::from_be_bytes([s[0], s[1], s[2], s[3]])
    } else {
        u32::from_le_bytes([s[0], s[1], s[2], s[3]])
    })
}

/// Read one IFD's entries, materialising every value.
fn read_ifd(b: &[u8], at: usize, be: bool) -> Result<Vec<Entry>> {
    let n = rd16(b, at, be)? as usize;
    // A corrupt count would otherwise walk off the end of the block.
    if at + 2 + n * 12 > b.len() {
        bail!("TIFF directory claims {n} entries it does not have");
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let e = at + 2 + i * 12;
        let tag = rd16(b, e, be)?;
        let typ = rd16(b, e + 2, be)?;
        let count = rd32(b, e + 4, be)?;
        let len = type_size(typ).saturating_mul(count as usize);
        let data = if len <= 4 {
            b[e + 8..e + 8 + len.min(4)].to_vec()
        } else {
            let off = rd32(b, e + 8, be)? as usize;
            match b.get(off..off + len) {
                Some(s) => s.to_vec(),
                // A value pointing outside the block is not something to guess at.
                None => bail!("TIFF value for tag {tag:#06x} points outside the block"),
            }
        };
        out.push(Entry {
            tag,
            typ,
            count,
            data,
        });
    }
    Ok(out)
}

fn sub_ifd(b: &[u8], entries: &[Entry], tag: u16, be: bool) -> Vec<Entry> {
    let Some(e) = entries.iter().find(|e| e.tag == tag) else {
        return Vec::new();
    };
    if e.data.len() < 4 {
        return Vec::new();
    }
    let off = if be {
        u32::from_be_bytes([e.data[0], e.data[1], e.data[2], e.data[3]])
    } else {
        u32::from_le_bytes([e.data[0], e.data[1], e.data[2], e.data[3]])
    } as usize;
    read_ifd(b, off, be).unwrap_or_default()
}

fn parse_tiff(b: &[u8]) -> Result<Tiff> {
    if b.len() < 8 {
        bail!("TIFF block too small");
    }
    let be = match &b[0..2] {
        b"MM" => true,
        b"II" => false,
        _ => bail!("not a TIFF block"),
    };
    let ifd0_at = rd32(b, 4, be)? as usize;
    let ifd0 = read_ifd(b, ifd0_at, be)?;
    let exif = sub_ifd(b, &ifd0, T_EXIF_IFD, be);
    let gps = sub_ifd(b, &ifd0, T_GPS_IFD, be);
    let interop = sub_ifd(b, &exif, T_INTEROP, be);

    // IFD1 holds the camera's embedded thumbnail, which the thumbnailer uses as a fast
    // path instead of decoding the full image — worth carrying across.
    let next = rd32(b, ifd0_at + 2 + ifd0.len() * 12, be).unwrap_or(0) as usize;
    let (mut ifd1, mut thumbnail) = (Vec::new(), Vec::new());
    if next != 0 && next < b.len() {
        if let Ok(entries) = read_ifd(b, next, be) {
            let get = |tag: u16| -> Option<u32> {
                let e = entries.iter().find(|e| e.tag == tag)?;
                (e.data.len() >= 4).then(|| {
                    if be {
                        u32::from_be_bytes([e.data[0], e.data[1], e.data[2], e.data[3]])
                    } else {
                        u32::from_le_bytes([e.data[0], e.data[1], e.data[2], e.data[3]])
                    }
                })
            };
            if let (Some(off), Some(len)) = (get(T_THUMB_OFFSET), get(T_THUMB_LENGTH)) {
                if let Some(s) = b.get(off as usize..(off + len) as usize) {
                    thumbnail = s.to_vec();
                }
            }
            ifd1 = entries;
        }
    }
    Ok(Tiff {
        big_endian: be,
        ifd0,
        exif,
        gps,
        interop,
        ifd1,
        thumbnail,
    })
}

fn u16b(v: u16, be: bool) -> [u8; 2] {
    if be {
        v.to_be_bytes()
    } else {
        v.to_le_bytes()
    }
}
fn u32b(v: u32, be: bool) -> [u8; 4] {
    if be {
        v.to_be_bytes()
    } else {
        v.to_le_bytes()
    }
}

/// Serialise a directory, appending oversized values to `heap`.
///
/// `base` is where the heap will sit once everything is concatenated, which is the only
/// way to know an offset before the bytes exist.
fn write_ifd(entries: &[Entry], be: bool, heap: &mut Vec<u8>, heap_base: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + entries.len() * 12 + 4);
    out.extend_from_slice(&u16b(entries.len() as u16, be));
    for e in entries {
        out.extend_from_slice(&u16b(e.tag, be));
        out.extend_from_slice(&u16b(e.typ, be));
        out.extend_from_slice(&u32b(e.count, be));
        if e.data.len() <= 4 {
            let mut v = e.data.clone();
            v.resize(4, 0);
            out.extend_from_slice(&v);
        } else {
            out.extend_from_slice(&u32b((heap_base + heap.len()) as u32, be));
            heap.extend_from_slice(&e.data);
            if heap.len() % 2 == 1 {
                heap.push(0); // TIFF values are word-aligned
            }
        }
    }
    out
}

fn rational(be: bool, num: u32, den: u32) -> Vec<u8> {
    let mut v = u32b(num, be).to_vec();
    v.extend_from_slice(&u32b(den, be));
    v
}

/// The GPS directory for one coordinate.
fn gps_entries(lat: f64, lon: f64, be: bool) -> Vec<Entry> {
    let dms = |v: f64| {
        let v = v.abs();
        let d = v.floor();
        let m = ((v - d) * 60.0).floor();
        let s = (v - d - m / 60.0) * 3600.0;
        let mut out = rational(be, d as u32, 1);
        out.extend(rational(be, m as u32, 1));
        // Seconds to four decimals: about 3 mm, far past what any camera records.
        out.extend(rational(be, (s * 10_000.0).round() as u32, 10_000));
        out
    };
    vec![
        Entry {
            tag: 0x0000,
            typ: 1,
            count: 4,
            data: vec![2, 3, 0, 0],
        },
        Entry {
            tag: 0x0001,
            typ: 2,
            count: 2,
            data: if lat < 0.0 {
                b"S\0".to_vec()
            } else {
                b"N\0".to_vec()
            },
        },
        Entry {
            tag: 0x0002,
            typ: 5,
            count: 3,
            data: dms(lat),
        },
        Entry {
            tag: 0x0003,
            typ: 2,
            count: 2,
            data: if lon < 0.0 {
                b"W\0".to_vec()
            } else {
                b"E\0".to_vec()
            },
        },
        Entry {
            tag: 0x0004,
            typ: 5,
            count: 3,
            data: dms(lon),
        },
    ]
}

fn set_ascii(entries: &mut Vec<Entry>, tag: u16, value: &str) {
    let mut data = value.as_bytes().to_vec();
    data.push(0);
    let entry = Entry {
        tag,
        typ: 2,
        count: data.len() as u32,
        data,
    };
    if let Some(old) = entries.iter_mut().find(|e| e.tag == tag) {
        *old = entry;
    } else {
        entries.push(entry);
    }
}

/// Rebuild a TIFF block, optionally replacing its GPS directory and capture time.
/// Supplying neither preserves the parsed values byte-for-byte.
fn rebuild_tiff(t: &Tiff, replacement_gps: Option<Vec<Entry>>, datetime: Option<&str>) -> Vec<u8> {
    let be = t.big_endian;
    let mut ifd0: Vec<Entry> = t
        .ifd0
        .iter()
        .filter(|e| e.tag != T_EXIF_IFD && e.tag != T_GPS_IFD)
        .cloned()
        .collect();
    let mut exif: Vec<Entry> = t
        .exif
        .iter()
        .filter(|e| e.tag != T_INTEROP)
        .cloned()
        .collect();
    let gps = replacement_gps.unwrap_or_else(|| t.gps.clone());
    if let Some(value) = datetime {
        // IFD0 DateTime, EXIF DateTimeOriginal and DateTimeDigitized. Writing all
        // three makes the correction agree in old cataloguers as well as cameras.
        set_ascii(&mut ifd0, 0x0132, value);
        set_ascii(&mut exif, 0x9003, value);
        set_ascii(&mut exif, 0x9004, value);
    }
    // TIFF requires ascending tags within a directory, and some readers rely on it.
    ifd0.sort_by_key(|e| e.tag);
    exif.sort_by_key(|e| e.tag);

    // Two passes: lay everything out with placeholder pointers to learn the sizes, then
    // emit again now that the offsets are known.
    let pointer = |tag: u16, be: bool| Entry {
        tag,
        typ: 4,
        count: 1,
        data: u32b(0, be).to_vec(),
    };
    let mut ifd0_p = ifd0.clone();
    ifd0_p.push(pointer(T_EXIF_IFD, be));
    if !gps.is_empty() {
        ifd0_p.push(pointer(T_GPS_IFD, be));
    }
    ifd0_p.sort_by_key(|e| e.tag);
    let mut exif_p = exif.clone();
    if !t.interop.is_empty() {
        exif_p.push(pointer(T_INTEROP, be));
        exif_p.sort_by_key(|e| e.tag);
    }

    let sz = |n: usize| 2 + n * 12 + 4;
    let ifd0_at = 8usize;
    let exif_at = ifd0_at + sz(ifd0_p.len());
    let gps_at = exif_at + sz(exif_p.len());
    let interop_at = gps_at + if gps.is_empty() { 0 } else { sz(gps.len()) };
    let ifd1_at = interop_at
        + if t.interop.is_empty() {
            0
        } else {
            sz(t.interop.len())
        };
    let heap_base = ifd1_at
        + if t.ifd1.is_empty() {
            0
        } else {
            sz(t.ifd1.len())
        };

    // Now with real pointers.
    let ptr = |tag: u16, at: usize| Entry {
        tag,
        typ: 4,
        count: 1,
        data: u32b(at as u32, be).to_vec(),
    };
    let mut ifd0_f = ifd0;
    ifd0_f.push(ptr(T_EXIF_IFD, exif_at));
    if !gps.is_empty() {
        ifd0_f.push(ptr(T_GPS_IFD, gps_at));
    }
    ifd0_f.sort_by_key(|e| e.tag);
    let mut exif_f = exif;
    if !t.interop.is_empty() {
        exif_f.push(ptr(T_INTEROP, interop_at));
        exif_f.sort_by_key(|e| e.tag);
    }

    let mut heap = Vec::new();
    let b_ifd0 = write_ifd(&ifd0_f, be, &mut heap, heap_base);
    let b_exif = write_ifd(&exif_f, be, &mut heap, heap_base);
    let b_gps = if gps.is_empty() {
        Vec::new()
    } else {
        write_ifd(&gps, be, &mut heap, heap_base)
    };
    let b_interop = if t.interop.is_empty() {
        Vec::new()
    } else {
        write_ifd(&t.interop, be, &mut heap, heap_base)
    };

    // The thumbnail's own offset lives in IFD1 and has to point at wherever it lands.
    let mut ifd1 = t.ifd1.clone();
    if !t.thumbnail.is_empty() {
        let thumb_at = heap_base + heap.len();
        for e in ifd1.iter_mut() {
            if e.tag == T_THUMB_OFFSET {
                e.data = u32b(thumb_at as u32, be).to_vec();
            }
        }
    }
    let b_ifd1 = if ifd1.is_empty() {
        Vec::new()
    } else {
        write_ifd(&ifd1, be, &mut heap, heap_base)
    };
    if !t.thumbnail.is_empty() {
        heap.extend_from_slice(&t.thumbnail);
    }

    let mut out = Vec::with_capacity(heap_base + heap.len());
    out.extend_from_slice(if be { b"MM" } else { b"II" });
    out.extend_from_slice(&u16b(42, be));
    out.extend_from_slice(&u32b(ifd0_at as u32, be));
    let mut push = |bytes: &[u8], next: u32| {
        if bytes.is_empty() {
            return;
        }
        out.extend_from_slice(&bytes[..bytes.len()]);
        out.extend_from_slice(&u32b(next, be));
    };
    push(&b_ifd0, if b_ifd1.is_empty() { 0 } else { ifd1_at as u32 });
    push(&b_exif, 0);
    push(&b_gps, 0);
    push(&b_interop, 0);
    push(&b_ifd1, 0);
    out.extend_from_slice(&heap);
    out
}

/// A fresh EXIF block for a photograph that carried none.
fn fresh_tiff(gps: Option<Vec<Entry>>, datetime: Option<&str>) -> Vec<u8> {
    let t = Tiff {
        big_endian: false,
        ifd0: Vec::new(),
        exif: Vec::new(),
        gps: Vec::new(),
        interop: Vec::new(),
        ifd1: Vec::new(),
        thumbnail: Vec::new(),
    };
    rebuild_tiff(&t, gps, datetime)
}

/// Put coordinates into a JPEG, keeping everything else its EXIF held.
///
/// The rewritten file is **read back and re-parsed before it replaces the original**,
/// and if the coordinates just written cannot be found in it, the original is left
/// exactly as it was. Rebuilding a TIFF is the one operation here that could corrupt a
/// photograph, so it is not trusted — it is checked.
pub fn write_gps(path: &Path, lat: f64, lon: f64) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg") {
        bail!(
            "blinkview can only write a location into JPEG files, not {}",
            if ext.is_empty() {
                "this".into()
            } else {
                ext.to_uppercase()
            }
        );
    }
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0) {
        bail!("{lat}, {lon} is not a place on earth");
    }
    // Refused here rather than discovered at the read-back below: `read_gps` treats
    // exactly 0,0 as no fix, so writing it would produce a file blinkview itself reads
    // as having no location. Refusing to write what we cannot read keeps the two
    // halves of this module honest with each other.
    if lat == 0.0 && lon == 0.0 {
        bail!(
            "0, 0 is what a camera writes when it has no fix, so blinkview reads it as no location"
        );
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let tiff = match find_app1(&bytes) {
        Some((_, _, payload)) => {
            let parsed = parse_tiff(payload)?;
            let gps = gps_entries(lat, lon, parsed.big_endian);
            rebuild_tiff(&parsed, Some(gps), None)
        }
        None => fresh_tiff(Some(gps_entries(lat, lon, false)), None),
    };
    let mut app1 = Vec::with_capacity(tiff.len() + 10);
    app1.extend_from_slice(&[0xFF, 0xE1]);
    let len = tiff.len() + 6 + 2;
    if len > u16::MAX as usize {
        bail!("this photograph's metadata is too large to rewrite");
    }
    app1.extend_from_slice(&(len as u16).to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);

    let out = splice_app1(&bytes, &app1)?;

    // Prove it before believing it.
    let tmp = path.with_extension("blinkview-gps-tmp");
    std::fs::write(&tmp, &out)?;
    match read_gps(&tmp) {
        Some((a, b)) if (a - lat).abs() < 0.001 && (b - lon).abs() < 0.001 => {
            std::fs::rename(&tmp, path).context("replacing the photograph")?;
            Ok(())
        }
        other => {
            let _ = std::fs::remove_file(&tmp);
            bail!("the rewritten file did not read back as {lat}, {lon} (got {other:?}) — {} was left alone",
                  path.display())
        }
    }
}

/// Put a corrected capture date into a JPEG, preserving all other EXIF fields.
///
/// As with [`write_gps`], the rewrite is parsed from a temporary file before it may
/// replace the original. The caller supplies a timezone-free camera wall clock because
/// the EXIF 2.x date fields themselves do not carry a timezone.
pub fn write_datetime(path: &Path, datetime: NaiveDateTime) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg") {
        bail!(
            "blinkview can only write a capture time into JPEG files, not {}",
            if ext.is_empty() {
                "this".into()
            } else {
                ext.to_uppercase()
            }
        );
    }

    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let value = datetime.format("%Y:%m:%d %H:%M:%S").to_string();
    let tiff = match find_app1(&bytes) {
        Some((_, _, payload)) => rebuild_tiff(&parse_tiff(payload)?, None, Some(&value)),
        None => fresh_tiff(None, Some(&value)),
    };
    let mut app1 = Vec::with_capacity(tiff.len() + 10);
    app1.extend_from_slice(&[0xFF, 0xE1]);
    let len = tiff.len() + 6 + 2;
    if len > u16::MAX as usize {
        bail!("this photograph's metadata is too large to rewrite");
    }
    app1.extend_from_slice(&(len as u16).to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let out = splice_app1(&bytes, &app1)?;

    let tmp = path.with_extension("blinkview-time-tmp");
    std::fs::write(&tmp, &out)?;
    match crate::timesource::from_exif(&tmp) {
        Some(got) if got == datetime => {
            std::fs::rename(&tmp, path).context("replacing the photograph")?;
            Ok(())
        }
        other => {
            let _ = std::fs::remove_file(&tmp);
            bail!(
                "the rewritten file did not read back as {} (got {other:?}) — {} was left alone",
                datetime.format("%Y-%m-%d %H:%M:%S"),
                path.display()
            )
        }
    }
}

/// The APP1 EXIF segment: (start, end, the TIFF payload inside it).
fn find_app1(b: &[u8]) -> Option<(usize, usize, &[u8])> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 3 < b.len() && b[i] == 0xFF {
        let marker = b[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if len < 2 || i + 2 + len > b.len() {
            return None;
        }
        if marker == 0xE1 && b.get(i + 4..i + 10) == Some(b"Exif\0\0") {
            return Some((i, i + 2 + len, &b[i + 10..i + 2 + len]));
        }
        i += 2 + len;
    }
    None
}

/// Replace the EXIF segment, or insert one straight after SOI.
fn splice_app1(b: &[u8], app1: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(b.len() + app1.len());
    match find_app1(b) {
        Some((start, end, _)) => {
            out.extend_from_slice(&b[..start]);
            out.extend_from_slice(app1);
            out.extend_from_slice(&b[end..]);
        }
        None => {
            if b.len() < 2 || b[0] != 0xFF || b[1] != 0xD8 {
                bail!("not a JPEG");
            }
            out.extend_from_slice(&b[..2]);
            out.extend_from_slice(app1);
            out.extend_from_slice(&b[2..]);
        }
    }
    Ok(out)
}

/// What one pass over the library found.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct Located {
    /// Photographs looked at this time. Zero on a second run, which is the point.
    pub checked: usize,
    /// Of those, how many carried coordinates.
    pub found: usize,
}

/// Read coordinates out of every photograph that has not been looked at yet.
///
/// Incremental by construction: the answer is cached against the content hash,
/// including the answer "none", so a second run over an unchanged library does nothing
/// at all. Reading GPS is a header parse rather than a decode — cheap enough to do on
/// demand when the map opens, which is why it is not part of the analysis pass.
pub fn locate(
    lib: &mut crate::Library,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Located> {
    let rows: Vec<_> = lib
        .index
        .all()?
        .into_iter()
        .filter(|r| r.kind == "photo")
        .collect();
    let seen = lib.index.gps_checked()?;
    let todo: Vec<_> = rows
        .into_iter()
        .filter(|r| !seen.contains(&r.hash))
        .collect();

    let counter = crate::progress::Counter::new(todo.len(), progress);
    let mut out = Located {
        checked: 0,
        found: 0,
    };
    for r in &todo {
        counter.tick();
        let at = read_gps(&lib.abs(&r.path));
        if at.is_some() {
            out.found += 1;
        }
        out.checked += 1;
        lib.index.set_gps(&r.hash, at)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real JPEG, written by the `image` crate, with no EXIF at all.
    fn jpeg(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let img = image::RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([(x * 5) as u8, (y * 7) as u8, 128])
        });
        let p = dir.join(name);
        img.save(&p).unwrap();
        p
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("of-geo-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_photograph_with_no_exif_can_be_given_a_location() {
        let d = tmp("fresh");
        let p = jpeg(&d, "a.jpg");
        assert!(
            read_gps(&p).is_none(),
            "the fixture starts with no coordinates"
        );

        write_gps(&p, 36.3932, 25.4615).unwrap(); // Santorini
        let (lat, lon) = read_gps(&p).expect("coordinates after writing");
        assert!((lat - 36.3932).abs() < 0.001, "{lat}");
        assert!((lon - 25.4615).abs() < 0.001, "{lon}");
        // And it still decodes as an image afterwards.
        let img = image::open(&p).unwrap();
        assert_eq!((img.width(), img.height()), (48, 32));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_photograph_with_no_exif_can_be_given_a_capture_time() {
        let d = tmp("fresh-time");
        let p = jpeg(&d, "a.jpg");
        let wanted =
            NaiveDateTime::parse_from_str("2026-08-19 14:03:27", "%Y-%m-%d %H:%M:%S").unwrap();

        write_datetime(&p, wanted).unwrap();

        assert_eq!(crate::timesource::from_exif(&p), Some(wanted));
        let img = image::open(&p).unwrap();
        assert_eq!((img.width(), img.height()), (48, 32));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn changing_capture_time_preserves_the_location() {
        let d = tmp("time-keeps-gps");
        let p = jpeg(&d, "a.jpg");
        write_gps(&p, 36.3932, 25.4615).unwrap();
        let wanted =
            NaiveDateTime::parse_from_str("2026-08-20 09:45:00", "%Y-%m-%d %H:%M:%S").unwrap();

        write_datetime(&p, wanted).unwrap();

        assert_eq!(crate::timesource::from_exif(&p), Some(wanted));
        let (lat, lon) = read_gps(&p).expect("location survives the date rewrite");
        assert!((lat - 36.3932).abs() < 0.001 && (lon - 25.4615).abs() < 0.001);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_southern_and_western_hemispheres_survive_the_round_trip() {
        let d = tmp("hemis");
        for (name, lat, lon) in [
            ("sw.jpg", -33.8688, -151.2093),
            ("se.jpg", -33.8688, 151.2093),
            ("nw.jpg", 40.7580, -73.9855),
        ] {
            let p = jpeg(&d, name);
            write_gps(&p, lat, lon).unwrap();
            let (a, b) = read_gps(&p).expect(name);
            assert!(
                (a - lat).abs() < 0.001 && (b - lon).abs() < 0.001,
                "{name}: {a},{b}"
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writing_a_location_keeps_the_rest_of_the_exif() {
        let d = tmp("keep");
        let p = jpeg(&d, "a.jpg");
        // Give it an EXIF block first, the way a camera would, then add coordinates.
        write_gps(&p, 10.0, 10.0).unwrap();
        let before = std::fs::read(&p).unwrap();
        write_gps(&p, 51.5074, -0.1278).unwrap(); // London
        let (lat, lon) = read_gps(&p).unwrap();
        assert!((lat - 51.5074).abs() < 0.001 && (lon - -0.1278).abs() < 0.001);
        // Replacing coordinates must not grow the file without bound.
        assert!(std::fs::read(&p).unwrap().len() < before.len() + 256);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_format_that_cannot_carry_a_location_is_refused_by_name() {
        let d = tmp("refuse");
        let p = d.join("clip.mov");
        std::fs::write(&p, b"not a jpeg").unwrap();
        let e = write_gps(&p, 1.0, 1.0).unwrap_err().to_string();
        assert!(e.contains("MOV"), "{e}");
        let h = d.join("shot.heic");
        std::fs::write(&h, b"nope").unwrap();
        assert!(write_gps(&h, 1.0, 1.0).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_coordinate_that_is_not_on_earth_is_refused() {
        let d = tmp("range");
        let p = jpeg(&d, "a.jpg");
        assert!(write_gps(&p, 91.0, 0.0).is_err());
        assert!(write_gps(&p, 0.0, 181.0).is_err());
        assert!(write_gps(&p, f64::NAN, 0.0).is_err());
        // None of that may have touched the file.
        assert!(read_gps(&p).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn null_island_is_refused_because_it_could_not_be_read_back() {
        let d = tmp("null");
        let p = jpeg(&d, "a.jpg");
        // `read_gps` treats exactly 0,0 as "no fix", so writing it would make a file
        // blinkview itself reads as unlocated. The two halves must agree.
        let e = write_gps(&p, 0.0, 0.0).unwrap_err().to_string();
        assert!(e.contains("no fix"), "{e}");
        assert!(read_gps(&p).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_rewrite_that_cannot_be_read_back_leaves_the_original_alone() {
        let d = tmp("guard");
        let p = jpeg(&d, "a.jpg");
        write_gps(&p, 48.8584, 2.2945).unwrap();
        let good = std::fs::read(&p).unwrap();
        // Clobber the TIFF byte-order mark inside the EXIF segment, which is the one
        // thing `parse_tiff` cannot recover from. The write must refuse and leave what
        // was there rather than writing half a file.
        let mut broken = good.clone();
        let at = broken
            .windows(6)
            .position(|w| w == b"Exif\0\0")
            .expect("the fixture has an EXIF segment");
        broken[at + 6] = 0x00;
        broken[at + 7] = 0x00;
        let q = d.join("broken.jpg");
        std::fs::write(&q, &broken).unwrap();
        let before = std::fs::read(&q).unwrap();
        let _ = write_gps(&q, 1.0, 1.0);
        assert_eq!(
            std::fs::read(&q).unwrap(),
            before,
            "a failed write must change nothing"
        );
        // No temporary file is left behind either.
        assert!(!d.join("broken.blinkview-gps-tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_bundled_table_loads() {
        let t = &*TABLE;
        assert!(t.rows.len() > 100_000, "{} places", t.rows.len());
        assert!(t.countries.len() > 200);
        assert!(!t.grid.is_empty());
    }

    #[test]
    fn coordinates_resolve_to_somewhere_recognisable() {
        // The Acropolis.
        let p = nearest(37.9715, 23.7257).expect("Athens");
        assert_eq!(p.name, "Athens", "{p:?}");
        assert_eq!(p.country, "Greece");
        assert!(p.label().contains("Greece"), "{}", p.label());

        // Times Square, and a country whose name is not its own region.
        let p = nearest(40.7580, -73.9855).expect("New York");
        assert_eq!(p.country, "United States");
    }

    #[test]
    fn the_middle_of_an_ocean_is_not_a_city() {
        // Point Nemo, the furthest place on earth from land.
        assert!(nearest(-48.876, -123.393).is_none());
    }

    #[test]
    fn a_label_does_not_repeat_itself() {
        let p = Place {
            name: "Singapore".into(),
            region: "Singapore".into(),
            country: "Singapore".into(),
            lat: 0.0,
            lon: 0.0,
        };
        assert_eq!(p.label(), "Singapore");
        let p = Place {
            name: "Fira".into(),
            region: "South Aegean".into(),
            country: "Greece".into(),
            lat: 0.0,
            lon: 0.0,
        };
        assert_eq!(p.label(), "Fira, South Aegean, Greece");
    }

    #[test]
    fn searching_offers_the_place_people_mean_first() {
        let hits = search("paris", 5);
        assert_eq!(hits[0].name, "Paris");
        assert_eq!(
            hits[0].country, "France",
            "the biggest Paris is the French one"
        );
        assert!(search("zzzzzznowhere", 5).is_empty());
        assert!(search("  ", 5).is_empty());
    }

    #[test]
    fn an_accent_is_never_a_reason_not_to_find_a_place() {
        // Nobody types the accent. Before the ASCII spelling was carried in the table,
        // "fira" reached Firavitoba in Colombia before Firá on Santorini.
        let hits = search("fira", 5);
        assert_eq!(
            hits[0].country,
            "Greece",
            "{:?}",
            hits.iter().map(|h| h.label()).collect::<Vec<_>>()
        );

        let hits = search("reykjavik", 3);
        assert_eq!(hits[0].country, "Iceland", "{:?}", hits[0]);
        // The accented spelling still works, since that is what the table displays.
        assert_eq!(search("reykjav\u{ed}k", 3)[0].country, "Iceland");
    }

    #[test]
    fn a_lookup_near_a_meridian_still_finds_its_neighbours() {
        // Suva sits just west of the 180th meridian; the grid must wrap rather than
        // treating the antimeridian as the edge of the world.
        let p = nearest(-18.1416, 178.4419).expect("Suva");
        assert_eq!(p.country, "Fiji", "{p:?}");
    }
}
