use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use directories::ProjectDirs;
use image::{
    DynamicImage, GenericImageView, ImageReader, codecs::jpeg::JpegEncoder, imageops::FilterType,
};
use sha2::{Digest, Sha256};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub const CUSTOM_AVATAR_PREFIX: &str = "data:image/jpeg;base64,";
pub const CUSTOM_AVATAR_REF_PREFIX: &str = "custom:";
pub const AVATAR_SIDE: u32 = 128;
pub const MAX_AVATAR_JPEG_BYTES: usize = 12 * 1024;
const MAX_SOURCE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SOURCE_DIMENSION: u32 = 8_192;

thread_local! {
    static IMAGE_CACHE: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
}

pub struct CropDraft {
    image: DynamicImage,
}

impl CropDraft {
    pub fn load(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path).context("读取图片信息失败")?;
        if metadata.len() > MAX_SOURCE_BYTES {
            bail!("图片文件不能超过 20 MiB");
        }
        let image = ImageReader::open(path)
            .context("无法打开图片")?
            .with_guessed_format()
            .context("无法识别图片格式")?
            .decode()
            .context("图片解码失败")?;
        let (width, height) = image.dimensions();
        if width == 0
            || height == 0
            || width > MAX_SOURCE_DIMENSION
            || height > MAX_SOURCE_DIMENSION
        {
            bail!("图片尺寸无效或超过 8192 像素");
        }
        Ok(Self { image })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.image.dimensions()
    }

    pub fn preview(&self) -> Image {
        dynamic_image_to_slint(&self.image)
    }

    pub fn encode(&self, zoom: f32, offset_x: f32, offset_y: f32) -> Result<String> {
        let zoom = zoom.clamp(1.0, 4.0);
        let (width, height) = self.image.dimensions();
        let base_side = width.min(height) as f32;
        let crop_side = (base_side / zoom).max(1.0);
        let max_left = width as f32 - crop_side;
        let max_top = height as f32 - crop_side;
        let centered_left = max_left / 2.0;
        let centered_top = max_top / 2.0;
        // UI offsets are measured in preview pixels. Convert their direction and
        // scale back into source pixels; dragging the image right reveals its left.
        let preview_scale = 200.0 / crop_side;
        let left = (centered_left - offset_x / preview_scale).clamp(0.0, max_left);
        let top = (centered_top - offset_y / preview_scale).clamp(0.0, max_top);
        let side = crop_side.round().clamp(1.0, base_side) as u32;
        let left = (left.round() as u32).min(width - side);
        let top = (top.round() as u32).min(height - side);
        let square = self
            .image
            .crop_imm(left, top, side, side)
            .resize_exact(AVATAR_SIDE, AVATAR_SIDE, FilterType::Lanczos3)
            .to_rgb8();

        for quality in [82, 76, 70, 64, 58, 52, 46, 40] {
            let mut bytes = Vec::new();
            JpegEncoder::new_with_quality(&mut bytes, quality)
                .encode_image(&square)
                .context("头像压缩失败")?;
            if bytes.len() <= MAX_AVATAR_JPEG_BYTES {
                return Ok(format!("{CUSTOM_AVATAR_PREFIX}{}", STANDARD.encode(bytes)));
            }
        }
        Err(anyhow!("图片内容过于复杂，无法压缩到头像大小限制"))
    }
}

pub fn is_custom_avatar(value: &str) -> bool {
    value.starts_with(CUSTOM_AVATAR_REF_PREFIX)
}

pub fn is_supported_avatar(value: &str) -> bool {
    matches!(value, "" | "L" | "K" | "M" | "R" | "E")
        || valid_reference(value)
        || decode_avatar_data(value).is_ok()
}

pub fn cache_upload(value: &str) -> Result<String> {
    let (bytes, image) = decode_avatar_data(value)?;
    let reference = reference_for_bytes(&bytes);
    write_cache(&reference, &bytes)?;
    cache_image(&reference, dynamic_image_to_slint_avatar(&image));
    Ok(reference)
}

pub(crate) fn reference_for_backup_data(value: &str) -> Result<String> {
    let (bytes, _) = decode_avatar_data(value)?;
    Ok(reference_for_bytes(&bytes))
}

pub fn store_download(reference: &str, value: &str) -> Result<()> {
    if !valid_reference(reference) {
        bail!("invalid avatar reference");
    }
    let (bytes, image) = decode_avatar_data(value)?;
    if reference_for_bytes(&bytes) != reference {
        bail!("avatar content hash mismatch");
    }
    write_cache(reference, &bytes)?;
    cache_image(reference, dynamic_image_to_slint_avatar(&image));
    Ok(())
}

pub fn has_cached(reference: &str) -> bool {
    valid_reference(reference)
        && cache_path(reference)
            .and_then(|path| fs::read(path).ok())
            .is_some_and(|bytes| {
                reference_for_bytes(&bytes) == reference && decode_jpeg(&bytes).is_ok()
            })
}

pub fn data_for_reference(reference: &str) -> Result<String> {
    let path = cache_path(reference).ok_or_else(|| anyhow!("invalid avatar reference"))?;
    let bytes = fs::read(path).context("读取头像缓存失败")?;
    if reference_for_bytes(&bytes) != reference || decode_jpeg(&bytes).is_err() {
        bail!("头像缓存校验失败");
    }
    Ok(format!("{CUSTOM_AVATAR_PREFIX}{}", STANDARD.encode(bytes)))
}

pub fn image_for_avatar(value: &str) -> Image {
    if !is_custom_avatar(value) {
        return Image::default();
    }
    IMAGE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(image) = cache.get(value) {
            return image.clone();
        }
        let image = cache_path(value)
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| {
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).ok()
            })
            .map(|decoded| dynamic_image_to_slint_avatar(&decoded))
            .unwrap_or_default();
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(value.to_owned(), image.clone());
        image
    })
}

fn decode_avatar_data(value: &str) -> Result<(Vec<u8>, DynamicImage)> {
    let encoded = value
        .strip_prefix(CUSTOM_AVATAR_PREFIX)
        .ok_or_else(|| anyhow!("not a custom avatar"))?;
    let bytes = STANDARD.decode(encoded).context("invalid avatar base64")?;
    if bytes.len() > MAX_AVATAR_JPEG_BYTES {
        bail!("avatar exceeds size limit");
    }
    let image = decode_jpeg(&bytes)?;
    Ok((bytes, image))
}

fn decode_jpeg(bytes: &[u8]) -> Result<DynamicImage> {
    if bytes.len() > MAX_AVATAR_JPEG_BYTES {
        bail!("avatar exceeds size limit");
    }
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .context("invalid avatar jpeg")?;
    if image.dimensions() != (AVATAR_SIDE, AVATAR_SIDE) {
        bail!("avatar must be 128x128");
    }
    Ok(image)
}

fn valid_reference(value: &str) -> bool {
    value
        .strip_prefix(CUSTOM_AVATAR_REF_PREFIX)
        .is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

fn reference_for_bytes(bytes: &[u8]) -> String {
    format!("{CUSTOM_AVATAR_REF_PREFIX}{:x}", Sha256::digest(bytes))
}

fn cache_path(reference: &str) -> Option<PathBuf> {
    let hash = reference.strip_prefix(CUSTOM_AVATAR_REF_PREFIX)?;
    if !valid_reference(reference) {
        return None;
    }
    ProjectDirs::from("dev", "EdenVoice", "EdenVoice")
        .map(|dirs| dirs.cache_dir().join("avatars").join(format!("{hash}.jpg")))
}

fn write_cache(reference: &str, bytes: &[u8]) -> Result<()> {
    let path = cache_path(reference).ok_or_else(|| anyhow!("invalid avatar reference"))?;
    if has_cached(reference) {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid avatar cache path"))?;
    fs::create_dir_all(parent).context("创建头像缓存目录失败")?;
    let temporary = path.with_extension("jpg.tmp");
    fs::write(&temporary, bytes).context("写入头像缓存失败")?;
    if path.exists() {
        fs::remove_file(&path).context("替换损坏的头像缓存失败")?;
    }
    fs::rename(temporary, path).context("保存头像缓存失败")
}

fn cache_image(reference: &str, image: Image) {
    IMAGE_CACHE.with(|cache| {
        cache.borrow_mut().insert(reference.to_owned(), image);
    });
}

fn dynamic_image_to_slint(image: &DynamicImage) -> Image {
    let rgba = image.to_rgba8();
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    );
    Image::from_rgba8(buffer)
}

fn dynamic_image_to_slint_avatar(image: &DynamicImage) -> Image {
    let mut rgba = image.to_rgba8();
    let radius = rgba.width().min(rgba.height()) as f32 / 2.0;
    let center_x = (rgba.width() as f32 - 1.0) / 2.0;
    let center_y = (rgba.height() as f32 - 1.0) / 2.0;
    for (x, y, pixel) in rgba.enumerate_pixels_mut() {
        let dx = x as f32 - center_x;
        let dy = y as f32 - center_y;
        if dx * dx + dy * dy > radius * radius {
            pixel.0[3] = 0;
        }
    }
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    );
    Image::from_rgba8(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn crop_produces_small_fixed_jpeg() {
        let mut source = RgbaImage::new(640, 320);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            *pixel = Rgba([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8, 255]);
        }
        let draft = CropDraft {
            image: DynamicImage::ImageRgba8(source),
        };
        let value = draft.encode(1.7, 18.0, -9.0).unwrap();
        assert!(value.starts_with(CUSTOM_AVATAR_PREFIX));
        let (_, decoded) = decode_avatar_data(&value).unwrap();
        assert_eq!(decoded.dimensions(), (AVATAR_SIDE, AVATAR_SIDE));
    }

    #[test]
    fn malformed_custom_avatar_is_rejected() {
        assert!(!is_supported_avatar("data:image/jpeg;base64,nope"));
    }
}
