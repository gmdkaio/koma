use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};

use crate::media;

const THUMB_CACHE_LIMIT: usize = 520;
const WORKER_THREADS: usize = 3;

pub struct ThumbCache {
    map: HashMap<PathBuf, gdk::Texture>,
    order: VecDeque<PathBuf>,
}

impl ThumbCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn get(&self, path: &Path) -> Option<gdk::Texture> {
        self.map.get(path).cloned()
    }

    pub fn insert(&mut self, path: PathBuf, tex: gdk::Texture) {
        if self.map.contains_key(&path) {
            return;
        }
        if self.map.len() >= THUMB_CACHE_LIMIT
            && let Some(oldest) = self.order.pop_front()
        {
            self.map.remove(&oldest);
        }
        self.order.push_back(path.clone());
        self.map.insert(path, tex);
    }

    pub fn get_or_load(&mut self, path: &Path) -> Option<gdk::Texture> {
        if let Some(tex) = self.map.get(path) {
            return Some(tex.clone());
        }
        let tex = load_texture(path)?;
        self.insert(path.to_path_buf(), tex.clone());
        Some(tex)
    }
}

struct DecodeTask {
    generation: u64,
    path: PathBuf,
}

struct SharedQueue {
    tasks: Mutex<VecDeque<DecodeTask>>,
    cvar: Condvar,
    generation: AtomicU64,
}

type WaiterMap = Rc<RefCell<HashMap<PathBuf, Vec<glib::WeakRef<gtk4::Picture>>>>>;

thread_local! {
    static ACTIVE_LOADER: RefCell<Option<Weak<AsyncThumbLoader>>> = const { RefCell::new(None) };
}

pub struct AsyncThumbLoader {
    cache: Rc<RefCell<ThumbCache>>,
    queue: Arc<SharedQueue>,
    waiters: WaiterMap,
    in_flight: Rc<RefCell<HashSet<PathBuf>>>,
}

impl AsyncThumbLoader {
    pub fn new(cache: Rc<RefCell<ThumbCache>>) -> Rc<Self> {
        let queue = Arc::new(SharedQueue {
            tasks: Mutex::new(VecDeque::new()),
            cvar: Condvar::new(),
            generation: AtomicU64::new(1),
        });
        let waiters: WaiterMap = Rc::new(RefCell::new(HashMap::new()));
        let in_flight: Rc<RefCell<HashSet<PathBuf>>> = Rc::new(RefCell::new(HashSet::new()));

        let loader = Rc::new(Self {
            cache,
            queue: queue.clone(),
            waiters,
            in_flight,
        });

        ACTIVE_LOADER.with(|slot| {
            *slot.borrow_mut() = Some(Rc::downgrade(&loader));
        });

        for _ in 0..WORKER_THREADS {
            let q = queue.clone();
            thread::spawn(move || {
                loop {
                    let task = {
                        let mut guard = q.tasks.lock().unwrap();
                        while guard.is_empty() {
                            guard = q.cvar.wait(guard).unwrap();
                        }
                        // FIFO keeps top-of-viewport and newly uploaded images decoding first.
                        guard.pop_front()
                    };
                    let Some(task) = task else { continue };
                    if task.generation != q.generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    let decoded = decode_fast_rgba(&task.path);
                    let path = task.path;
                    glib::MainContext::default().invoke(move || {
                        ACTIVE_LOADER.with(|slot| {
                            if let Some(loader) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                                loader.deliver(path, decoded);
                            }
                        });
                    });
                }
            });
        }

        loader
    }

    fn deliver(&self, path: PathBuf, decoded: Option<(i32, i32, Vec<u8>)>) {
        self.in_flight.borrow_mut().remove(&path);
        let Some((w, h, rgba)) = decoded else {
            self.waiters.borrow_mut().remove(&path);
            return;
        };

        let stride = (w as usize) * 4;
        let bytes = glib::Bytes::from_owned(rgba);
        let tex: gdk::Texture =
            gdk::MemoryTexture::new(w, h, gdk::MemoryFormat::R8g8b8a8, &bytes, stride).into();
        self.cache.borrow_mut().insert(path.clone(), tex.clone());

        if let Some(pics) = self.waiters.borrow_mut().remove(&path) {
            let expected = path.to_string_lossy();
            for weak in pics {
                if let Some(pic) = weak.upgrade()
                    && pic.widget_name() == expected.as_ref()
                {
                    pic.set_paintable(Some(&tex));
                }
            }
        }
    }

    pub fn bump_generation(&self) {
        self.queue.generation.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.queue.tasks.lock() {
            guard.clear();
        }
        self.waiters.borrow_mut().clear();
        self.in_flight.borrow_mut().clear();
    }

    pub fn bind_picture(&self, picture: &gtk4::Picture, path: &Path) {
        let key_str = path.to_string_lossy();
        picture.set_widget_name(&key_str);

        if let Some(tex) = self.cache.borrow().get(path) {
            picture.set_paintable(Some(&tex));
            return;
        }

        picture.set_paintable(gdk::Paintable::NONE);
        let path_buf = path.to_path_buf();
        self.waiters
            .borrow_mut()
            .entry(path_buf.clone())
            .or_default()
            .push(picture.downgrade());

        if self.in_flight.borrow_mut().insert(path_buf.clone()) {
            let generation = self.queue.generation.load(Ordering::Relaxed);
            if let Ok(mut guard) = self.queue.tasks.lock() {
                guard.push_back(DecodeTask {
                    generation,
                    path: path_buf,
                });
                self.queue.cvar.notify_one();
            }
        }
    }
}

pub fn decode_full_rgba(path: &Path) -> Option<(i32, i32, Vec<u8>)> {
    decode_fast_rgba(path)
}

/// Fast path uses system libwebp SIMD decoder for `.webp` thumbnails (~10x faster than image-webp).
fn decode_fast_rgba(path: &Path) -> Option<(i32, i32, Vec<u8>)> {
    if path.extension().and_then(|e| e.to_str()) == Some("webp")
        && let Ok(bytes) = fs::read(path)
        && let Some(decoded) = media::decode_webp_rgba(&bytes)
    {
        return Some(decoded);
    }
    let img = image::open(path).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w as i32, h as i32, rgba.into_raw()))
}

pub fn load_texture(path: &Path) -> Option<gdk::Texture> {
    if path.extension().and_then(|e| e.to_str()) == Some("webp")
        && let Some((w, h, rgba)) = decode_fast_rgba(path)
    {
        let stride = (w as usize) * 4;
        let bytes = glib::Bytes::from_owned(rgba);
        return Some(
            gdk::MemoryTexture::new(w, h, gdk::MemoryFormat::R8g8b8a8, &bytes, stride).into(),
        );
    }
    let file = gio::File::for_path(path);
    if let Ok(tex) = gdk::Texture::from_file(&file) {
        return Some(tex);
    }
    let (w, h, rgba) = decode_fast_rgba(path)?;
    let stride = (w as usize) * 4;
    let bytes = glib::Bytes::from_owned(rgba);
    Some(gdk::MemoryTexture::new(w, h, gdk::MemoryFormat::R8g8b8a8, &bytes, stride).into())
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_timestamp(unix_secs: i64) -> String {
    let Ok(dt) = glib::DateTime::from_unix_local(unix_secs) else {
        return "Unknown".to_string();
    };
    let Ok(now) = glib::DateTime::now_local() else {
        return dt
            .format("%Y-%m-%d %H:%M")
            .map(|s| s.to_string())
            .unwrap_or_default();
    };
    let hm = dt
        .format("%H:%M")
        .map(|s| s.to_string())
        .unwrap_or_default();
    if dt.ymd() == now.ymd() {
        format!("Today, {hm}")
    } else if let Ok(yesterday) = now.add_days(-1)
        && dt.ymd() == yesterday.ymd()
    {
        format!("Yesterday, {hm}")
    } else if dt.year() == now.year() {
        dt.format("%b %e, %H:%M")
            .map(|s| s.trim().to_string())
            .unwrap_or(hm)
    } else {
        dt.format("%Y-%m-%d")
            .map(|s| s.to_string())
            .unwrap_or(hm)
    }
}

pub fn format_relative(unix_secs: i64) -> String {
    let Ok(now) = glib::DateTime::now_local() else {
        return String::new();
    };
    let diff = now.to_unix() - unix_secs;
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 172_800 {
        "yesterday".to_string()
    } else if diff < 86_400 * 30 {
        format!("{}d ago", diff / 86_400)
    } else {
        glib::DateTime::from_unix_local(unix_secs)
            .ok()
            .and_then(|dt| dt.format("%b %e").ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }
}

pub fn short_format(mime: &str) -> &'static str {
    match mime {
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        "image/gif" => "GIF",
        "image/webp" => "WebP",
        _ => "IMG",
    }
}
