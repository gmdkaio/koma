pub mod gdrive;
pub mod sync;

use std::path::Path;

#[derive(Debug, Clone)]
pub struct RemoteChannel {
    pub remote_id: String,
    pub slug: String,
}

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub remote_id: String,
    pub channel_remote_id: String,
    pub filename: String,
    pub mime_type: String,
}

pub trait StorageBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn ensure_channel_folder(&self, slug: &str) -> Result<String, String>;
    fn list_channels(&self) -> Result<Vec<RemoteChannel>, String>;
    fn list_files(&self, channel_remote_id: &str) -> Result<Vec<RemoteFile>, String>;
    fn upload_file(
        &self,
        channel_remote_id: &str,
        filename: &str,
        mime_type: &str,
        local_path: &Path,
    ) -> Result<RemoteFile, String>;
    fn download_file(&self, remote_id: &str) -> Result<Vec<u8>, String>;
    fn update_file(
        &self,
        remote_id: &str,
        new_filename: Option<&str>,
        move_parents: Option<(&str, &str)>,
    ) -> Result<(), String>;
    fn rename_folder(&self, remote_id: &str, new_slug: &str) -> Result<(), String>;
    fn delete_item(&self, remote_id: &str) -> Result<(), String>;
}
