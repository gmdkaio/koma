use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::media;

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: i64,
    pub slug: String,
    pub image_count: i64,
    pub latest_thumb: Option<String>,
    pub last_added: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ImageRecord {
    pub id: i64,
    pub channel_id: i64,
    pub channel_slug: String,
    pub filename: String,
    pub rel_path: String,
    pub thumb_path: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub thumb_bytes: u64,
    pub created_at: i64,
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

fn sanitize_filename(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed == "."
        || trimmed == ".."
    {
        return Err(format!("invalid filename: {raw:?}"));
    }
    Ok(trimmed.to_string())
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
                "SELECT c.id,
                        c.slug,
                        COUNT(i.id) AS img_count,
                        (SELECT i2.thumb_path
                         FROM images i2
                         WHERE i2.channel_id = c.id
                         ORDER BY i2.created_at DESC, i2.id DESC
                         LIMIT 1) AS latest_thumb,
                        MAX(i.created_at) AS last_added
                 FROM channels c
                 LEFT JOIN images i ON i.channel_id = c.id
                 GROUP BY c.id
                 ORDER BY img_count DESC, c.slug ASC",
            )
            .map_err(|e| format!("failed to query channels: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(Channel {
                    id: row.get(0)?,
                    slug: row.get(1)?,
                    image_count: row.get(2)?,
                    latest_thumb: row.get(3)?,
                    last_added: row.get(4)?,
                })
            })
            .map_err(|e| format!("failed to map channels: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to read channel row: {e}"))
    }

    pub fn create_channel(&self, raw_name: &str) -> Result<Channel, String> {
        let slug = slugify(raw_name)?;
        let now = now_unix();
        self.conn
            .execute(
                "INSERT OR IGNORE INTO channels (slug, next_seq, created_at) VALUES (?1, 1, ?2)",
                params![slug, now],
            )
            .map_err(|e| format!("failed to create channel {slug}: {e}"))?;

        self.conn
            .query_row(
                "SELECT c.id, c.slug, COUNT(i.id),
                        (SELECT i2.thumb_path FROM images i2 WHERE i2.channel_id = c.id ORDER BY i2.created_at DESC, i2.id DESC LIMIT 1),
                        MAX(i.created_at)
                 FROM channels c
                 LEFT JOIN images i ON i.channel_id = c.id
                 WHERE c.slug = ?1
                 GROUP BY c.id",
                params![slug],
                |row| {
                    Ok(Channel {
                        id: row.get(0)?,
                        slug: row.get(1)?,
                        image_count: row.get(2)?,
                        latest_thumb: row.get(3)?,
                        last_added: row.get(4)?,
                    })
                },
            )
            .map_err(|e| format!("failed to load channel {slug}: {e}"))
    }

    pub fn list_images(&self, channel_id: Option<i64>) -> Result<Vec<ImageRecord>, String> {
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<ImageRecord> {
            Ok(ImageRecord {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                channel_slug: row.get(2)?,
                filename: row.get(3)?,
                rel_path: row.get(4)?,
                thumb_path: row.get(5)?,
                mime_type: row.get(6)?,
                width: row.get(7)?,
                height: row.get(8)?,
                size_bytes: row.get::<_, i64>(9)? as u64,
                thumb_bytes: 0,
                created_at: row.get(10)?,
            })
        };

        if let Some(cid) = channel_id {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT i.id, i.channel_id, c.slug, i.filename, i.rel_path, i.thumb_path,
                            i.mime_type, i.width, i.height, i.size_bytes, i.created_at
                     FROM images i
                     JOIN channels c ON c.id = i.channel_id
                     WHERE i.channel_id = ?1
                     ORDER BY i.created_at DESC, i.id DESC",
                )
                .map_err(|e| format!("failed to query channel images: {e}"))?;
            let rows = stmt
                .query_map(params![cid], map_row)
                .map_err(|e| format!("failed to map images: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("failed to read image row: {e}"))
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT i.id, i.channel_id, c.slug, i.filename, i.rel_path, i.thumb_path,
                            i.mime_type, i.width, i.height, i.size_bytes, i.created_at
                     FROM images i
                     JOIN channels c ON c.id = i.channel_id
                     ORDER BY i.created_at DESC, i.id DESC",
                )
                .map_err(|e| format!("failed to query all images: {e}"))?;
            let rows = stmt
                .query_map([], map_row)
                .map_err(|e| format!("failed to map images: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("failed to read image row: {e}"))
        }
    }

    pub fn rename_channel(&mut self, channel_id: i64, new_name: &str) -> Result<Channel, String> {
        let new_slug = slugify(new_name)?;
        let old_slug: String = self
            .conn
            .query_row(
                "SELECT slug FROM channels WHERE id = ?1",
                params![channel_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("channel not found: {e}"))?;

        if old_slug == new_slug {
            return self.create_channel(&new_slug);
        }

        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM channels WHERE slug = ?1",
                params![new_slug],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("failed to check channel slug: {e}"))?;
        if exists.is_some() {
            return Err(format!("channel #{new_slug} already exists; use Merge instead"));
        }

        let old_media_dir = self.root.join("media").join(&old_slug);
        let new_media_dir = self.root.join("media").join(&new_slug);
        let old_thumb_dir = self.root.join("thumbs").join(&old_slug);
        let new_thumb_dir = self.root.join("thumbs").join(&new_slug);

        let moved_media = if old_media_dir.exists() {
            fs::rename(&old_media_dir, &new_media_dir)
                .map_err(|e| format!("failed to rename media folder: {e}"))?;
            true
        } else {
            false
        };
        let moved_thumb = if old_thumb_dir.exists() {
            if let Err(e) = fs::rename(&old_thumb_dir, &new_thumb_dir) {
                if moved_media {
                    let _ = fs::rename(&new_media_dir, &old_media_dir);
                }
                return Err(format!("failed to rename thumbs folder: {e}"));
            }
            true
        } else {
            false
        };

        let old_media_prefix = format!("media/{old_slug}/");
        let new_media_prefix = format!("media/{new_slug}/");
        let old_thumb_prefix = format!("thumbs/{old_slug}/");
        let new_thumb_prefix = format!("thumbs/{new_slug}/");

        let db_res = (|| -> Result<(), String> {
            let tx = self
                .conn
                .transaction()
                .map_err(|e| format!("failed to start rename transaction: {e}"))?;
            tx.execute(
                "UPDATE channels SET slug = ?1 WHERE id = ?2",
                params![new_slug, channel_id],
            )
            .map_err(|e| format!("failed to update channel slug: {e}"))?;
            tx.execute(
                "UPDATE images
                 SET rel_path = ?1 || substr(rel_path, length(?2) + 1),
                     thumb_path = ?3 || substr(thumb_path, length(?4) + 1)
                 WHERE channel_id = ?5",
                params![
                    new_media_prefix,
                    old_media_prefix,
                    new_thumb_prefix,
                    old_thumb_prefix,
                    channel_id
                ],
            )
            .map_err(|e| format!("failed to update image paths: {e}"))?;
            tx.commit()
                .map_err(|e| format!("failed to commit rename: {e}"))
        })();

        if let Err(e) = db_res {
            if moved_media {
                let _ = fs::rename(&new_media_dir, &old_media_dir);
            }
            if moved_thumb {
                let _ = fs::rename(&new_thumb_dir, &old_thumb_dir);
            }
            return Err(e);
        }

        self.create_channel(&new_slug)
    }

    pub fn merge_channels(&mut self, from_id: i64, into_id: i64) -> Result<usize, String> {
        if from_id == into_id {
            return Err("cannot merge a channel into itself".to_string());
        }
        let images = self.list_images(Some(from_id))?;
        let ids: Vec<i64> = images.iter().rev().map(|i| i.id).collect();
        let moved = if ids.is_empty() {
            0
        } else {
            self.move_images(&ids, into_id)?.len()
        };
        self.delete_channel(from_id)?;
        Ok(moved)
    }

    pub fn delete_channel(&mut self, channel_id: i64) -> Result<(), String> {
        let slug: String = self
            .conn
            .query_row(
                "SELECT slug FROM channels WHERE id = ?1",
                params![channel_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("channel not found: {e}"))?;

        self.conn
            .execute("DELETE FROM channels WHERE id = ?1", params![channel_id])
            .map_err(|e| format!("failed to delete channel: {e}"))?;

        let media_dir = self.root.join("media").join(&slug);
        if media_dir.exists() {
            let _ = fs::remove_dir_all(&media_dir);
        }
        let thumb_dir = self.root.join("thumbs").join(&slug);
        if thumb_dir.exists() {
            let _ = fs::remove_dir_all(&thumb_dir);
        }
        Ok(())
    }

    pub fn move_images(
        &mut self,
        image_ids: &[i64],
        target_channel_id: i64,
    ) -> Result<Vec<ImageRecord>, String> {
        if image_ids.is_empty() {
            return Ok(Vec::new());
        }

        let target_slug: String = self
            .conn
            .query_row(
                "SELECT slug FROM channels WHERE id = ?1",
                params![target_channel_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("target channel not found: {e}"))?;

        let target_media_dir = self.root.join("media").join(&target_slug);
        let target_thumb_dir = self.root.join("thumbs").join(&target_slug);
        fs::create_dir_all(&target_media_dir)
            .map_err(|e| format!("failed to create media dir: {e}"))?;
        fs::create_dir_all(&target_thumb_dir)
            .map_err(|e| format!("failed to create thumbs dir: {e}"))?;

        let mut moved_records = Vec::new();
        for &image_id in image_ids {
            let tx = self
                .conn
                .transaction()
                .map_err(|e| format!("failed to start move transaction: {e}"))?;

            let current: Option<(i64, String, String, String, String, u32, u32, i64, i64)> = tx
                .query_row(
                    "SELECT channel_id, filename, rel_path, thumb_path, mime_type, width, height, size_bytes, created_at
                     FROM images WHERE id = ?1",
                    params![image_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| format!("failed to load image {image_id}: {e}"))?;

            let Some((
                cur_cid,
                cur_filename,
                cur_rel,
                cur_thumb,
                mime_type,
                width,
                height,
                size_bytes,
                created_at,
            )) = current
            else {
                continue;
            };

            if cur_cid == target_channel_id {
                continue;
            }

            let seq: i64 = tx
                .query_row(
                    "SELECT next_seq FROM channels WHERE id = ?1",
                    params![target_channel_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("failed to read target seq: {e}"))?;

            tx.execute(
                "UPDATE channels SET next_seq = ?1 WHERE id = ?2",
                params![seq + 1, target_channel_id],
            )
            .map_err(|e| format!("failed to increment target seq: {e}"))?;

            let ext = Path::new(&cur_filename)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png");
            let stem = format!("{target_slug}_{seq:04}");
            let new_filename = format!("{stem}.{ext}");
            let new_rel = format!("media/{target_slug}/{new_filename}");
            let new_thumb = format!("thumbs/{target_slug}/{stem}.webp");

            let old_media_abs = self.root.join(&cur_rel);
            let new_media_abs = self.root.join(&new_rel);
            let old_thumb_abs = self.root.join(&cur_thumb);
            let new_thumb_abs = self.root.join(&new_thumb);

            fs::rename(&old_media_abs, &new_media_abs)
                .map_err(|e| format!("failed to move {}: {e}", old_media_abs.display()))?;
            if let Err(e) = fs::rename(&old_thumb_abs, &new_thumb_abs) {
                let _ = fs::rename(&new_media_abs, &old_media_abs);
                return Err(format!("failed to move {}: {e}", old_thumb_abs.display()));
            }

            let update_res = tx.execute(
                "UPDATE images
                 SET channel_id = ?1, seq = ?2, filename = ?3, rel_path = ?4, thumb_path = ?5
                 WHERE id = ?6",
                params![
                    target_channel_id,
                    seq,
                    new_filename,
                    new_rel,
                    new_thumb,
                    image_id
                ],
            );

            if let Err(e) = update_res {
                let _ = fs::rename(&new_media_abs, &old_media_abs);
                let _ = fs::rename(&new_thumb_abs, &old_thumb_abs);
                return Err(format!("failed to update moved image row: {e}"));
            }

            if let Err(e) = tx.commit() {
                let _ = fs::rename(&new_media_abs, &old_media_abs);
                let _ = fs::rename(&new_thumb_abs, &old_thumb_abs);
                return Err(format!("failed to commit move: {e}"));
            }

            moved_records.push(ImageRecord {
                id: image_id,
                channel_id: target_channel_id,
                channel_slug: target_slug.clone(),
                filename: new_filename,
                rel_path: new_rel,
                thumb_path: new_thumb,
                mime_type,
                width,
                height,
                size_bytes: size_bytes as u64,
                thumb_bytes: 0,
                created_at,
            });
        }

        Ok(moved_records)
    }

    pub fn rename_image(&mut self, image_id: i64, raw_filename: &str) -> Result<ImageRecord, String> {
        let mut new_name = sanitize_filename(raw_filename)?;
        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("failed to start rename image tx: {e}"))?;

        let (channel_id, slug, old_filename, old_rel, thumb_path, mime_type, width, height, size_bytes, created_at):
            (i64, String, String, String, String, String, u32, u32, i64, i64) = tx
            .query_row(
                "SELECT i.channel_id, c.slug, i.filename, i.rel_path, i.thumb_path,
                        i.mime_type, i.width, i.height, i.size_bytes, i.created_at
                 FROM images i
                 JOIN channels c ON c.id = i.channel_id
                 WHERE i.id = ?1",
                params![image_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .map_err(|e| format!("image not found: {e}"))?;

        if Path::new(&new_name).extension().is_none()
            && let Some(ext) = Path::new(&old_filename).extension().and_then(|e| e.to_str())
        {
            new_name = format!("{new_name}.{ext}");
        }

        if new_name == old_filename {
            return Ok(ImageRecord {
                id: image_id,
                channel_id,
                channel_slug: slug,
                filename: old_filename,
                rel_path: old_rel,
                thumb_path,
                mime_type,
                width,
                height,
                size_bytes: size_bytes as u64,
                thumb_bytes: 0,
                created_at,
            });
        }

        let new_rel = format!("media/{slug}/{new_name}");
        let old_abs = self.root.join(&old_rel);
        let new_abs = self.root.join(&new_rel);
        if new_abs.exists() {
            return Err(format!("file {new_name} already exists in #{slug}"));
        }

        fs::rename(&old_abs, &new_abs)
            .map_err(|e| format!("failed to rename file: {e}"))?;

        let res = tx.execute(
            "UPDATE images SET filename = ?1, rel_path = ?2 WHERE id = ?3",
            params![new_name, new_rel, image_id],
        );
        if let Err(e) = res {
            let _ = fs::rename(&new_abs, &old_abs);
            return Err(format!("failed to update filename in db: {e}"));
        }
        if let Err(e) = tx.commit() {
            let _ = fs::rename(&new_abs, &old_abs);
            return Err(format!("failed to commit filename change: {e}"));
        }

        Ok(ImageRecord {
            id: image_id,
            channel_id,
            channel_slug: slug,
            filename: new_name,
            rel_path: new_rel,
            thumb_path,
            mime_type,
            width,
            height,
            size_bytes: size_bytes as u64,
            thumb_bytes: 0,
            created_at,
        })
    }

    pub fn delete_images(&mut self, image_ids: &[i64]) -> Result<usize, String> {
        let mut deleted = 0usize;
        for &id in image_ids {
            let paths: Option<(String, String)> = self
                .conn
                .query_row(
                    "SELECT rel_path, thumb_path FROM images WHERE id = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("failed to look up image {id}: {e}"))?;

            let Some((rel_path, thumb_path)) = paths else {
                continue;
            };
            self.conn
                .execute("DELETE FROM images WHERE id = ?1", params![id])
                .map_err(|e| format!("failed to delete image {id}: {e}"))?;
            let _ = fs::remove_file(self.root.join(rel_path));
            let _ = fs::remove_file(self.root.join(thumb_path));
            deleted += 1;
        }
        Ok(deleted)
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
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create channel media dir: {e}"))?;
        }
        if let Some(parent) = thumb_abs.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create channel thumb dir: {e}"))?;
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

        let id = tx.last_insert_rowid();
        if let Err(e) = tx.commit() {
            let _ = fs::remove_file(&media_abs);
            let _ = fs::remove_file(&thumb_abs);
            return Err(format!("failed to commit transaction: {e}"));
        }

        Ok(ImageRecord {
            id,
            channel_id,
            channel_slug: slug,
            filename,
            rel_path,
            thumb_path,
            mime_type: decoded.kind.mime().to_string(),
            width: decoded.width,
            height: decoded.height,
            size_bytes,
            thumb_bytes,
            created_at: ts,
        })
    }

    pub fn ingest_file(&mut self, channel_name: &str, path: &Path) -> Result<ImageRecord, String> {
        let bytes =
            fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mtime = file_mtime_unix(path);
        self.ingest_bytes(channel_name, &bytes, Some(mtime))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use std::io::Cursor;

    fn sample_png_bytes() -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(32, 24, Rgba([80, 120, 220, 255]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn channel_and_image_mutations_keep_disk_and_db_in_sync() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("koma-store-test-{nanos}"));
        let mut store = Store::open(root.clone()).unwrap();
        let png = sample_png_bytes();

        let img1 = store.ingest_bytes("alpha", &png, Some(1000)).unwrap();
        let img2 = store.ingest_bytes("beta", &png, Some(2000)).unwrap();
        let beta = store.create_channel("beta").unwrap();

        let moved = store.move_images(&[img1.id], beta.id).unwrap();
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].filename, "beta_0002.png");
        assert!(root.join(&moved[0].rel_path).exists());
        assert!(!root.join(&img1.rel_path).exists());

        let renamed_img = store.rename_image(img2.id, "custom_shot").unwrap();
        assert_eq!(renamed_img.filename, "custom_shot.png");
        assert!(root.join(&renamed_img.rel_path).exists());

        let renamed_ch = store.rename_channel(beta.id, "gamma").unwrap();
        assert_eq!(renamed_ch.slug, "gamma");
        assert_eq!(renamed_ch.image_count, 2);
        let gamma_imgs = store.list_images(Some(renamed_ch.id)).unwrap();
        assert!(gamma_imgs.iter().all(|i| root.join(&i.rel_path).exists()));
        assert!(gamma_imgs.iter().all(|i| root.join(&i.thumb_path).exists()));

        let alpha = store.create_channel("alpha").unwrap();
        store.merge_channels(renamed_ch.id, alpha.id).unwrap();
        let alpha_imgs = store.list_images(Some(alpha.id)).unwrap();
        assert_eq!(alpha_imgs.len(), 2);

        let deleted = store.delete_images(&[alpha_imgs[0].id]).unwrap();
        assert_eq!(deleted, 1);
        assert!(!root.join(&alpha_imgs[0].rel_path).exists());
        assert_eq!(store.list_images(Some(alpha.id)).unwrap().len(), 1);

        let _ = fs::remove_dir_all(&root);
    }
}
