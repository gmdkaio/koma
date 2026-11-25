use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::media;
use crate::store::{Store, file_mtime_unix, slugify};

pub struct ImportOptions {
    pub source_dir: PathBuf,
    pub dry_run: bool,
    pub merges: HashMap<String, String>,
}

#[derive(Debug)]
pub struct ImportFailure {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub channels: BTreeMap<String, usize>,
    pub imported: usize,
    pub skipped_empty_dirs: Vec<String>,
    pub failures: Vec<ImportFailure>,
    pub total_media_bytes: u64,
    pub total_thumb_bytes: u64,
}

pub fn run_import(
    mut store: Option<&mut Store>,
    opts: &ImportOptions,
) -> Result<ImportReport, String> {
    if !opts.source_dir.is_dir() {
        return Err(format!(
            "source directory does not exist: {}",
            opts.source_dir.display()
        ));
    }
    if !opts.dry_run && store.is_none() {
        return Err("store is required when dry_run is false".to_string());
    }

    let mut normalized_merges = HashMap::with_capacity(opts.merges.len());
    for (from, to) in &opts.merges {
        normalized_merges.insert(slugify(from)?, slugify(to)?);
    }

    let mut report = ImportReport::default();
    let mut subdirs: Vec<PathBuf> = fs::read_dir(&opts.source_dir)
        .map_err(|e| format!("failed to read {}: {e}", opts.source_dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();

    let mut planned: BTreeMap<String, Vec<(i64, PathBuf)>> = BTreeMap::new();

    for dir in subdirs {
        let raw_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("non-utf8 folder name: {}", dir.display()))?;

        let files: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| format!("failed to read {}: {e}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();

        if files.is_empty() {
            report.skipped_empty_dirs.push(raw_name.to_string());
            continue;
        }

        let slug = slugify(raw_name)?;
        let target = normalized_merges.get(&slug).cloned().unwrap_or(slug);
        let bucket = planned.entry(target).or_default();
        for file in files {
            bucket.push((file_mtime_unix(&file), file));
        }
    }

    for (channel, mut files) in planned {
        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        for (_mtime, file_path) in files {
            if opts.dry_run {
                match validate_file(&file_path) {
                    Ok(size) => {
                        *report.channels.entry(channel.clone()).or_insert(0) += 1;
                        report.imported += 1;
                        report.total_media_bytes += size;
                    }
                    Err(reason) => report.failures.push(ImportFailure {
                        path: file_path,
                        reason,
                    }),
                }
            } else {
                let s = store.as_deref_mut().expect("checked above");
                match s.ingest_file(&channel, &file_path) {
                    Ok(rec) => {
                        *report.channels.entry(channel.clone()).or_insert(0) += 1;
                        report.imported += 1;
                        report.total_media_bytes += rec.size_bytes;
                        report.total_thumb_bytes += rec.thumb_bytes;
                    }
                    Err(reason) => report.failures.push(ImportFailure {
                        path: file_path,
                        reason,
                    }),
                }
            }
        }
    }

    Ok(report)
}

fn validate_file(path: &Path) -> Result<u64, String> {
    let bytes = fs::read(path).map_err(|e| format!("read error: {e}"))?;
    media::validate_image(&bytes)?;
    Ok(bytes.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("koma-test-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sample_png(path: &Path, w: u32, h: u32) {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(w, h, Rgba([120, 40, 200, 255]));
        img.save_with_format(path, image::ImageFormat::Png).unwrap();
    }

    #[test]
    fn import_dry_run_and_copy_only_with_merges_and_extensionless() {
        let src = temp_dir("src");
        let dst = temp_dir("dst");

        fs::create_dir_all(src.join("ReZero")).unwrap();
        fs::create_dir_all(src.join("crusch")).unwrap();
        fs::create_dir_all(src.join("wallpapers")).unwrap();

        write_sample_png(&src.join("ReZero/img1.png"), 640, 480);
        write_sample_png(&src.join("crusch/noext"), 800, 600);

        let mut merges = HashMap::new();
        merges.insert("crusch".to_string(), "rezero".to_string());

        let dry_opts = ImportOptions {
            source_dir: src.clone(),
            dry_run: true,
            merges: merges.clone(),
        };
        let dry_report = run_import(None, &dry_opts).unwrap();
        assert_eq!(dry_report.imported, 2);
        assert_eq!(dry_report.channels.get("rezero"), Some(&2));
        assert_eq!(dry_report.skipped_empty_dirs, vec!["wallpapers"]);
        assert!(dry_report.failures.is_empty());
        assert!(!dst.join("koma.db").exists());

        let mut store = Store::open(dst.clone()).unwrap();
        let real_opts = ImportOptions {
            source_dir: src.clone(),
            dry_run: false,
            merges,
        };
        let report = run_import(Some(&mut store), &real_opts).unwrap();
        assert_eq!(report.imported, 2);
        assert!(report.failures.is_empty());
        assert!(report.total_thumb_bytes > 0);

        let channels = store.list_channels().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].slug, "rezero");
        assert_eq!(channels[0].image_count, 2);

        let images = store.list_images(Some(channels[0].id)).unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "rezero_0002.png");
        assert_eq!(images[1].filename, "rezero_0001.png");

        let thumb_bytes = fs::read(dst.join("thumbs/rezero/rezero_0001.webp")).unwrap();
        assert_eq!(&thumb_bytes[0..4], b"RIFF");
        assert_eq!(&thumb_bytes[8..12], b"WEBP");

        // Source files must stay untouched.
        assert!(src.join("crusch/noext").exists());
        assert!(src.join("ReZero/img1.png").exists());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }
}
