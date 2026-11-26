use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::storage::gdrive::{
    GDriveBackend, TransferFile, authorize_interactive, clear_saved_tokens, has_saved_tokens,
};
use crate::storage::{RemoteFile, StorageBackend};
use crate::store::{Store, slugify};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferBucket {
    Throwaway,
    Files,
}

impl TransferBucket {
    pub fn folder_name(self) -> &'static str {
        match self {
            Self::Throwaway => ".throwaway",
            Self::Files => ".files",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Throwaway => "Throwaway",
            Self::Files => "Files",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub uploaded: usize,
    pub downloaded: usize,
    pub updated: usize,
    pub deleted_remote: usize,
    pub deleted_local: usize,
    pub linked: usize,
}

impl SyncReport {
    pub fn had_local_changes(&self) -> bool {
        self.downloaded > 0 || self.deleted_local > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    LocalOnly,
    Syncing,
    Synced(String),
    Offline(String),
}

#[derive(Debug)]
pub enum SyncEvent {
    StatusChanged(SyncStatus),
    SyncFinished(SyncReport),
    AuthFinished(Result<(), String>),
    TransfersUpdated {
        throwaway: Vec<TransferFile>,
        files: Vec<TransferFile>,
    },
    TransferToast(String),
}

enum SyncCmd {
    TriggerSync,
    StartOAuth,
    Disconnect,
    RefreshTransfers,
    UploadTransfers {
        bucket: TransferBucket,
        paths: Vec<PathBuf>,
    },
    UploadTransferBytes {
        bucket: TransferBucket,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
    },
    DownloadTransfer {
        bucket: TransferBucket,
        file: TransferFile,
        dest_dir: PathBuf,
        auto_delete: bool,
    },
    DeleteTransfer {
        bucket: TransferBucket,
        remote_id: String,
        filename: String,
    },
    Shutdown,
}

pub struct SyncHandle {
    tx: Sender<SyncCmd>,
    rx: Receiver<SyncEvent>,
}

impl SyncHandle {
    pub fn spawn(data_root: PathBuf) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<SyncCmd>();
        let (evt_tx, evt_rx) = mpsc::channel::<SyncEvent>();

        thread::Builder::new()
            .name("koma-sync".to_string())
            .spawn(move || worker_loop(data_root, cmd_rx, evt_tx))
            .expect("failed to spawn sync worker thread");

        Self {
            tx: cmd_tx,
            rx: evt_rx,
        }
    }

    pub fn trigger_sync(&self) {
        let _ = self.tx.send(SyncCmd::TriggerSync);
    }

    pub fn start_oauth(&self) {
        let _ = self.tx.send(SyncCmd::StartOAuth);
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(SyncCmd::Disconnect);
    }

    pub fn refresh_transfers(&self) {
        let _ = self.tx.send(SyncCmd::RefreshTransfers);
    }

    pub fn upload_transfers(&self, bucket: TransferBucket, paths: Vec<PathBuf>) {
        let _ = self.tx.send(SyncCmd::UploadTransfers { bucket, paths });
    }

    pub fn upload_transfer_bytes(
        &self,
        bucket: TransferBucket,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
    ) {
        let _ = self.tx.send(SyncCmd::UploadTransferBytes {
            bucket,
            filename,
            mime_type,
            bytes,
        });
    }

    pub fn download_transfer(
        &self,
        bucket: TransferBucket,
        file: TransferFile,
        dest_dir: PathBuf,
        auto_delete: bool,
    ) {
        let _ = self.tx.send(SyncCmd::DownloadTransfer {
            bucket,
            file,
            dest_dir,
            auto_delete,
        });
    }

    pub fn delete_transfer(&self, bucket: TransferBucket, remote_id: String, filename: String) {
        let _ = self.tx.send(SyncCmd::DeleteTransfer {
            bucket,
            remote_id,
            filename,
        });
    }

    pub fn poll_events(&self) -> Vec<SyncEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = self.rx.try_recv() {
            out.push(evt);
        }
        out
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(SyncCmd::Shutdown);
    }
}

fn worker_loop(data_root: PathBuf, cmd_rx: Receiver<SyncCmd>, evt_tx: Sender<SyncEvent>) {
    let mut store = match Store::open(data_root.clone()) {
        Ok(s) => s,
        Err(e) => {
            let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::Offline(e)));
            return;
        }
    };

    let mut backend: Option<GDriveBackend> = if has_saved_tokens(&data_root) {
        GDriveBackend::open(data_root.clone()).ok()
    } else {
        None
    };

    if let Some(ref b) = backend {
        run_sync_pass(&mut store, b, &evt_tx);
        refresh_transfers_now(b, &evt_tx, None);
    } else {
        let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::LocalOnly));
    }

    let mut last_poll = Instant::now();
    let poll_interval = Duration::from_secs(60);
    let mut queued_cmd: Option<SyncCmd> = None;

    loop {
        let next = if let Some(cmd) = queued_cmd.take() {
            Ok(cmd)
        } else {
            cmd_rx.recv_timeout(Duration::from_secs(5))
        };
        match next {
            Ok(SyncCmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(SyncCmd::TriggerSync) => {
                // Coalesce rapid bursts (e.g. pasting 5 images in a row) without dropping other commands.
                while let Ok(extra) = cmd_rx.try_recv() {
                    if !matches!(extra, SyncCmd::TriggerSync) {
                        queued_cmd = Some(extra);
                        break;
                    }
                }
                if backend.is_none() && has_saved_tokens(&data_root) {
                    backend = GDriveBackend::open(data_root.clone()).ok();
                }
                if let Some(ref b) = backend {
                    run_sync_pass(&mut store, b, &evt_tx);
                    refresh_transfers_now(b, &evt_tx, None);
                    last_poll = Instant::now();
                } else {
                    let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::LocalOnly));
                }
            }
            Ok(SyncCmd::StartOAuth) => {
                let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::Syncing));
                match authorize_interactive(&data_root)
                    .and_then(|_| GDriveBackend::open(data_root.clone()))
                {
                    Ok(b) => {
                        backend = Some(b);
                        let _ = evt_tx.send(SyncEvent::AuthFinished(Ok(())));
                        if let Some(ref b) = backend {
                            run_sync_pass(&mut store, b, &evt_tx);
                            refresh_transfers_now(b, &evt_tx, None);
                        }
                        last_poll = Instant::now();
                    }
                    Err(e) => {
                        let _ = evt_tx.send(SyncEvent::AuthFinished(Err(e)));
                        let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::LocalOnly));
                    }
                }
            }
            Ok(SyncCmd::Disconnect) => {
                backend = None;
                let _ = clear_saved_tokens(&data_root);
                let _ = store.clear_all_remote_links();
                let _ = evt_tx.send(SyncEvent::TransfersUpdated {
                    throwaway: Vec::new(),
                    files: Vec::new(),
                });
                let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::LocalOnly));
            }
            Ok(SyncCmd::RefreshTransfers) => {
                if backend.is_none() && has_saved_tokens(&data_root) {
                    backend = GDriveBackend::open(data_root.clone()).ok();
                }
                if let Some(ref b) = backend {
                    refresh_transfers_now(b, &evt_tx, None);
                } else {
                    let _ = evt_tx.send(SyncEvent::TransferToast(
                        "Connect Google Drive in Settings first".to_string(),
                    ));
                }
            }
            Ok(SyncCmd::UploadTransfers { bucket, paths }) => {
                if backend.is_none() && has_saved_tokens(&data_root) {
                    backend = GDriveBackend::open(data_root.clone()).ok();
                }
                let Some(ref b) = backend else {
                    let _ = evt_tx.send(SyncEvent::TransferToast(
                        "Connect Google Drive in Settings first".to_string(),
                    ));
                    continue;
                };
                let mut ok = 0usize;
                let mut last_err = None;
                for path in &paths {
                    match b.upload_special_file(bucket.folder_name(), path) {
                        Ok(_) => ok += 1,
                        Err(e) => last_err = Some(e),
                    }
                }
                refresh_transfers_now(b, &evt_tx, None);
                if ok > 0 {
                    let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                        "Uploaded {ok} file(s) to {}",
                        bucket.label()
                    )));
                } else if let Some(err) = last_err {
                    let _ = evt_tx.send(SyncEvent::TransferToast(format!("Upload failed: {err}")));
                }
            }
            Ok(SyncCmd::UploadTransferBytes {
                bucket,
                filename,
                mime_type,
                bytes,
            }) => {
                if backend.is_none() && has_saved_tokens(&data_root) {
                    backend = GDriveBackend::open(data_root.clone()).ok();
                }
                let Some(ref b) = backend else {
                    let _ = evt_tx.send(SyncEvent::TransferToast(
                        "Connect Google Drive in Settings first".to_string(),
                    ));
                    continue;
                };
                match b.upload_special_bytes(bucket.folder_name(), &filename, &mime_type, &bytes) {
                    Ok(_) => {
                        refresh_transfers_now(b, &evt_tx, None);
                        let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                            "Uploaded {filename} to {}",
                            bucket.label()
                        )));
                    }
                    Err(e) => {
                        let _ = evt_tx.send(SyncEvent::TransferToast(format!("Upload failed: {e}")));
                    }
                }
            }
            Ok(SyncCmd::DownloadTransfer {
                bucket,
                file,
                dest_dir,
                auto_delete,
            }) => {
                let Some(ref b) = backend else {
                    let _ = evt_tx.send(SyncEvent::TransferToast(
                        "Connect Google Drive in Settings first".to_string(),
                    ));
                    continue;
                };
                match b.download_file(&file.remote_id) {
                    Ok(bytes) => {
                        if let Err(e) = fs::create_dir_all(&dest_dir) {
                            let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                                "Could not create {}: {e}",
                                dest_dir.display()
                            )));
                            continue;
                        }
                        let dest_path = unique_dest_path(&dest_dir, &file.filename);
                        match fs::write(&dest_path, &bytes) {
                            Ok(()) => {
                                if auto_delete {
                                    let _ = b.delete_item(&file.remote_id);
                                    refresh_transfers_now(b, &evt_tx, Some(&file.remote_id));
                                    let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                                        "Saved {} and cleared from {}",
                                        dest_path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or(&file.filename),
                                        bucket.label()
                                    )));
                                } else {
                                    let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                                        "Saved to {}",
                                        dest_path.display()
                                    )));
                                }
                            }
                            Err(e) => {
                                let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                                    "Write failed: {e}"
                                )));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                            "Download failed: {e}"
                        )));
                    }
                }
            }
            Ok(SyncCmd::DeleteTransfer {
                bucket,
                remote_id,
                filename,
            }) => {
                let Some(ref b) = backend else { continue };
                match b.delete_item(&remote_id) {
                    Ok(()) => {
                        refresh_transfers_now(b, &evt_tx, Some(&remote_id));
                        let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                            "Deleted {filename} from {}",
                            bucket.label()
                        )));
                    }
                    Err(e) => {
                        let _ = evt_tx.send(SyncEvent::TransferToast(format!(
                            "Delete failed: {e}"
                        )));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(ref b) = backend
                    && last_poll.elapsed() >= poll_interval
                {
                    run_sync_pass(&mut store, b, &evt_tx);
                    refresh_transfers_now(b, &evt_tx, None);
                    last_poll = Instant::now();
                }
            }
        }
    }
}

fn refresh_transfers_now(
    backend: &GDriveBackend,
    evt_tx: &Sender<SyncEvent>,
    exclude_id: Option<&str>,
) {
    let mut throwaway = backend
        .list_special_files(TransferBucket::Throwaway.folder_name())
        .unwrap_or_default();
    let mut files = backend
        .list_special_files(TransferBucket::Files.folder_name())
        .unwrap_or_default();
    if let Some(skip) = exclude_id {
        throwaway.retain(|f| f.remote_id != skip);
        files.retain(|f| f.remote_id != skip);
    }
    let _ = evt_tx.send(SyncEvent::TransfersUpdated { throwaway, files });
}

fn unique_dest_path(dir: &Path, filename: &str) -> PathBuf {
    let safe_name = Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("download.bin");
    let first = dir.join(safe_name);
    if !first.exists() {
        return first;
    }
    let stem = Path::new(safe_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = Path::new(safe_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    for idx in 1..1000 {
        let candidate = if ext.is_empty() {
            dir.join(format!("{stem} ({idx})"))
        } else {
            dir.join(format!("{stem} ({idx}).{ext}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

fn run_sync_pass(store: &mut Store, backend: &dyn StorageBackend, evt_tx: &Sender<SyncEvent>) {
    let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::Syncing));
    match sync_once(store, backend) {
        Ok(report) => {
            let summary = format!(
                "{} · +{} ↑ / +{} ↓",
                backend.backend_name(),
                report.uploaded,
                report.downloaded
            );
            let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::Synced(summary)));
            let _ = evt_tx.send(SyncEvent::SyncFinished(report));
        }
        Err(e) => {
            let _ = evt_tx.send(SyncEvent::StatusChanged(SyncStatus::Offline(e)));
        }
    }
}

pub fn sync_once(store: &mut Store, backend: &dyn StorageBackend) -> Result<SyncReport, String> {
    let mut report = SyncReport::default();

    // 1. Flush file tombstones first; defer folder tombstones until after moves complete
    // so merging a channel moves its remote files out before trashing the old folder.
    let mut pending_folder_tombstones = Vec::new();
    let mut pending_folder_ids: HashSet<String> = HashSet::new();
    for tomb in store.list_tombstones()? {
        if tomb.kind == "folder" {
            pending_folder_ids.insert(tomb.remote_id.clone());
            pending_folder_tombstones.push(tomb);
        } else {
            backend.delete_item(&tomb.remote_id)?;
            store.clear_tombstone(tomb.id)?;
            report.deleted_remote += 1;
        }
    }

    // 2. Reconcile channel folders under koma/<channel>.
    let remote_channels = backend.list_channels()?;
    let mut remote_by_id: HashMap<String, String> = HashMap::new();
    let mut remote_by_slug: HashMap<String, String> = HashMap::new();
    for rc in &remote_channels {
        if pending_folder_ids.contains(&rc.remote_id) {
            continue;
        }
        remote_by_id.insert(rc.remote_id.clone(), rc.slug.clone());
        if let Ok(norm) = slugify(&rc.slug) {
            remote_by_slug.insert(norm, rc.remote_id.clone());
        }
    }

    let local_channels = store.list_channels_sync()?;
    let mut known_remote_folder_ids: HashSet<String> = HashSet::new();

    for lc in &local_channels {
        if let Some(ref rid) = lc.remote_id {
            if let Some(remote_slug) = remote_by_id.get(rid) {
                if remote_slug != &lc.slug {
                    backend.rename_folder(rid, &lc.slug)?;
                    report.updated += 1;
                }
                known_remote_folder_ids.insert(rid.clone());
                continue;
            }
            // Previously linked folder vanished remotely; re-ensure by slug.
        }

        let folder_id = if let Some(existing_id) = remote_by_slug.get(&lc.slug) {
            existing_id.clone()
        } else {
            backend.ensure_channel_folder(&lc.slug)?
        };
        store.set_channel_remote_id(lc.id, &folder_id)?;
        known_remote_folder_ids.insert(folder_id);
    }

    // Create local channels for any new remote folders added from another device.
    for rc in &remote_channels {
        if pending_folder_ids.contains(&rc.remote_id) {
            continue;
        }
        if !known_remote_folder_ids.contains(&rc.remote_id)
            && let Ok(ch) = store.create_channel(&rc.slug)
        {
            store.set_channel_remote_id(ch.id, &rc.remote_id)?;
            known_remote_folder_ids.insert(rc.remote_id.clone());
        }
    }

    // Reload synchronized channel map (id -> (slug, remote_id)).
    let channels_after = store.list_channels_sync()?;
    let mut remote_folder_to_slug: HashMap<String, String> = HashMap::new();
    for ch in &channels_after {
        if let Some(ref rid) = ch.remote_id {
            remote_folder_to_slug.insert(rid.clone(), ch.slug.clone());
        }
    }

    // 3. Gather current remote files across active and pending-tombstone channel folders
    // (including pending-tombstone folders lets merged channel images move out cleanly).
    let mut folder_ids_to_scan: HashSet<String> = remote_folder_to_slug.keys().cloned().collect();
    folder_ids_to_scan.extend(pending_folder_ids.iter().cloned());

    let mut remote_files_by_id: HashMap<String, RemoteFile> = HashMap::new();
    let mut remote_files_by_key: HashMap<(String, String), RemoteFile> = HashMap::new();
    for folder_id in &folder_ids_to_scan {
        for rf in backend.list_files(folder_id)? {
            remote_files_by_key.insert(
                (rf.channel_remote_id.clone(), rf.filename.clone()),
                rf.clone(),
            );
            remote_files_by_id.insert(rf.remote_id.clone(), rf);
        }
    }

    // 4. Push unsynced local images, propagate renames/moves, and mirror remote deletions.
    let local_images = store.list_images_sync()?;
    let mut local_remote_ids: HashSet<String> = HashSet::new();

    for img in &local_images {
        let Some(ref target_folder) = img.channel_remote_id else {
            continue;
        };

        if let Some(ref rid) = img.remote_id {
            if let Some(rf) = remote_files_by_id.get(rid) {
                local_remote_ids.insert(rid.clone());
                let name_changed = rf.filename != img.filename;
                let folder_changed = &rf.channel_remote_id != target_folder;
                if name_changed || folder_changed {
                    let new_name = name_changed.then_some(img.filename.as_str());
                    let move_parents = folder_changed
                        .then_some((rf.channel_remote_id.as_str(), target_folder.as_str()));
                    backend.update_file(rid, new_name, move_parents)?;
                    report.updated += 1;
                }
            } else {
                // File was synced before, then deleted on Drive / phone -> remove locally.
                if store.delete_image_by_remote_id(rid)? {
                    report.deleted_local += 1;
                }
            }
            continue;
        }

        // Unsynced local file: link if exact filename already exists in remote folder, else upload.
        let key = (target_folder.clone(), img.filename.clone());
        if let Some(existing) = remote_files_by_key.get(&key) {
            store.set_image_remote_id(img.id, &existing.remote_id)?;
            local_remote_ids.insert(existing.remote_id.clone());
            report.linked += 1;
        } else {
            let local_abs = store.root().join(&img.rel_path);
            if !local_abs.is_file() {
                continue;
            }
            let uploaded = backend.upload_file(
                target_folder,
                &img.filename,
                &img.mime_type,
                &local_abs,
            )?;
            store.set_image_remote_id(img.id, &uploaded.remote_id)?;
            local_remote_ids.insert(uploaded.remote_id);
            report.uploaded += 1;
        }
    }

    // 5. Pull any remote-only image files into local disk + SQLite (generating WebP thumbnails).
    for rf in remote_files_by_id.values() {
        if local_remote_ids.contains(&rf.remote_id)
            || pending_folder_ids.contains(&rf.channel_remote_id)
        {
            continue;
        }
        if !looks_like_image(&rf.mime_type, &rf.filename) {
            continue;
        }
        let Some(slug) = remote_folder_to_slug.get(&rf.channel_remote_id) else {
            continue;
        };
        let bytes = backend.download_file(&rf.remote_id)?;
        store.ingest_remote_bytes(slug, &rf.filename, &bytes, &rf.remote_id, None)?;
        report.downloaded += 1;
    }

    // 6. Now trash any deleted/merged channel folders safely.
    for tomb in pending_folder_tombstones {
        backend.delete_item(&tomb.remote_id)?;
        store.clear_tombstone(tomb.id)?;
        report.deleted_remote += 1;
    }

    Ok(report)
}

fn looks_like_image(mime: &str, filename: &str) -> bool {
    if mime.starts_with("image/") {
        return true;
    }
    let lower = filename.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::RemoteChannel;
    use image::{ImageBuffer, Rgba};
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct FakeState {
        next_id: usize,
        folders: HashMap<String, String>,              // id -> slug
        files: HashMap<String, (RemoteFile, Vec<u8>)>, // id -> (meta, bytes)
    }

    struct FakeBackend {
        state: Mutex<FakeState>,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                state: Mutex::new(FakeState::default()),
            }
        }

        fn seed_remote_image(&self, slug: &str, filename: &str, bytes: Vec<u8>) -> String {
            let folder_id = self.ensure_channel_folder(slug).unwrap();
            let mut st = self.state.lock().unwrap();
            st.next_id += 1;
            let fid = format!("rf_{}", st.next_id);
            st.files.insert(
                fid.clone(),
                (
                    RemoteFile {
                        remote_id: fid.clone(),
                        channel_remote_id: folder_id,
                        filename: filename.to_string(),
                        mime_type: "image/png".to_string(),
                    },
                    bytes,
                ),
            );
            fid
        }
    }

    impl StorageBackend for FakeBackend {
        fn backend_name(&self) -> &'static str {
            "FakeDrive"
        }

        fn ensure_channel_folder(&self, slug: &str) -> Result<String, String> {
            let mut st = self.state.lock().unwrap();
            if let Some((id, _)) = st.folders.iter().find(|(_, s)| s.as_str() == slug) {
                return Ok(id.clone());
            }
            st.next_id += 1;
            let id = format!("dir_{}", st.next_id);
            st.folders.insert(id.clone(), slug.to_string());
            Ok(id)
        }

        fn list_channels(&self) -> Result<Vec<RemoteChannel>, String> {
            let st = self.state.lock().unwrap();
            Ok(st
                .folders
                .iter()
                .map(|(id, slug)| RemoteChannel {
                    remote_id: id.clone(),
                    slug: slug.clone(),
                })
                .collect())
        }

        fn list_files(&self, channel_remote_id: &str) -> Result<Vec<RemoteFile>, String> {
            let st = self.state.lock().unwrap();
            Ok(st
                .files
                .values()
                .filter(|(f, _)| f.channel_remote_id == channel_remote_id)
                .map(|(f, _)| f.clone())
                .collect())
        }

        fn upload_file(
            &self,
            channel_remote_id: &str,
            filename: &str,
            mime_type: &str,
            local_path: &Path,
        ) -> Result<RemoteFile, String> {
            let bytes = std::fs::read(local_path).map_err(|e| e.to_string())?;
            let mut st = self.state.lock().unwrap();
            st.next_id += 1;
            let id = format!("rf_{}", st.next_id);
            let rf = RemoteFile {
                remote_id: id.clone(),
                channel_remote_id: channel_remote_id.to_string(),
                filename: filename.to_string(),
                mime_type: mime_type.to_string(),
            };
            st.files.insert(id, (rf.clone(), bytes));
            Ok(rf)
        }

        fn download_file(&self, remote_id: &str) -> Result<Vec<u8>, String> {
            let st = self.state.lock().unwrap();
            st.files
                .get(remote_id)
                .map(|(_, b)| b.clone())
                .ok_or_else(|| format!("missing remote file {remote_id}"))
        }

        fn update_file(
            &self,
            remote_id: &str,
            new_filename: Option<&str>,
            move_parents: Option<(&str, &str)>,
        ) -> Result<(), String> {
            let mut st = self.state.lock().unwrap();
            let (rf, _) = st
                .files
                .get_mut(remote_id)
                .ok_or_else(|| format!("missing remote file {remote_id}"))?;
            if let Some(name) = new_filename {
                rf.filename = name.to_string();
            }
            if let Some((_, new_parent)) = move_parents {
                rf.channel_remote_id = new_parent.to_string();
            }
            Ok(())
        }

        fn rename_folder(&self, remote_id: &str, new_slug: &str) -> Result<(), String> {
            let mut st = self.state.lock().unwrap();
            let entry = st
                .folders
                .get_mut(remote_id)
                .ok_or_else(|| format!("missing folder {remote_id}"))?;
            *entry = new_slug.to_string();
            Ok(())
        }

        fn delete_item(&self, remote_id: &str) -> Result<(), String> {
            let mut st = self.state.lock().unwrap();
            st.files.remove(remote_id);
            st.folders.remove(remote_id);
            Ok(())
        }
    }

    fn sample_png() -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(24, 18, Rgba([120, 180, 240, 255]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn two_way_sync_handles_push_pull_rename_move_and_deletes() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("koma-sync-test-{nanos}"));
        let mut store = Store::open(root.clone()).unwrap();
        let backend = FakeBackend::new();
        let png = sample_png();

        // 1. Local paste -> push to remote
        let local_img = store.ingest_bytes("rei", &png, Some(1000)).unwrap();
        let r1 = sync_once(&mut store, &backend).unwrap();
        assert_eq!(r1.uploaded, 1);
        assert_eq!(r1.downloaded, 0);

        // 2. Idempotent second sync
        let r2 = sync_once(&mut store, &backend).unwrap();
        assert_eq!(r2, SyncReport::default());

        // 3. Remote upload from phone -> pulls into local store with WebP thumb
        let remote_id = backend.seed_remote_image("monogatari", "monogatari_0001.png", png.clone());
        let r3 = sync_once(&mut store, &backend).unwrap();
        assert_eq!(r3.downloaded, 1);
        let mono = store.create_channel("monogatari").unwrap();
        let mono_imgs = store.list_images(Some(mono.id)).unwrap();
        assert_eq!(mono_imgs.len(), 1);
        assert!(root.join(&mono_imgs[0].thumb_path).exists());

        // 4. Local rename + move propagate to remote
        store.rename_image(local_img.id, "rei_fav.png").unwrap();
        store.move_images(&[local_img.id], mono.id).unwrap();
        let r4 = sync_once(&mut store, &backend).unwrap();
        assert_eq!(r4.updated, 1);
        let remote_mono_files = backend
            .list_files(&backend.ensure_channel_folder("monogatari").unwrap())
            .unwrap();
        assert_eq!(remote_mono_files.len(), 2);

        // 5. Local delete propagates via tombstone
        store.delete_images(&[local_img.id]).unwrap();
        let r5 = sync_once(&mut store, &backend).unwrap();
        assert_eq!(r5.deleted_remote, 1);

        // 6. Remote delete mirrors back to local
        backend.delete_item(&remote_id).unwrap();
        let r6 = sync_once(&mut store, &backend).unwrap();
        assert_eq!(r6.deleted_local, 1);
        assert_eq!(store.list_images(Some(mono.id)).unwrap().len(), 0);

        // 7. Merging a synced channel moves its remote files out before trashing old folder
        let extra = store.ingest_bytes("extra", &png, Some(2000)).unwrap();
        assert_eq!(sync_once(&mut store, &backend).unwrap().uploaded, 1);
        store.merge_channels(extra.channel_id, mono.id).unwrap();
        let r7 = sync_once(&mut store, &backend).unwrap();
        assert_eq!(r7.updated, 1);
        assert_eq!(r7.deleted_remote, 1);
        assert_eq!(store.list_images(Some(mono.id)).unwrap().len(), 1);
        let remote_mono_after = backend
            .list_files(&backend.ensure_channel_folder("monogatari").unwrap())
            .unwrap();
        assert_eq!(remote_mono_after.len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }
}
