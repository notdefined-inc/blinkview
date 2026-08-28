//! Downloading the ONNX models.
//!
//! The models are 37MB and not committed, so a fresh checkout cannot detect a face
//! until they are fetched. Without this command that step is a paragraph of README
//! instructions, which is the difference between software someone else can install and
//! software only its author can run.
//!
//! Downloads are verified against the SHA-256 of the exact files this project was
//! validated against (ADR-0003, ADR-0004). A model that differs silently invalidates
//! every threshold, so a mismatch is a hard failure rather than a warning.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct ModelSpec {
    pub name: &'static str,
    pub url: String,
    pub sha256: &'static str,
    pub bytes: u64,
}

/// `media.githubusercontent.com` rather than `raw.` — these are Git LFS objects, and
/// the raw endpoint returns a 133-byte pointer file that loads as a corrupt model.
const BASE: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models";
/// MobileCLIP-S0, for semantic search (ADR-0008). Separate host from the face models.
const CLIP_BASE: &str = "https://huggingface.co/Xenova/mobileclip_s0/resolve/main";

pub fn specs() -> Vec<ModelSpec> {
    vec![
        ModelSpec {
            name: super::models::YUNET,
            // The 2026may export: its spatial axes are symbolic, which the 2023mar one
            // is not. See ADR-0004.
            url: format!("{BASE}/face_detection_yunet/face_detection_yunet_2026may.onnx"),
            sha256: "ebafce4e3c118d6554634be5c27ab333b4c047a9a8c3faf1d7cf93101c22f0f0",
            bytes: 229_738,
        },
        ModelSpec {
            name: super::models::SFACE,
            url: format!("{BASE}/face_recognition_sface/face_recognition_sface_2021dec.onnx"),
            sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
            bytes: 38_696_353,
        },
        ModelSpec {
            name: crate::semantic::VISION,
            // fp32, not the 11MB int8 build: quantisation measurably compressed the
            // embedding space (mean pairwise 0.703 vs 0.566), and a compressed space
            // discriminates less. See ADR-0008.
            url: format!("{CLIP_BASE}/onnx/vision_model.onnx"),
            sha256: "17d3c037b1d488c10c50e09f6009ea5a198caef4e0e8f4ea5617b7cb2d067ac0",
            bytes: 45_543_630,
        },
        ModelSpec {
            name: crate::semantic::TEXT,
            // fp32, not the 40MB int8 build. Measured on 237 real photos against the
            // fp32 reference: int8 text vectors diverge by cosine 0.89-0.94, inflate
            // scores enough to double the matches clearing the relevance threshold
            // (149 vs 78), and rank a desk scene above the actual church for "a church".
            // It is also unstable across onnxruntime builds -- int8 in ort 1.22 differs
            // from int8 in Python 1.23 by up to cosine 0.989, which silently moves
            // photos across the threshold and makes ADR-0004 parity untestable.
            // fp16 is not an option: onnxruntime crashes fusing it. See ADR-0008.
            url: format!("{CLIP_BASE}/onnx/text_model.onnx"),
            sha256: "f6e9bd5742bfc515889e901634d8a2ff2a57fab8564e4ad3760e800b1a51b77c",
            bytes: 169_807_789,
        },
        ModelSpec {
            name: crate::semantic::TOKENIZER,
            url: format!("{CLIP_BASE}/tokenizer.json"),
            sha256: "72ed5c96db5729294468543e4bc75fce14ca63f58e37300290189ba1c1e52b85",
            bytes: 2_224_081,
        },
    ]
}

/// Where fetched models are written: the user cache, so several libraries share one
/// copy and deleting a library's `.openfoto` never forces a re-download.
pub fn cache_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".cache/openfoto/models"))
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// True when the file exists and its contents are the expected ones.
pub fn is_present(spec: &ModelSpec) -> bool {
    super::models::find(spec.name)
        .ok()
        .and_then(|p| sha256_of(&p).ok())
        .is_some_and(|h| h == spec.sha256)
}

/// Download one model. Reports (bytes_done, bytes_total).
pub fn fetch_one(spec: &ModelSpec, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<PathBuf> {
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(spec.name);
    // Download beside the target and rename, so an interrupted fetch never leaves a
    // truncated file that looks installed.
    let tmp = dir.join(format!("{}.part", spec.name));

    let mut resp = ureq::get(&spec.url)
        .call()
        .with_context(|| format!("requesting {}", spec.url))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(spec.bytes as usize);

    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(&tmp)?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0usize;
    progress(0, total);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n;
        progress(done.min(total), total);
    }
    file.flush()?;
    drop(file);

    let got = sha256_of(&tmp)?;
    if got != spec.sha256 {
        let _ = std::fs::remove_file(&tmp);
        bail!(
            "{} does not match the expected contents (sha256 {got}, expected {}). \
             Refusing to install it: a different model silently invalidates every \
             threshold openfoto relies on.",
            spec.name,
            spec.sha256
        );
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// Fetch every model that is missing or wrong. Returns the names actually downloaded.
pub fn fetch_missing(progress: &(dyn Fn(&str, usize, usize) + Sync)) -> Result<Vec<String>> {
    let mut got = Vec::new();
    for spec in specs() {
        if is_present(&spec) {
            continue;
        }
        let sink = |d: usize, t: usize| progress(spec.name, d, t);
        fetch_one(&spec, &sink)?;
        got.push(spec.name.to_string());
    }
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_never_fetch_lfs_pointers() {
        // raw.githubusercontent returns a 133-byte LFS pointer rather than the model,
        // which then fails to load as a corrupt file well after download.
        for s in specs() {
            assert!(
                !s.url.contains("raw.githubusercontent.com"),
                "{} would download an LFS pointer, not the model",
                s.name
            );
            assert!(
                s.url.starts_with(BASE) || s.url.starts_with(CLIP_BASE),
                "{} comes from an unpinned host: {}",
                s.name,
                s.url
            );
        }
    }

    #[test]
    fn specs_pin_a_sha256() {
        for s in specs() {
            assert_eq!(s.sha256.len(), 64, "{} needs a full sha256", s.name);
            assert!(s.bytes > 0);
        }
    }

    #[test]
    fn cache_dir_is_under_the_user_cache() {
        let d = cache_dir().unwrap();
        assert!(d.ends_with("openfoto/models"), "{}", d.display());
    }
}
