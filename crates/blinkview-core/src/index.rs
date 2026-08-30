//! SQLite index. Entirely derived from the photos on disk — see ADR-0001.
//!
//! Note `files.hash` is deliberately *not* unique: two files may legitimately hold
//! identical bytes. `path` is the unique column.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Bumped whenever blinkview learns to read something it could not before, so files
/// previously given up on are tried again.
pub const DECODER_GENERATION: &str = "2026-08-29-webp-gif";
use std::path::Path;

pub struct Index {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub hash: String,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub kind: String,
    pub taken_at: Option<i64>,
    pub taken_src: Option<String>,
}

impl Index {
    pub fn open(db: &Path) -> Result<Self> {
        let conn = Connection::open(db).with_context(|| format!("opening {}", db.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS files (
                id        INTEGER PRIMARY KEY,
                hash      TEXT    NOT NULL,
                path      TEXT    NOT NULL UNIQUE,
                size      INTEGER NOT NULL,
                mtime     INTEGER NOT NULL,
                kind      TEXT    NOT NULL,
                taken_at  INTEGER,
                taken_src TEXT
            );
            CREATE INDEX IF NOT EXISTS files_hash ON files(hash);
            CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
            -- Where a photograph was taken, keyed by content hash like everything else
            -- derived, so it survives a rename or a move. A row with NULL coordinates
            -- means *checked, and it has none* — without that the map would re-read
            -- every screenshot in the library every time it opened.
            CREATE TABLE IF NOT EXISTS gps (
                hash TEXT PRIMARY KEY,
                lat  REAL,
                lon  REAL
            );
            -- Keyed by content hash, not path: a renamed or moved file keeps its
            -- signature for free, which is what makes rescans cheap (ADR-0001).
            -- One row per detected face. Keyed by photo content hash, so faces
            -- survive renames and moves along with everything else.
            CREATE TABLE IF NOT EXISTS faces (
                id        INTEGER PRIMARY KEY,
                hash      TEXT    NOT NULL,
                idx       INTEGER NOT NULL,
                x         REAL    NOT NULL,
                y         REAL    NOT NULL,
                w         REAL    NOT NULL,
                h         REAL    NOT NULL,
                score     REAL    NOT NULL,
                ratio     REAL    NOT NULL,
                embedding BLOB,
                UNIQUE(hash, idx)
            );
            CREATE INDEX IF NOT EXISTS faces_hash ON faces(hash);
            -- Photos analysed but containing no usable face. Without this we would
            -- re-decode every landscape shot on each run.
            CREATE TABLE IF NOT EXISTS faces_done (hash TEXT PRIMARY KEY);
            -- Photographs that could not be decoded. Without this a file blinkview
            -- cannot read looks identical to one not yet analysed, so every pass
            -- retries it and the library never reports itself finished.
            CREATE TABLE IF NOT EXISTS unreadable (
                hash   TEXT PRIMARY KEY,
                reason TEXT NOT NULL,
                at     INTEGER NOT NULL,
                -- Which build gave up. A later one may understand the format: WebP
                -- was added exactly because fifteen files kept failing, and had this
                -- table existed first they would have been written off for good.
                version TEXT NOT NULL DEFAULT ''
            );
            -- Semantic (CLIP) embeddings. Derived, so it belongs in the disposable
            -- vault; keyed by content hash like everything else, so it survives
            -- renaming and moving.
            CREATE TABLE IF NOT EXISTS clip (
                hash      TEXT PRIMARY KEY,
                embedding BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS signatures (
                hash      TEXT PRIMARY KEY,
                dhash     INTEGER NOT NULL,
                thumb     BLOB    NOT NULL,
                sharpness REAL    NOT NULL,
                width     INTEGER NOT NULL,
                height    INTEGER NOT NULL
            );
            "#,
        )?;
        Ok(Self { conn })
    }

    pub fn by_path(&self, path: &str) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT hash,path,size,mtime,kind,taken_at,taken_src FROM files WHERE path=?1",
                params![path],
                row_to_file,
            )
            .optional()?)
    }

    /// Rows sharing a content hash. Used to re-identify files moved outside the tool.
    pub fn by_hash(&self, hash: &str) -> Result<Vec<FileRow>> {
        let mut st = self
            .conn
            .prepare("SELECT hash,path,size,mtime,kind,taken_at,taken_src FROM files WHERE hash=?1")?;
        let rows = st.query_map(params![hash], row_to_file)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn all(&self) -> Result<Vec<FileRow>> {
        let mut st = self
            .conn
            .prepare("SELECT hash,path,size,mtime,kind,taken_at,taken_src FROM files ORDER BY path")?;
        let rows = st.query_map([], row_to_file)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn upsert(&self, f: &FileRow) -> Result<()> {
        self.conn.execute(
            r#"INSERT INTO files (hash,path,size,mtime,kind,taken_at,taken_src)
               VALUES (?1,?2,?3,?4,?5,?6,?7)
               ON CONFLICT(path) DO UPDATE SET
                 hash=excluded.hash, size=excluded.size, mtime=excluded.mtime,
                 kind=excluded.kind, taken_at=excluded.taken_at,
                 taken_src=excluded.taken_src"#,
            params![f.hash, f.path, f.size, f.mtime, f.kind, f.taken_at, f.taken_src],
        )?;
        Ok(())
    }

    /// Move a row to a new path, keeping its identity. Used both when we move files
    /// ourselves and when a scan discovers the user moved one in Finder.
    pub fn repath(&self, from: &str, to: &str) -> Result<()> {
        self.conn
            .execute("UPDATE files SET path=?2 WHERE path=?1", params![from, to])?;
        Ok(())
    }

    /// Is this index usable?
    ///
    /// `quick_check` alone is not enough, and measurably so: scribbling 512 bytes over
    /// a page body still returns "ok", because it validates b-tree structure rather
    /// than page contents. So the tables are also counted, which walks every page they
    /// occupy and turns damage into an error here instead of a failed query later.
    ///
    /// Neither catches garbage *inside* an otherwise well-formed cell — a corrupted
    /// path string reads back as nonsense rather than an error. That case is repaired
    /// by the next scan, which reconciles against the filesystem anyway.
    pub fn integrity_check(&self) -> Result<()> {
        let status: String = self
            .conn
            .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
        if status != "ok" {
            anyhow::bail!("index failed its integrity check: {status}");
        }
        for table in ["files", "clip", "faces", "signatures"] {
            self.conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .with_context(|| format!("reading {table}"))?;
        }
        Ok(())
    }

    pub fn remove_path(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM files WHERE path=?1", params![path])?;
        Ok(())
    }

    /// Record a photograph's coordinates, or that it has none.
    pub fn set_gps(&self, hash: &str, at: Option<(f64, f64)>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO gps(hash,lat,lon) VALUES(?1,?2,?3)
             ON CONFLICT(hash) DO UPDATE SET lat=excluded.lat, lon=excluded.lon",
            params![hash, at.map(|p| p.0), at.map(|p| p.1)],
        )?;
        Ok(())
    }

    /// `None` when the photograph has not been looked at; `Some(None)` when it was and
    /// carried nothing.
    pub fn get_gps(&self, hash: &str) -> Result<Option<Option<(f64, f64)>>> {
        let mut q = self.conn.prepare("SELECT lat,lon FROM gps WHERE hash=?1")?;
        let mut rows = q.query(params![hash])?;
        match rows.next()? {
            None => Ok(None),
            Some(r) => {
                let lat: Option<f64> = r.get(0)?;
                let lon: Option<f64> = r.get(1)?;
                Ok(Some(match (lat, lon) {
                    (Some(a), Some(b)) => Some((a, b)),
                    _ => None,
                }))
            }
        }
    }

    /// Every photograph that has coordinates.
    pub fn located(&self) -> Result<Vec<(String, f64, f64)>> {
        let mut q = self
            .conn
            .prepare("SELECT hash,lat,lon FROM gps WHERE lat IS NOT NULL AND lon IS NOT NULL")?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Content hashes already looked at, whatever the answer was.
    pub fn gps_checked(&self) -> Result<std::collections::HashSet<String>> {
        let mut q = self.conn.prepare("SELECT hash FROM gps")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_signature(&self, hash: &str) -> Result<Option<crate::imagesig::Signature>> {
        Ok(self
            .conn
            .query_row(
                "SELECT dhash,thumb,sharpness,width,height FROM signatures WHERE hash=?1",
                params![hash],
                |r| {
                    Ok(crate::imagesig::Signature {
                        dhash: r.get::<_, i64>(0)? as u64,
                        thumb: r.get(1)?,
                        sharpness: r.get(2)?,
                        width: r.get(3)?,
                        height: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn put_signature(&self, hash: &str, s: &crate::imagesig::Signature) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO signatures (hash,dhash,thumb,sharpness,width,height)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![hash, s.dhash as i64, s.thumb, s.sharpness, s.width, s.height],
        )?;
        Ok(())
    }

    /// Raw connection, for modules that own their own tables (faces, signatures).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn get_clip(&self, hash: &str) -> Result<Option<Vec<f32>>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT embedding FROM clip WHERE hash=?1", params![hash], |r| r.get(0))
            .optional()?;
        Ok(blob.as_deref().and_then(floats_from))
    }

    pub fn put_clip(&self, hash: &str, embedding: &[f32]) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO clip (hash, embedding) VALUES (?1, ?2)",
            params![hash, floats_to(embedding)],
        )?;
        Ok(())
    }

    /// Record that a photograph could not be decoded, so it is not tried for ever.
    ///
    /// Keyed by content hash: if the file is replaced with a readable one its hash
    /// changes and the note no longer applies.
    pub fn mark_unreadable(&self, hash: &str, reason: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO unreadable (hash, reason, at, version) \
             VALUES (?1, ?2, ?3, ?4)",
            params![hash, reason, chrono::Utc::now().timestamp(), DECODER_GENERATION],
        )?;
        Ok(())
    }

    /// True only when *this* build already failed on it. A note left by an older build
    /// is not trusted, so adding a format retries what it can now read instead of
    /// leaving those files written off.
    pub fn is_unreadable(&self, hash: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM unreadable WHERE hash=?1 AND version=?2",
                params![hash, DECODER_GENERATION],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Every photograph blinkview could not read, with why.
    pub fn unreadable(&self) -> Result<Vec<(String, String)>> {
        let mut st = self.conn.prepare("SELECT hash, reason FROM unreadable")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Forget the failures, so a library can be retried after blinkview learns a format.
    pub fn clear_unreadable(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM unreadable", [])?)
    }

    /// How many photographs have an embedding, without reading any of them.
    pub fn count_clip(&self) -> Result<usize> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM clip", [], |r| r.get::<_, i64>(0))? as usize)
    }

    pub fn all_clip(&self) -> Result<Vec<(String, Vec<f32>)>> {
        let mut st = self.conn.prepare("SELECT hash, embedding FROM clip")?;
        let rows = st.query_map([], |r| {
            let h: String = r.get(0)?;
            let b: Vec<u8> = r.get(1)?;
            Ok((h, b))
        })?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|(h, b)| floats_from(&b).map(|e| (h, e)))
            .collect())
    }

    pub fn clip_count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM clip", [], |r| r.get(0))?)
    }

    pub fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?)
    }

    pub fn transaction<T>(&mut self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let tx = self.conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

fn floats_to(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn floats_from(b: &[u8]) -> Option<Vec<f32>> {
    if b.is_empty() || !b.len().is_multiple_of(4) {
        return None;
    }
    Some(b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

fn row_to_file(r: &rusqlite::Row) -> rusqlite::Result<FileRow> {
    Ok(FileRow {
        hash: r.get(0)?,
        path: r.get(1)?,
        size: r.get(2)?,
        mtime: r.get(3)?,
        kind: r.get(4)?,
        taken_at: r.get(5)?,
        taken_src: r.get(6)?,
    })
}
