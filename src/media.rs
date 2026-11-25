use std::ffi::{c_int, c_void};

use image::{GenericImageView, ImageFormat, imageops::FilterType};

pub const THUMB_MAX_DIM: u32 = 360;
pub const THUMB_WEBP_QUALITY: f32 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl MediaKind {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Gif => ImageFormat::Gif,
            Self::Webp => ImageFormat::WebP,
        }
    }
}

pub struct DecodedImage {
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub thumb_webp: Vec<u8>,
}

#[link(name = "webp")]
unsafe extern "C" {
    fn WebPEncodeRGBA(
        rgba: *const u8,
        width: c_int,
        height: c_int,
        stride: c_int,
        quality_factor: f32,
        output: *mut *mut u8,
    ) -> usize;
    fn WebPFree(ptr: *mut c_void);
}

pub fn sniff_format(bytes: &[u8]) -> Result<MediaKind, String> {
    let fmt = image::guess_format(bytes).map_err(|e| format!("unrecognized image header: {e}"))?;
    match fmt {
        ImageFormat::Png => Ok(MediaKind::Png),
        ImageFormat::Jpeg => Ok(MediaKind::Jpeg),
        ImageFormat::Gif => Ok(MediaKind::Gif),
        ImageFormat::WebP => Ok(MediaKind::Webp),
        other => Err(format!("unsupported image format: {other:?}")),
    }
}

pub fn validate_image(bytes: &[u8]) -> Result<(MediaKind, u32, u32), String> {
    let kind = sniff_format(bytes)?;
    let img = image::load_from_memory_with_format(bytes, kind.image_format())
        .map_err(|e| format!("invalid {} data: {e}", kind.ext()))?;
    let (width, height) = img.dimensions();
    Ok((kind, width, height))
}

pub fn inspect_and_thumbnail(bytes: &[u8]) -> Result<DecodedImage, String> {
    let kind = sniff_format(bytes)?;
    let img = image::load_from_memory_with_format(bytes, kind.image_format())
        .map_err(|e| format!("failed to decode {}: {e}", kind.ext()))?;
    let (width, height) = img.dimensions();

    let resized = if width > THUMB_MAX_DIM || height > THUMB_MAX_DIM {
        img.resize(THUMB_MAX_DIM, THUMB_MAX_DIM, FilterType::Triangle)
    } else {
        img
    };

    let rgba = resized.to_rgba8();
    let thumb_webp = encode_webp_rgba(rgba.as_raw(), rgba.width(), rgba.height(), THUMB_WEBP_QUALITY)?;

    Ok(DecodedImage {
        kind,
        width,
        height,
        thumb_webp,
    })
}

fn encode_webp_rgba(rgba: &[u8], width: u32, height: u32, quality: f32) -> Result<Vec<u8>, String> {
    let w = c_int::try_from(width).map_err(|_| format!("thumbnail width overflow: {width}"))?;
    let h = c_int::try_from(height).map_err(|_| format!("thumbnail height overflow: {height}"))?;
    let stride = w
        .checked_mul(4)
        .ok_or_else(|| format!("thumbnail stride overflow: {width}"))?;

    let mut out: *mut u8 = std::ptr::null_mut();
    // SAFETY: `rgba` has length width * height * 4 from ImageBuffer::to_rgba8; libwebp allocates
    // `out` and we copy and free it with WebPFree before returning.
    let size = unsafe { WebPEncodeRGBA(rgba.as_ptr(), w, h, stride, quality, &mut out) };
    if size == 0 || out.is_null() {
        return Err("libwebp failed to encode thumbnail".to_string());
    }

    let bytes = unsafe { std::slice::from_raw_parts(out, size).to_vec() };
    unsafe { WebPFree(out.cast()) };
    Ok(bytes)
}
