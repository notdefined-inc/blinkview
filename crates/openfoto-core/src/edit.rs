//! Rotating and cropping photos.
//!
//! This is the only part of openfoto that changes a photograph rather than moving it,
//! so it follows what Apple Photos, Google Photos and Samsung Gallery all converge on:
//! **the original is always recoverable**. Those apps can keep an edit list in a
//! database and render on demand; we deliberately have no database, and `.openfoto/` is
//! disposable (ADR-0001) — storing the only copy of an original there would mean
//! deleting a cache silently destroyed the user's photo.
//!
//! So the original moves to a visible `Originals/` folder, exactly as deleting moves a
//! photo to `Trash/`, and the edited image takes its place. Finder shows it, other
//! apps see an ordinary JPEG, and the journal makes it undoable.
//!
//! Destructive mode overwrites without keeping the original. It exists because a user
//! who has decided is entitled to decide, but it is never the default.

use crate::{imageio, Library};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const ORIGINALS: &str = "Originals";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Rotate {
    None,
    Cw90,
    Cw180,
    Cw270,
}

/// A crop in fractions of the image, so it survives the UI not knowing pixel sizes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Per-pixel tone adjustments, each neutral at 0.0.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct Adjust {
    /// -1.0 to 1.0. Added to every channel.
    #[serde(default)]
    pub brightness: f32,
    /// -1.0 to 1.0. Scales distance from mid-grey.
    #[serde(default)]
    pub contrast: f32,
    /// -1.0 to 1.0. Blends toward or away from luminance.
    #[serde(default)]
    pub saturation: f32,
}

/// Named starting points, defined here rather than in the window so the CLI and the
/// app cannot drift apart on what "warm" means.
///
/// Each is only a set of the three adjustments — there is no hidden fourth control —
/// so a preset can be nudged afterwards instead of being a mode you are stuck in.
pub const PRESETS: [(&str, Adjust); 5] = [
    // Saturation at -1.0 removes colour entirely; the small contrast lift is what
    // keeps a black-and-white from reading as a grey wash.
    ("Mono", Adjust { brightness: 0.0, contrast: 0.12, saturation: -1.0 }),
    ("Warm", Adjust { brightness: 0.04, contrast: 0.06, saturation: 0.18 }),
    ("Cool", Adjust { brightness: 0.02, contrast: 0.08, saturation: -0.12 }),
    ("Punch", Adjust { brightness: 0.0, contrast: 0.28, saturation: 0.3 }),
    ("Faded", Adjust { brightness: 0.08, contrast: -0.18, saturation: -0.22 }),
];

impl Adjust {
    /// The preset by name, if there is one. Case-insensitive: the name travels through
    /// a UI and a command line before it gets here.
    pub fn preset(name: &str) -> Option<Adjust> {
        PRESETS
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
            .map(|(_, a)| *a)
    }

    pub fn is_neutral(&self) -> bool {
        self.brightness == 0.0 && self.contrast == 0.0 && self.saturation == 0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    #[serde(default = "default_rotate")]
    pub rotate: Rotate,
    /// Fine rotation in degrees, for levelling a horizon. Applied after the coarse
    /// quarter-turn and auto-cropped so no blank corners survive.
    #[serde(default)]
    pub straighten: f32,
    #[serde(default)]
    pub adjust: Adjust,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    #[serde(default)]
    pub crop: Option<Crop>,
    /// Keep the original in `Originals/`. Default, and what the UI offers first.
    #[serde(default = "default_true")]
    pub keep_original: bool,
}

fn default_rotate() -> Rotate {
    Rotate::None
}
fn default_true() -> bool {
    true
}

impl Edit {
    pub fn is_noop(&self) -> bool {
        self.rotate == Rotate::None
            && self.crop.is_none()
            && !self.flip_h
            && !self.flip_v
            && self.straighten.abs() < 0.01
            && self.adjust.is_neutral()
    }
}

/// The largest centred rectangle **of the image's own aspect ratio** that fits inside
/// that image rotated by `angle` radians.
///
/// Straightening leaves blank wedges at the corners, and they have to go. The obvious
/// answer — the largest rectangle of *any* shape — changes the proportions of the
/// photograph, so a 3:4 portrait comes back some other shape. Apple Photos and Google
/// Photos both keep the original aspect and zoom in slightly instead, which is what a
/// person expects from "straighten", so that is what this computes.
///
/// A centred axis-aligned box of half-size (u, v) fits inside the rotated w x h
/// rectangle exactly when
///     u|cos| + v|sin| <= w/2   and   u|sin| + v|cos| <= h/2
/// and substituting u = a*v for the desired aspect `a` gives a bound on v from each.
fn inscribed_same_aspect(w: f32, h: f32, angle: f32) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    let (sin_a, cos_a) = (angle.sin().abs(), angle.cos().abs());
    let a = w / h;
    let v = (w / (2.0 * (a * cos_a + sin_a))).min(h / (2.0 * (a * sin_a + cos_a)));
    (2.0 * a * v, 2.0 * v)
}

/// How much the preview must zoom so no blank corner shows, for a given angle.
/// The frontend uses this so what is previewed is what gets saved.
pub fn straighten_zoom(w: f32, h: f32, degrees: f32) -> f32 {
    if w <= 0.0 || h <= 0.0 {
        return 1.0;
    }
    let (kw, _) = inscribed_same_aspect(w, h, degrees.to_radians());
    if kw <= 0.0 { 1.0 } else { w / kw }
}

/// Rotate by an arbitrary angle with bilinear sampling, then trim the blank corners.
fn straighten(img: &image::RgbImage, degrees: f32) -> image::RgbImage {
    let angle = degrees.to_radians();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (kw, kh) = inscribed_same_aspect(w, h, angle);
    let (ow, oh) = ((kw.floor().max(1.0)) as u32, (kh.floor().max(1.0)) as u32);

    let (sin_a, cos_a) = (angle.sin(), angle.cos());
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ocx, ocy) = (ow as f32 / 2.0, oh as f32 / 2.0);

    image::RgbImage::from_fn(ow, oh, |x, y| {
        // Map the output pixel back into the source and sample there.
        let (dx, dy) = (x as f32 - ocx, y as f32 - ocy);
        let sx = cx + dx * cos_a + dy * sin_a;
        let sy = cy - dx * sin_a + dy * cos_a;
        let (x0, y0) = (sx.floor(), sy.floor());
        let (tx, ty) = (sx - x0, sy - y0);
        let mut out = [0f32; 3];
        for (ddy, wy) in [(0.0, 1.0 - ty), (1.0, ty)] {
            for (ddx, wx) in [(0.0, 1.0 - tx), (1.0, tx)] {
                let px = (x0 + ddx).clamp(0.0, w - 1.0) as u32;
                let py = (y0 + ddy).clamp(0.0, h - 1.0) as u32;
                let p = img.get_pixel(px, py);
                for c in 0..3 {
                    out[c] += wx * wy * f32::from(p[c]);
                }
            }
        }
        image::Rgb([
            out[0].round().clamp(0.0, 255.0) as u8,
            out[1].round().clamp(0.0, 255.0) as u8,
            out[2].round().clamp(0.0, 255.0) as u8,
        ])
    })
}

/// Apply tone adjustments in place.
fn adjust(img: &mut image::RgbImage, a: &Adjust) {
    let bright = a.brightness.clamp(-1.0, 1.0) * 100.0;
    // Map -1..1 onto a multiplier that is gentle downward and strong upward.
    let contrast = (a.contrast.clamp(-1.0, 1.0) + 1.0).powi(2);
    let sat = a.saturation.clamp(-1.0, 1.0) + 1.0;
    for p in img.pixels_mut() {
        let mut c = [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])];
        for v in c.iter_mut() {
            *v = (*v - 128.0) * contrast + 128.0 + bright;
        }
        // Rec. 601 luma, so desaturating keeps perceived brightness.
        let luma = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        for v in c.iter_mut() {
            *v = luma + (*v - luma) * sat;
        }
        for i in 0..3 {
            p[i] = c[i].round().clamp(0.0, 255.0) as u8;
        }
    }
}

pub struct Applied {
    /// Where the untouched original ended up, when it was kept.
    pub original: Option<String>,
    pub width: u32,
    pub height: u32,
    /// The rewritten file's content hash. Everything the user authored is keyed by it
    /// (ADR-0007), so the caller has to carry ratings and labels across.
    pub hash: String,
}

/// Apply an edit to one photo, in place.
/// Move a photograph into the visible `Originals/` folder, returning where it went.
///
/// Shared with metadata stripping, which keeps the original for the same reason
/// editing does: the result cannot be turned back into what it was.
pub fn keep_original(lib: &Library, rel_path: &str) -> Result<Option<String>> {
    let dir = lib.abs(ORIGINALS);
    std::fs::create_dir_all(&dir)?;
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let mut dest = dir.join(name);
    // Never clobber an original already kept for a different photograph.
    let mut n = 2;
    while dest.exists() {
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, "jpg"));
        dest = dir.join(format!("{stem}_{n}.{ext}"));
        n += 1;
    }
    std::fs::rename(lib.abs(rel_path), &dest).with_context(|| "moving the original aside")?;
    Ok(lib.rel(&dest))
}

pub fn apply(lib: &Library, rel_path: &str, edit: &Edit) -> Result<Applied> {
    if edit.is_noop() {
        anyhow::bail!("nothing to apply");
    }
    let src = lib.abs(rel_path);
    let mut img = imageio::load_rgb(&src).with_context(|| format!("reading {rel_path}"))?;

    // Order matters and is not arbitrary: rotate and flip first, then crop. The user
    // draws the crop rectangle on the *transformed* preview, so its fractions are in
    // that space. Cropping first would apply their rectangle to the untransformed
    // image and cut the wrong region.
    img = match edit.rotate {
        Rotate::None => img,
        Rotate::Cw90 => image::imageops::rotate90(&img),
        Rotate::Cw180 => image::imageops::rotate180(&img),
        Rotate::Cw270 => image::imageops::rotate270(&img),
    };
    if edit.flip_h {
        img = image::imageops::flip_horizontal(&img);
    }
    if edit.flip_v {
        img = image::imageops::flip_vertical(&img);
    }
    // Fine rotation sits between the quarter-turn and the crop, so the crop rectangle
    // the user drew still refers to what they were looking at.
    if edit.straighten.abs() >= 0.01 {
        img = straighten(&img, edit.straighten);
    }
    if !edit.adjust.is_neutral() {
        adjust(&mut img, &edit.adjust);
    }
    if let Some(c) = edit.crop {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let x = (c.x.clamp(0.0, 1.0) * w) as u32;
        let y = (c.y.clamp(0.0, 1.0) * h) as u32;
        let cw = ((c.w.clamp(0.0, 1.0) * w) as u32).min(img.width().saturating_sub(x)).max(1);
        let ch = ((c.h.clamp(0.0, 1.0) * h) as u32).min(img.height().saturating_sub(y)).max(1);
        img = image::imageops::crop_imm(&img, x, y, cw, ch).to_image();
    }

    // Preserve the original before anything is overwritten.
    let original = if edit.keep_original { keep_original(lib, rel_path)? } else { None };

    // Write beside the target and swap, so a failure never leaves a truncated photo
    // where the original used to be.
    let tmp = src.with_extension("openfoto-tmp");
    let (width, height) = (img.width(), img.height());
    image::DynamicImage::ImageRgb8(img)
        .save_with_format(&tmp, image::ImageFormat::Jpeg)
        .with_context(|| "writing the edited image")?;
    std::fs::rename(&tmp, &src).with_context(|| "replacing the photo")?;

    Ok(Applied { original, width, height, hash: crate::scan::hash_file(&src)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window carries the same five presets, in the units its sliders use
    /// (-100..100 against the core's -1..1). Drift between the two would mean "Warm"
    /// did one thing in the editor and another in a batch, so the two lists are
    /// checked against each other rather than kept in step by hand.
    #[test]
    fn the_window_and_the_core_agree_on_what_a_preset_means() {
        let js = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/dist/app.js");
        let src = std::fs::read_to_string(&js)
            .unwrap_or_else(|e| panic!("reading {}: {e}", js.display()));
        let flat: String = src.chars().filter(|c| !c.is_whitespace()).collect();
        for (name, a) in PRESETS {
            let want = format!(
                "[\"{name}\",{{brightness:{},contrast:{},saturation:{}}}]",
                (a.brightness * 100.0).round() as i32,
                (a.contrast * 100.0).round() as i32,
                (a.saturation * 100.0).round() as i32,
            );
            assert!(flat.contains(&want), "app.js is missing or disagrees on {want}");
        }
    }

    #[test]
    fn a_preset_is_found_however_it_is_typed() {
        assert_eq!(Adjust::preset("mono"), Adjust::preset("Mono"));
        assert_eq!(Adjust::preset(" WARM "), Adjust::preset("Warm"));
        assert!(Adjust::preset("sepia").is_none());
        // Every preset must actually do something, or it is a button that lies.
        assert!(PRESETS.iter().all(|(_, a)| !a.is_neutral()));
    }

    fn edit(rotate: Rotate) -> Edit {
        Edit {
            rotate,
            straighten: 0.0,
            adjust: Adjust::default(),
            flip_h: false,
            flip_v: false,
            crop: None,
            keep_original: true,
        }
    }

    #[test]
    fn straightening_trims_the_blank_corners() {
        let img = image::RgbImage::from_pixel(400, 300, image::Rgb([200, 100, 50]));
        let out = straighten(&img, 5.0);
        // Smaller than the source, because the wedges are cut away...
        assert!(out.width() < 400 && out.height() < 300, "{}x{}", out.width(), out.height());
        // ...and no pixel is blank: every corner still carries image.
        for (x, y) in [(0, 0), (out.width() - 1, 0), (0, out.height() - 1),
                       (out.width() - 1, out.height() - 1)] {
            let p = out.get_pixel(x, y);
            assert!(p[0] > 150, "corner {x},{y} is blank: {p:?}");
        }
    }

    /// Straightening must not reshape the photograph. A 4:3 frame stays 4:3.
    #[test]
    fn straightening_preserves_the_aspect_ratio() {
        for angle in [1.0f32, 5.0, 12.0, -8.0] {
            let img = image::RgbImage::from_pixel(400, 300, image::Rgb([200, 100, 50]));
            let out = straighten(&img, angle);
            let before = 400.0 / 300.0;
            let after = out.width() as f32 / out.height() as f32;
            assert!((before - after).abs() < 0.02,
                "{angle} deg reshaped {before:.3} -> {after:.3}");
        }
    }

    /// The zoom the preview applies must match the trim the save performs, or the
    /// preview shows something the user will not get.
    #[test]
    fn preview_zoom_matches_the_saved_trim() {
        for angle in [2.0f32, 7.0, 15.0] {
            let img = image::RgbImage::from_pixel(400, 300, image::Rgb([9, 9, 9]));
            let out = straighten(&img, angle);
            let zoom = straighten_zoom(400.0, 300.0, angle);
            let implied = 400.0 / zoom;
            assert!((implied - out.width() as f32).abs() <= 1.5,
                "{angle} deg: preview implies {implied:.1}px wide, save gave {}", out.width());
        }
    }

    #[test]
    fn zero_angle_needs_no_zoom() {
        assert!((straighten_zoom(400.0, 300.0, 0.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_zero_angle_straighten_is_not_an_edit() {
        let mut e = edit(Rotate::None);
        e.straighten = 0.0;
        assert!(e.is_noop());
        e.straighten = 1.5;
        assert!(!e.is_noop());
    }

    #[test]
    fn adjustments_are_neutral_at_zero() {
        let mut img = image::RgbImage::from_pixel(4, 4, image::Rgb([120, 130, 140]));
        let before = img.clone();
        adjust(&mut img, &Adjust::default());
        assert_eq!(img.into_raw(), before.into_raw(), "a neutral adjust must change nothing");
    }

    #[test]
    fn brightness_and_saturation_move_the_right_way() {
        let mut up = image::RgbImage::from_pixel(2, 2, image::Rgb([100, 100, 100]));
        adjust(&mut up, &Adjust { brightness: 0.3, ..Default::default() });
        assert!(up.get_pixel(0, 0)[0] > 100);

        let mut grey = image::RgbImage::from_pixel(2, 2, image::Rgb([200, 40, 40]));
        adjust(&mut grey, &Adjust { saturation: -1.0, ..Default::default() });
        let p = grey.get_pixel(0, 0);
        assert!(p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1, "fully desaturated: {p:?}");
    }

    #[test]
    fn a_noop_edit_is_rejected() {
        assert!(edit(Rotate::None).is_noop());
    }

    #[test]
    fn a_flip_alone_is_an_edit() {
        let mut e = edit(Rotate::None);
        e.flip_h = true;
        assert!(!e.is_noop());
    }

    /// The crop the user drew is in the space they saw, so transforms come first.
    /// A 90-degree rotation swaps the axes; cropping first would cut the wrong region.
    #[test]
    fn crop_applies_after_rotation() {
        let dir = std::env::temp_dir().join(format!("of-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 40x10 landscape, so orientation is unambiguous after rotating.
        let img = image::RgbImage::from_fn(40, 10, |x, _| image::Rgb([(x * 6) as u8, 0, 0]));
        let path = dir.join("20260101_000000.jpg");
        img.save(&path).unwrap();

        let lib = crate::Library::open(&dir).unwrap();
        let mut e = edit(Rotate::Cw90);
        e.keep_original = false;
        // Left half of the rotated (10x40) image.
        e.crop = Some(Crop { x: 0.0, y: 0.0, w: 1.0, h: 0.5 });
        let out = apply(&lib, "20260101_000000.jpg", &e).unwrap();
        assert_eq!((out.width, out.height), (10, 20), "rotate then crop");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn keeping_the_original_is_the_default() {
        let e: Edit = serde_json::from_str(r#"{"rotate":"cw90"}"#).unwrap();
        assert!(e.keep_original, "safe editing must be the default");
        assert_eq!(e.rotate, Rotate::Cw90);
        assert!(e.crop.is_none());
    }

    #[test]
    fn destructive_must_be_asked_for_explicitly() {
        let e: Edit = serde_json::from_str(r#"{"rotate":"cw90","keep_original":false}"#).unwrap();
        assert!(!e.keep_original);
    }

    #[test]
    fn crop_fractions_are_clamped_not_trusted() {
        // The UI sends fractions; a bad one must not panic or read out of bounds.
        let c = Crop { x: -0.5, y: 2.0, w: 5.0, h: -1.0 };
        assert_eq!(c.x.clamp(0.0, 1.0), 0.0);
        assert_eq!(c.y.clamp(0.0, 1.0), 1.0);
        assert_eq!(c.w.clamp(0.0, 1.0), 1.0);
        assert_eq!(c.h.clamp(0.0, 1.0), 0.0);
    }
}
