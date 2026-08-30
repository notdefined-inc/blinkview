//! Persisting detected faces alongside the photo index.

use crate::{faces::embed::DIM, Library};
use anyhow::Result;
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

fn to_blob(v: &[f32]) -> Vec<u8> {
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
