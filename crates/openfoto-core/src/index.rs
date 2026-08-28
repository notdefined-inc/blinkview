//! SQLite index. Entirely derived from the photos on disk — see ADR-0001.
//!
//! Note `files.hash` is deliberately *not* unique: two files may legitimately hold
//! identical bytes. `path` is the unique column.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
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

    pub fn remove_path(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM files WHERE path=?1", params![path])?;
        Ok(())
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
    Some(b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
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
