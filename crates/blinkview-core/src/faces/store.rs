//! Persisting detected faces alongside the photo index.

use crate::{faces::embed::DIM, Library};
use anyhow::{Context, Result};
use rusqlite::params;

#[derive(Debug, Clone)]
pub struct StoredFace {
    pub hash: String,
    pub idx: i64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// Face width as a fraction of image width. Scale-invariant, so it stays
    /// meaningful regardless of the resolution analysis ran at.
    pub ratio: f32,
    pub embedding: Option<Vec<f32>>,
}

pub(crate) fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Option<Vec<f32>> {
    if b.len() != DIM * 4 {
        return None;
    }
    Some(b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

impl Library {
    pub fn put_face(&self, f: &StoredFace) -> Result<()> {
        self.index.conn().execute(
            "INSERT OR REPLACE INTO faces (hash,idx,x,y,w,h,score,ratio,embedding)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                f.hash, f.idx, f.x, f.y, f.w, f.h, f.score, f.ratio,
                f.embedding.as_deref().map(to_blob)
            ],
        )?;
        Ok(())
    }

    pub fn mark_faces_done(&self, hash: &str) -> Result<()> {
        self.index
            .conn()
            .execute("INSERT OR IGNORE INTO faces_done (hash) VALUES (?1)", params![hash])?;
        Ok(())
    }

    pub fn faces_done(&self, hash: &str) -> Result<bool> {
        Ok(self.index.conn().query_row(
            "SELECT 1 FROM faces_done WHERE hash=?1",
            params![hash],
            |_| Ok(()),
        ).is_ok())
    }

    pub fn all_faces(&self) -> Result<Vec<StoredFace>> {
        let conn = self.index.conn();
        let mut st = conn.prepare(
            "SELECT hash,idx,x,y,w,h,score,ratio,embedding FROM faces ORDER BY hash, idx",
        )?;
        let rows = st.query_map([], |r| {
            let blob: Option<Vec<u8>> = r.get(8)?;
            Ok(StoredFace {
                hash: r.get(0)?,
                idx: r.get(1)?,
                x: r.get(2)?,
                y: r.get(3)?,
                w: r.get(4)?,
                h: r.get(5)?,
                score: r.get(6)?,
                ratio: r.get(7)?,
                embedding: blob.as_deref().and_then(from_blob),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The embedding one face pointer names, when that face is still in the index.
    pub fn face_embedding(&self, hash: &str, idx: i64) -> Result<Option<Vec<f32>>> {
        // No row is a normal answer — the face is gone — so QueryReturnedNoRows is
        // flattened into None rather than propagated as an error.
        let blob: Option<Vec<u8>> = self
            .index
            .conn()
            .query_row(
                "SELECT embedding FROM faces WHERE hash=?1 AND idx=?2",
                params![hash, idx],
                |r| r.get(0),
            )
            .ok();
        Ok(blob.as_deref().and_then(from_blob))
    }

    /// Every stored face as (its embedding bytes, its `"<hash>:<idx>"` key).
    ///
    /// The table behind turning vectors into pointers on save: a vector that names a
    /// face becomes the pointer, and only one that matches nothing is kept as bytes.
    pub(crate) fn face_blobs(&self) -> Result<Vec<(Vec<u8>, String)>> {
        Ok(self
            .all_faces()?
            .into_iter()
            .filter_map(|f| {
                f.embedding
                    .as_ref()
                    .map(|e| (to_blob(e), format!("{}:{}", f.hash, f.idx)))
            })
            .collect())
    }

    /// The named people in this library, ready to match against.
    ///
    /// A file written before the pointers change (ADR-0019) still holds vectors; the
    /// first read turns what the index knows into pointers and rewrites it, so the
    /// 172 KB a reference library carried becomes a few kilobytes without anyone
    /// asking. Converges: once nothing more can be contracted, nothing more is
    /// written, and a file whose vectors match nothing is left exactly as it is.
    pub fn people(&self) -> Result<crate::faces::people::People> {
        let records = crate::faces::people::People::read_records(self.root())?;
        let inline_before = records.inline_vectors();
        let people = crate::faces::people::People::from_records(self, records);
        if inline_before > 0 {
            let contracted = people.to_records(self)?;
            if contracted.inline_vectors() < inline_before {
                self.save_people(&people)?;
            }
        }
        Ok(people)
    }

    /// Write the named people back, as pointers wherever the index has the face.
    pub fn save_people(&self, people: &crate::faces::people::People) -> Result<()> {
        let records = people.to_records(self)?;
        let p = crate::faces::people::People::path(self.root());
        std::fs::write(&p, serde_json::to_vec_pretty(&records)?)
            .with_context(|| format!("writing {}", p.display()))?;
        // The old in-cache copy, if a very old version left one, is stale now.
        let _ = std::fs::remove_file(
            self.root().join(crate::library::VAULT_DIR).join("people.json"),
        );
        Ok(())
    }

    /// Largest face ratio per photo hash. Drives the scenery split.
    pub fn max_face_ratio(&self, hash: &str) -> Result<f32> {
        Ok(self
            .index
            .conn()
            .query_row(
                "SELECT COALESCE(MAX(ratio), 0.0) FROM faces WHERE hash=?1",
                params![hash],
                |r| r.get(0),
            )
            .unwrap_or(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_round_trips() {
        let v: Vec<f32> = (0..DIM).map(|i| i as f32 / 128.0).collect();
        let back = from_blob(&to_blob(&v)).expect("round trip");
        assert_eq!(v, back);
    }

    #[test]
    fn rejects_wrong_length_blob() {
        assert!(from_blob(&[0u8; 16]).is_none());
    }
}
