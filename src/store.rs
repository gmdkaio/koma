use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::media;

#[derive(Debug, Clone)]
pub struct Channel {
    pub slug: String,
    pub image_count: i64,
}

#[derive(Debug, Clone)]
pub struct ImageRecord {
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub thumb_bytes: u64,
}

pub struct Store {
    root: PathBuf,
    conn: Connection,
}

pub fn default_data_dir() -> Result<PathBuf, String> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("koma"));
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
    Ok(PathBuf::from(home).join(".local/share/koma"))
}

pub fn slugify(raw: &str) -> Result<String, String> {
    let mut slug = String::with_capacity(raw.len());
    let mut prev_underscore = false;

    for ch in raw.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
            prev_underscore = false;
        } else if (c == '_' || c == '-' || c.is_whitespace()) && !slug.is_empty() && !prev_underscore
        {
            slug.push('_');
            prev_underscore = true;
        }
    }

    while slug.ends_with('_') {
        slug.pop();
    }

    if slug.is_empty() {
        return Err(format!("invalid empty channel name: {raw:?}"));
    }
    Ok(slug)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn file_mtime_unix(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(now_unix)
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(root.join("media"))
            .map_err(|e| format!("failed to create media dir: {e}"))?;
        fs::create_dir_all(root.join("thumbs"))
            .map_err(|e| format!("failed to create thumbs dir: {e}"))?;

        let db_path = root.join("koma.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("failed to open {}: {e}", db_path.display()))?;

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;

            CREATE TABLE IF NOT EXISTS channels (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                slug       TEXT NOT NULL UNIQUE,
                next_seq   INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS images (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id  INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
                seq         INTEGER NOT NULL,
                filename    TEXT NOT NULL,
                rel_path    TEXT NOT NULL UNIQUE,
                thumb_path  TEXT NOT NULL,
                mime_type   TEXT NOT NULL,
                width       INTEGER NOT NULL,
                height      INTEGER NOT NULL,
                size_bytes  INTEGER NOT NULL,
                created_at  INTEGER NOT NULL,
                UNIQUE(channel_id, seq)
            );

            CREATE INDEX IF NOT EXISTS idx_images_channel_created
                ON images(channel_id, created_at, id);
            ",
        )
        .map_err(|e| format!("failed to initialize schema: {e}"))?;

        Ok(Self { root, conn })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn list_channels(&self) -> Result<Vec<Channel>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.slug, COUNT(i.id)
                 FROM channels c
                 LEFT JOIN images i ON i.channel_id = c.id
                 GROUP BY c.id
                 ORDER BY c.slug ASC",
            )
            .map_err(|e| format!("failed to query channels: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(Channel {
                    slug: row.get(0)?,
                    image_count: row.get(1)?,
                })
            })
            .map_err(|e| format!("failed to map channels: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to read channel row: {e}"))
    }

    #[cfg(test)]
    pub fn list_images(&self, slug: &str) -> Result<Vec<ImageRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT i.filename, i.thumb_path, i.width, i.height, i.size_bytes
                 FROM images i
                 JOIN channels c ON c.id = i.channel_id
                 WHERE c.slug = ?1
                 ORDER BY i.created_at ASC, i.id ASC",
            )
            .map_err(|e| format!("failed to query images: {e}"))?;

        let rows = stmt
            .query_map(params![slug], |row| {
                let thumb_path: String = row.get(1)?;
                let thumb_bytes = fs::metadata(self.root.join(&thumb_path))
                    .map(|m| m.len())
                    .unwrap_or(0);
                Ok(ImageRecord {
                    filename: row.get(0)?,
                    width: row.get(2)?,
                    height: row.get(3)?,
                    size_bytes: row.get::<_, i64>(4)? as u64,
                    thumb_bytes,
                })
            })
            .map_err(|e| format!("failed to map images: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to read image row: {e}"))
    }

    pub fn ingest_bytes(
        &mut self,
        channel_name: &str,
        bytes: &[u8],
        created_at: Option<i64>,
    ) -> Result<ImageRecord, String> {
        let slug = slugify(channel_name)?;
        let decoded = media::inspect_and_thumbnail(bytes)?;
        let ts = created_at.unwrap_or_else(now_unix);
        let size_bytes = bytes.len() as u64;
        let thumb_bytes = decoded.thumb_webp.len() as u64;

        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        tx.execute(
            "INSERT OR IGNORE INTO channels (slug, next_seq, created_at) VALUES (?1, 1, ?2)",
            params![slug, ts],
        )
        .map_err(|e| format!("failed to ensure channel {slug}: {e}"))?;

        let (channel_id, seq): (i64, i64) = tx
            .query_row(
                "SELECT id, next_seq FROM channels WHERE slug = ?1",
                params![slug],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("failed to read channel sequence: {e}"))?
            .ok_or_else(|| format!("channel {slug} missing after insert"))?;

        tx.execute(
            "UPDATE channels SET next_seq = ?1 WHERE id = ?2",
            params![seq + 1, channel_id],
        )
        .map_err(|e| format!("failed to increment sequence: {e}"))?;

        let stem = format!("{slug}_{seq:04}");
        let filename = format!("{stem}.{}", decoded.kind.ext());
        let rel_path = format!("media/{slug}/{filename}");
        let thumb_path = format!("thumbs/{slug}/{stem}.webp");

        let media_abs = self.root.join(&rel_path);
        let thumb_abs = self.root.join(&thumb_path);

        if let Some(parent) = media_abs.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("failed to create channel media dir: {e}"))?;
        }
        if let Some(parent) = thumb_abs.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("failed to create channel thumb dir: {e}"))?;
        }

        fs::write(&media_abs, bytes)
            .map_err(|e| format!("failed to write {}: {e}", media_abs.display()))?;
        if let Err(e) = fs::write(&thumb_abs, &decoded.thumb_webp) {
            let _ = fs::remove_file(&media_abs);
            return Err(format!("failed to write {}: {e}", thumb_abs.display()));
        }

        let insert_res = tx.execute(
            "INSERT INTO images
                (channel_id, seq, filename, rel_path, thumb_path, mime_type, width, height, size_bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                channel_id,
                seq,
                filename,
                rel_path,
                thumb_path,
                decoded.kind.mime(),
                decoded.width,
                decoded.height,
                size_bytes as i64,
                ts
            ],
        );

        if let Err(e) = insert_res {
            let _ = fs::remove_file(&media_abs);
            let _ = fs::remove_file(&thumb_abs);
            return Err(format!("failed to insert image row: {e}"));
        }

        if let Err(e) = tx.commit() {
            let _ = fs::remove_file(&media_abs);
            let _ = fs::remove_file(&thumb_abs);
            return Err(format!("failed to commit transaction: {e}"));
        }

        Ok(ImageRecord {
            filename,
            width: decoded.width,
            height: decoded.height,
            size_bytes,
            thumb_bytes,
        })
    }

    pub fn ingest_file(&mut self, channel_name: &str, path: &Path) -> Result<ImageRecord, String> {
        let bytes = fs::read(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mtime = file_mtime_unix(path);
        self.ingest_bytes(channel_name, &bytes, Some(mtime))
    }
}
