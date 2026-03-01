use image::RgbaImage;
use log::{error, warn};
use std::path::Path;
use std::{collections::BTreeSet, fs, path::PathBuf, sync::Mutex, time::SystemTime};
use xxhash_rust::const_xxh3::xxh3_64;

use orbclient::{Color, Renderer};
use orbimage::Image;

/// returns the cache path and cache hash
fn get_cached_background(path: &Path, w: u32, h: u32) -> Option<PathBuf> {
    let cache_dir = Path::new("/var/cache/backgrounds");

    if !cache_dir.is_dir() {
        if let Err(e) = fs::create_dir_all(&cache_dir) {
            warn!("Unable to create cache directory: {e:?}");
            return None;
        }
    }

    let mtime = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let input = format!("{}:{}:{}x{}", path.display(), mtime, w, h);
    let hash = xxh3_64(input.as_bytes());

    Some(cache_dir.join(format!("{:x}.bmp", hash)))
}

static CACHED_IMAGES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

pub(crate) fn scale_and_cache(source: &Path, width: u32, height: u32) -> Image {
    let cache_path = get_cached_background(source, width, height);

    if let Some(ref path) = cache_path {
        if path.exists() {
            if let Ok(mut cached_images) = CACHED_IMAGES.lock() {
                if let Some(name) = path.file_name().map(|x| x.to_string_lossy()) {
                    cached_images.insert(name.to_string());
                }
            }
            if let Ok(img) = Image::from_path(path) {
                return img;
            }
        }
    }

    let original = match Image::from_path(source) {
        Ok(image) => image,
        Err(err) => {
            error!("error loading {}: {}", source.display(), err);
            return Image::from_color(width, height, Color::rgb(0, 0, 0xff));
        }
    };

    let scaled = if width == original.width() && height == original.height() {
        original
    } else {
        original
            .resize(width, height, orbimage::ResizeType::Catrom)
            .unwrap()
    };

    if let Some(path) = cache_path {
        let width = scaled.width();
        let height = scaled.height();
        let data = scaled.data();

        let mut rgba_bytes = Vec::with_capacity((width * height * 4) as usize);
        for color in data.iter() {
            rgba_bytes.extend_from_slice(&[color.r(), color.g(), color.b(), color.a()]);
        }

        if let Some(img_buffer) = RgbaImage::from_raw(width, height, rgba_bytes) {
            if let Err(err) = img_buffer.save(&path) {
                warn!("Unable to write background cache: {err:?}");
            } else {
                if let Ok(mut cached_images) = CACHED_IMAGES.lock() {
                    if let Some(name) = path.file_name().map(|x| x.to_string_lossy()) {
                        cached_images.insert(name.to_string());
                    }
                }
            }
        }
    }

    scaled
}

pub(crate) fn remove_unused_cache() -> std::io::Result<()> {
    let cache_dir = Path::new("/var/cache/backgrounds");

    if !cache_dir.is_dir() {
        return Ok(());
    }

    let paths = fs::read_dir(&cache_dir)?;

    let Ok(cached_images) = CACHED_IMAGES.lock() else {
        return Ok(());
    };

    for path in paths {
        let Ok(path) = path else {
            continue;
        };
        let name = path.file_name().to_string_lossy().to_string();
        if !cached_images.contains(&name) {
            fs::remove_file(path.path())?;
        }
    }

    Ok(())
}
