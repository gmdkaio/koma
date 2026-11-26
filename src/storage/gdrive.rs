use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::storage::{RemoteChannel, RemoteFile, StorageBackend};

const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3/files";
const OAUTH_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const ROOT_FOLDER_NAME: &str = "koma";
const MAX_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct TransferFile {
    pub remote_id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub created_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GDriveClientConfig {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenState {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Deserialize)]
struct GoogleConsoleJson {
    installed: Option<GDriveClientConfig>,
    web: Option<GDriveClientConfig>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    3600
}

#[derive(Debug, Deserialize)]
struct DriveFileItem {
    id: String,
    name: String,
    #[serde(rename = "mimeType", default)]
    mime_type: String,
    #[serde(default)]
    parents: Option<Vec<String>>,
    #[serde(default)]
    size: Option<String>,
    #[serde(rename = "createdTime", default)]
    created_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DriveFileList {
    #[serde(default)]
    files: Vec<DriveFileItem>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

pub fn default_client_config_path() -> Result<PathBuf, String> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("koma/gdrive_client.json"));
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "neither XDG_CONFIG_HOME nor HOME is set".to_string())?;
    Ok(PathBuf::from(home).join(".config/koma/gdrive_client.json"))
}

pub fn token_file_path(data_root: &Path) -> PathBuf {
    data_root.join("gdrive_tokens.json")
}

pub fn parse_client_json(raw: &str) -> Result<GDriveClientConfig, String> {
    let parsed: GoogleConsoleJson =
        serde_json::from_str(raw).map_err(|e| format!("invalid Google OAuth JSON: {e}"))?;
    let cfg = if let Some(c) = parsed.installed {
        c
    } else if let Some(c) = parsed.web {
        c
    } else if let Some(id) = parsed.client_id {
        GDriveClientConfig {
            client_id: id,
            client_secret: parsed.client_secret.unwrap_or_default(),
        }
    } else {
        return Err("OAuth JSON missing client_id (expected Google Desktop client JSON)".to_string());
    };
    if cfg.client_id.trim().is_empty() {
        return Err("OAuth client_id is empty".to_string());
    }
    Ok(cfg)
}

impl GDriveClientConfig {
    pub fn load() -> Result<Self, String> {
        if let Ok(id) = std::env::var("KOMA_GDRIVE_CLIENT_ID")
            && !id.trim().is_empty()
        {
            return Ok(Self {
                client_id: id.trim().to_string(),
                client_secret: std::env::var("KOMA_GDRIVE_CLIENT_SECRET")
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            });
        }
        let path = default_client_config_path()?;
        let raw = fs::read_to_string(&path).map_err(|_| {
            format!(
                "Google Drive OAuth client not configured (import JSON or save to {})",
                path.display()
            )
        })?;
        parse_client_json(&raw)
    }

    pub fn is_configured() -> bool {
        Self::load().is_ok()
    }
}

pub fn import_client_json(source: &Path) -> Result<PathBuf, String> {
    let raw = fs::read_to_string(source)
        .map_err(|e| format!("failed to read {}: {e}", source.display()))?;
    let cfg = parse_client_json(&raw)?;
    let dest = default_client_config_path()?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create config dir: {e}"))?;
    }
    let normalized = serde_json::to_string_pretty(&cfg)
        .map_err(|e| format!("failed to serialize client config: {e}"))?;
    write_private_file(&dest, normalized.as_bytes())?;
    Ok(dest)
}

pub fn has_saved_tokens(data_root: &Path) -> bool {
    token_file_path(data_root).is_file()
}

pub fn clear_saved_tokens(data_root: &Path) -> Result<(), String> {
    let path = token_file_path(data_root);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("failed to remove tokens: {e}"))?;
    }
    Ok(())
}

pub fn authorize_interactive(data_root: &Path) -> Result<TokenState, String> {
    let cfg = GDriveClientConfig::load()?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("failed to bind OAuth callback listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to read callback port: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let state = random_hex(16)?;

    let auth_url = format!(
        "{OAUTH_AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
        url_encode(&cfg.client_id),
        url_encode(&redirect_uri),
        url_encode(DRIVE_SCOPE),
        url_encode(&state),
    );

    open_in_browser(&auth_url)?;

    listener
        .set_nonblocking(false)
        .map_err(|e| format!("failed to configure callback socket: {e}"))?;
    let (mut stream, _) = listener
        .accept()
        .map_err(|e| format!("OAuth callback wait failed: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));

    let mut buf = [0u8; 4096];
    let n = stream
        .read(&mut buf)
        .map_err(|e| format!("failed to read OAuth callback request: {e}"))?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let target = req
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "malformed OAuth callback request".to_string())?;

    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params = parse_query(query);

    let send_html = |stream: &mut std::net::TcpStream, status: &str, body: &str| {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    };

    if let Some(err) = params.iter().find(|(k, _)| k == "error").map(|(_, v)| v) {
        send_html(
            &mut stream,
            "400 Bad Request",
            "<html><body style='font-family:sans-serif;background:#111;color:#eee;padding:40px'><h3>koma authorization cancelled</h3><p>You can close this tab.</p></body></html>",
        );
        return Err(format!("Google OAuth denied: {err}"));
    }

    let got_state = params
        .iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    if got_state != state {
        send_html(
            &mut stream,
            "400 Bad Request",
            "<html><body>OAuth state mismatch.</body></html>",
        );
        return Err("OAuth state mismatch".to_string());
    }

    let code = params
        .iter()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| "OAuth callback missing code".to_string())?;

    let agent = build_agent();
    let mut form: Vec<(&str, &str)> = vec![
        ("code", code.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];
    if !cfg.client_secret.is_empty() {
        form.push(("client_secret", cfg.client_secret.as_str()));
    }

    let token_res: TokenResponse = agent
        .post(OAUTH_TOKEN_URL)
        .send_form(form)
        .map_err(|e| format!("token exchange failed: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("failed to parse token response: {e}"))?;

    let refresh_token = token_res.refresh_token.ok_or_else(|| {
        "Google did not return a refresh_token; revoke app access in Google Account and reconnect"
            .to_string()
    })?;

    let tokens = TokenState {
        access_token: token_res.access_token,
        refresh_token,
        expires_at: now_unix_u64() + token_res.expires_in,
    };
    save_tokens(data_root, &tokens)?;

    send_html(
        &mut stream,
        "200 OK",
        "<html><body style='font-family:sans-serif;background:#0e1014;color:#e6e9ef;padding:48px;text-align:center'><h2>koma connected to Google Drive</h2><p style='opacity:.7'>You can close this tab and return to koma.</p></body></html>",
    );
    Ok(tokens)
}

pub struct GDriveBackend {
    agent: Agent,
    cfg: GDriveClientConfig,
    data_root: PathBuf,
    tokens: Mutex<TokenState>,
    root_folder_id: Mutex<Option<String>>,
}

impl GDriveBackend {
    pub fn open(data_root: PathBuf) -> Result<Self, String> {
        let cfg = GDriveClientConfig::load()?;
        let raw = fs::read_to_string(token_file_path(&data_root))
            .map_err(|_| "not signed in to Google Drive".to_string())?;
        let tokens: TokenState =
            serde_json::from_str(&raw).map_err(|e| format!("corrupt Drive token file: {e}"))?;
        Ok(Self {
            agent: build_agent(),
            cfg,
            data_root,
            tokens: Mutex::new(tokens),
            root_folder_id: Mutex::new(None),
        })
    }

    fn access_token(&self) -> Result<String, String> {
        let mut guard = self
            .tokens
            .lock()
            .map_err(|_| "token lock poisoned".to_string())?;
        let now = now_unix_u64();
        if now + 60 < guard.expires_at && !guard.access_token.is_empty() {
            return Ok(guard.access_token.clone());
        }

        let mut form: Vec<(&str, &str)> = vec![
            ("refresh_token", guard.refresh_token.as_str()),
            ("client_id", self.cfg.client_id.as_str()),
            ("grant_type", "refresh_token"),
        ];
        if !self.cfg.client_secret.is_empty() {
            form.push(("client_secret", self.cfg.client_secret.as_str()));
        }

        let refreshed: TokenResponse = self
            .agent
            .post(OAUTH_TOKEN_URL)
            .send_form(form)
            .map_err(|e| format!("Drive token refresh failed: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| format!("failed to parse refreshed token: {e}"))?;

        guard.access_token = refreshed.access_token;
        guard.expires_at = now_unix_u64() + refreshed.expires_in;
        if let Some(rt) = refreshed.refresh_token {
            guard.refresh_token = rt;
        }
        save_tokens(&self.data_root, &guard)?;
        Ok(guard.access_token.clone())
    }

    fn auth_header(&self) -> Result<String, String> {
        Ok(format!("Bearer {}", self.access_token()?))
    }

    fn ensure_root_folder(&self) -> Result<String, String> {
        {
            let guard = self
                .root_folder_id
                .lock()
                .map_err(|_| "root folder lock poisoned".to_string())?;
            if let Some(id) = guard.as_ref() {
                return Ok(id.clone());
            }
        }

        let q = format!(
            "mimeType = '{FOLDER_MIME}' and name = '{}' and 'root' in parents and trashed = false",
            escape_query(ROOT_FOLDER_NAME)
        );
        let existing = self.query_files(&q)?;
        let id = if let Some(first) = existing.into_iter().next() {
            first.id
        } else {
            self.create_folder(ROOT_FOLDER_NAME, "root")?
        };

        let mut guard = self
            .root_folder_id
            .lock()
            .map_err(|_| "root folder lock poisoned".to_string())?;
        *guard = Some(id.clone());
        Ok(id)
    }

    fn create_folder(&self, name: &str, parent_id: &str) -> Result<String, String> {
        let auth = self.auth_header()?;
        let body = serde_json::json!({
            "name": name,
            "mimeType": FOLDER_MIME,
            "parents": [parent_id],
        });
        let created: DriveFileItem = self
            .agent
            .post(DRIVE_API_BASE)
            .header("Authorization", &auth)
            .query("fields", "id,name,mimeType")
            .send_json(&body)
            .map_err(|e| format!("Drive create folder {name} failed: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| format!("Drive parse folder response failed: {e}"))?;
        Ok(created.id)
    }

    fn query_files(&self, q: &str) -> Result<Vec<DriveFileItem>, String> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let auth = self.auth_header()?;
            let mut req = self
                .agent
                .get(DRIVE_API_BASE)
                .header("Authorization", &auth)
                .query("q", q)
                .query("spaces", "drive")
                .query("pageSize", "1000")
                .query("fields", "nextPageToken,files(id,name,mimeType,parents,size,createdTime)");
            if let Some(ref tok) = page_token {
                req = req.query("pageToken", tok);
            }

            let list: DriveFileList = req
                .call()
                .map_err(|e| format!("Drive list failed: {e}"))?
                .body_mut()
                .read_json()
                .map_err(|e| format!("Drive parse list failed: {e}"))?;

            out.extend(list.files);
            match list.next_page_token {
                Some(tok) if !tok.is_empty() => page_token = Some(tok),
                _ => break,
            }
        }
        Ok(out)
    }

    pub fn list_special_files(&self, folder_name: &str) -> Result<Vec<TransferFile>, String> {
        let root_id = self.ensure_root_folder()?;
        let fq = format!(
            "mimeType = '{FOLDER_MIME}' and name = '{}' and '{}' in parents and trashed = false",
            escape_query(folder_name),
            escape_query(&root_id)
        );
        let Some(folder) = self.query_files(&fq)?.into_iter().next() else {
            return Ok(Vec::new());
        };
        let q = format!(
            "mimeType != '{FOLDER_MIME}' and '{}' in parents and trashed = false",
            escape_query(&folder.id)
        );
        let mut items: Vec<TransferFile> = self
            .query_files(&q)?
            .into_iter()
            .map(|f| TransferFile {
                remote_id: f.id,
                filename: f.name,
                size_bytes: f
                    .size
                    .as_deref()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0),
                created_time: f.created_time.unwrap_or_default(),
            })
            .collect();
        items.sort_by(|a, b| b.created_time.cmp(&a.created_time));
        Ok(items)
    }

    pub fn upload_special_file(
        &self,
        folder_name: &str,
        local_path: &Path,
    ) -> Result<TransferFile, String> {
        let filename = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| "invalid file name".to_string())?;
        let bytes = fs::read(local_path)
            .map_err(|e| format!("failed to read {}: {e}", local_path.display()))?;
        let mime = guess_mime_from_path(local_path);
        self.upload_special_bytes(folder_name, filename, mime, &bytes)
    }

    pub fn upload_special_bytes(
        &self,
        folder_name: &str,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<TransferFile, String> {
        let folder_id = self.ensure_channel_folder(folder_name)?;
        let auth = self.auth_header()?;
        let boundary = format!("koma_boundary_{}", random_hex(12)?);

        let meta_json = serde_json::json!({
            "name": filename,
            "parents": [folder_id],
        })
        .to_string();

        let mut body = Vec::with_capacity(bytes.len() + meta_json.len() + 256);
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
        body.extend_from_slice(meta_json.as_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(format!("Content-Type: {mime_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let ct = format!("multipart/related; boundary={boundary}");
        let uploaded: DriveFileItem = self
            .agent
            .post(DRIVE_UPLOAD_BASE)
            .header("Authorization", &auth)
            .header("Content-Type", &ct)
            .query("uploadType", "multipart")
            .query("fields", "id,name,mimeType,size,createdTime")
            .send(body.as_slice())
            .map_err(|e| format!("Drive upload {filename} failed: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| format!("Drive parse upload response failed: {e}"))?;

        Ok(TransferFile {
            remote_id: uploaded.id,
            filename: uploaded.name,
            size_bytes: uploaded
                .size
                .as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(bytes.len() as u64),
            created_time: uploaded.created_time.unwrap_or_default(),
        })
    }
}

pub fn guess_mime_from_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "7z" => "application/x-7z-compressed",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "apk" => "application/vnd.android.package-archive",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

impl StorageBackend for GDriveBackend {
    fn backend_name(&self) -> &'static str {
        "Google Drive"
    }

    fn ensure_channel_folder(&self, slug: &str) -> Result<String, String> {
        let root_id = self.ensure_root_folder()?;
        let q = format!(
            "mimeType = '{FOLDER_MIME}' and name = '{}' and '{}' in parents and trashed = false",
            escape_query(slug),
            escape_query(&root_id)
        );
        if let Some(existing) = self.query_files(&q)?.into_iter().next() {
            return Ok(existing.id);
        }
        self.create_folder(slug, &root_id)
    }

    fn list_channels(&self) -> Result<Vec<RemoteChannel>, String> {
        let root_id = self.ensure_root_folder()?;
        let q = format!(
            "mimeType = '{FOLDER_MIME}' and '{}' in parents and trashed = false",
            escape_query(&root_id)
        );
        let folders = self.query_files(&q)?;
        Ok(folders
            .into_iter()
            .filter(|f| !f.name.starts_with('.'))
            .map(|f| RemoteChannel {
                remote_id: f.id,
                slug: f.name,
            })
            .collect())
    }

    fn list_files(&self, channel_remote_id: &str) -> Result<Vec<RemoteFile>, String> {
        let q = format!(
            "mimeType != '{FOLDER_MIME}' and '{}' in parents and trashed = false",
            escape_query(channel_remote_id)
        );
        let items = self.query_files(&q)?;
        Ok(items
            .into_iter()
            .map(|f| RemoteFile {
                remote_id: f.id,
                channel_remote_id: f
                    .parents
                    .and_then(|p| p.into_iter().next())
                    .unwrap_or_else(|| channel_remote_id.to_string()),
                filename: f.name,
                mime_type: f.mime_type,
            })
            .collect())
    }

    fn upload_file(
        &self,
        channel_remote_id: &str,
        filename: &str,
        mime_type: &str,
        local_path: &Path,
    ) -> Result<RemoteFile, String> {
        let bytes = fs::read(local_path)
            .map_err(|e| format!("failed to read {}: {e}", local_path.display()))?;
        let auth = self.auth_header()?;
        let boundary = format!("koma_boundary_{}", random_hex(12)?);

        let meta_json = serde_json::json!({
            "name": filename,
            "parents": [channel_remote_id],
        })
        .to_string();

        let mut body = Vec::with_capacity(bytes.len() + meta_json.len() + 256);
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
        body.extend_from_slice(meta_json.as_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(format!("Content-Type: {mime_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let ct = format!("multipart/related; boundary={boundary}");
        let uploaded: DriveFileItem = self
            .agent
            .post(DRIVE_UPLOAD_BASE)
            .header("Authorization", &auth)
            .header("Content-Type", &ct)
            .query("uploadType", "multipart")
            .query("fields", "id,name,mimeType,parents")
            .send(body.as_slice())
            .map_err(|e| format!("Drive upload {filename} failed: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| format!("Drive parse upload response failed: {e}"))?;

        Ok(RemoteFile {
            remote_id: uploaded.id,
            channel_remote_id: channel_remote_id.to_string(),
            filename: uploaded.name,
            mime_type: uploaded.mime_type,
        })
    }

    fn download_file(&self, remote_id: &str) -> Result<Vec<u8>, String> {
        let auth = self.auth_header()?;
        let url = format!("{DRIVE_API_BASE}/{remote_id}");
        self.agent
            .get(&url)
            .header("Authorization", &auth)
            .query("alt", "media")
            .call()
            .map_err(|e| format!("Drive download {remote_id} failed: {e}"))?
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD_BYTES)
            .read_to_vec()
            .map_err(|e| format!("Drive read body {remote_id} failed: {e}"))
    }

    fn update_file(
        &self,
        remote_id: &str,
        new_filename: Option<&str>,
        move_parents: Option<(&str, &str)>,
    ) -> Result<(), String> {
        let auth = self.auth_header()?;
        let url = format!("{DRIVE_API_BASE}/{remote_id}");
        let mut req = self.agent.patch(&url).header("Authorization", &auth);
        if let Some((old_parent, new_parent)) = move_parents {
            req = req
                .query("removeParents", old_parent)
                .query("addParents", new_parent);
        }
        let body = if let Some(name) = new_filename {
            serde_json::json!({ "name": name })
        } else {
            serde_json::json!({})
        };
        req.send_json(&body)
            .map_err(|e| format!("Drive update {remote_id} failed: {e}"))?;
        Ok(())
    }

    fn rename_folder(&self, remote_id: &str, new_slug: &str) -> Result<(), String> {
        self.update_file(remote_id, Some(new_slug), None)
    }

    fn delete_item(&self, remote_id: &str) -> Result<(), String> {
        let auth = self.auth_header()?;
        let url = format!("{DRIVE_API_BASE}/{remote_id}");
        // Trash instead of hard-deleting so Drive's 30-day trash provides an undo safety net.
        let res = self
            .agent
            .patch(&url)
            .header("Authorization", &auth)
            .send_json(&serde_json::json!({ "trashed": true }));
        match res {
            Ok(_) => Ok(()),
            Err(ureq::Error::StatusCode(404)) => Ok(()),
            Err(e) => Err(format!("Drive trash {remote_id} failed: {e}")),
        }
    }
}

fn build_agent() -> Agent {
    Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into()
}

fn save_tokens(data_root: &Path, tokens: &TokenState) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("failed to serialize tokens: {e}"))?;
    write_private_file(&token_file_path(data_root), raw.as_bytes())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    Ok(())
}

fn open_in_browser(url: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map_err(|e| format!("failed to launch browser via xdg-open: {e}"))?;
    Ok(())
}

fn now_unix_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn escape_query(val: &str) -> String {
    val.replace('\\', "\\\\").replace('\'', "\\'")
}

fn random_hex(num_bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; num_bytes];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| format!("failed to read /dev/urandom: {e}"))?;
    let mut out = String::with_capacity(num_bytes * 2);
    for b in buf {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(pair), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_client_json_supports_installed_and_flat_formats() {
        let installed = r#"{"installed":{"client_id":"abc.apps.googleusercontent.com","client_secret":"sec123"}}"#;
        let cfg = parse_client_json(installed).unwrap();
        assert_eq!(cfg.client_id, "abc.apps.googleusercontent.com");
        assert_eq!(cfg.client_secret, "sec123");

        let flat = r#"{"client_id":"flat-id"}"#;
        let cfg2 = parse_client_json(flat).unwrap();
        assert_eq!(cfg2.client_id, "flat-id");
        assert_eq!(cfg2.client_secret, "");

        assert!(parse_client_json(r#"{"installed":{"client_id":""}}"#).is_err());
        assert!(parse_client_json(r#"{}"#).is_err());
    }
}
