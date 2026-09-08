//! Thumbnail generation for album artwork

use image::imageops::FilterType;
use image::ImageReader;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::LibraryError;

/// Default thumbnail size (width and height)
/// 500px is a good balance for UI display while keeping file size reasonable
const THUMBNAIL_SIZE: u32 = 500;

/// Bound for the ON-DEMAND large art tier (contract
/// `2026-08-15-immersive-completion` 04 §5, ruling 6): the big slots
/// (immersive main art, SPLIT, lightbox) decode the ORIGINAL embedded/folder
/// art bounded at 1600px — that covers the largest immersive slot
/// (~660-1000 CSS px) at 2x DPR headroom without unbounded decodes of 3000px
/// embedded art. Lists keep the 500px tier; nothing here changes their path.
pub const LARGE_ART_PX: u32 = 1600;

/// Get the thumbnails directory path
pub fn get_thumbnails_dir() -> Result<PathBuf, LibraryError> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| LibraryError::Other("Could not find data directory".into()))?;
    let thumbnails_dir = data_dir.join("qbz").join("thumbnails");

    // Create directory if it doesn't exist
    if !thumbnails_dir.exists() {
        fs::create_dir_all(&thumbnails_dir).map_err(|e| {
            LibraryError::Other(format!("Failed to create thumbnails directory: {}", e))
        })?;
    }

    Ok(thumbnails_dir)
}

/// Generate a unique filename for a thumbnail based on the source path
fn get_thumbnail_filename(source_path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    source_path.to_string_lossy().hash(&mut hasher);
    format!("{:016x}.jpg", hasher.finish())
}

/// Get the thumbnail path for a source image
pub fn get_thumbnail_path(source_path: &Path) -> Result<PathBuf, LibraryError> {
    let thumbnails_dir = get_thumbnails_dir()?;
    let filename = get_thumbnail_filename(source_path);
    Ok(thumbnails_dir.join(filename))
}

/// Check if a thumbnail exists for the given source path
pub fn thumbnail_exists(source_path: &Path) -> Result<bool, LibraryError> {
    let thumbnail_path = get_thumbnail_path(source_path)?;
    Ok(thumbnail_path.exists())
}

/// Generate a thumbnail for the given source image
pub fn generate_thumbnail(source_path: &Path) -> Result<PathBuf, LibraryError> {
    let thumbnail_path = get_thumbnail_path(source_path)?;

    // If thumbnail already exists, return it
    if thumbnail_path.exists() {
        return Ok(thumbnail_path);
    }

    // Read source image
    let img = ImageReader::open(source_path)
        .map_err(|e| LibraryError::Other(format!("Failed to open image: {}", e)))?
        .decode()
        .map_err(|e| LibraryError::Other(format!("Failed to decode image: {}", e)))?;

    // Resize to thumbnail size (maintaining aspect ratio, fit within square)
    let thumbnail = img.resize(THUMBNAIL_SIZE, THUMBNAIL_SIZE, FilterType::Lanczos3);

    // Save as JPEG with quality 85
    thumbnail
        .save(&thumbnail_path)
        .map_err(|e| LibraryError::Other(format!("Failed to save thumbnail: {}", e)))?;

    Ok(thumbnail_path)
}

/// Generate a thumbnail from image bytes (for embedded artwork)
pub fn generate_thumbnail_from_bytes(
    bytes: &[u8],
    cache_key: &str,
) -> Result<PathBuf, LibraryError> {
    let thumbnails_dir = get_thumbnails_dir()?;

    // Generate filename from cache key
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    cache_key.hash(&mut hasher);
    let filename = format!("{:016x}.jpg", hasher.finish());
    let thumbnail_path = thumbnails_dir.join(&filename);

    // If thumbnail already exists, return it
    if thumbnail_path.exists() {
        return Ok(thumbnail_path);
    }

    // Decode image from bytes
    let cursor = Cursor::new(bytes);
    let img = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| LibraryError::Other(format!("Failed to guess image format: {}", e)))?
        .decode()
        .map_err(|e| LibraryError::Other(format!("Failed to decode image: {}", e)))?;

    // Resize to thumbnail size
    let thumbnail = img.resize(THUMBNAIL_SIZE, THUMBNAIL_SIZE, FilterType::Lanczos3);

    // Save as JPEG
    thumbnail
        .save(&thumbnail_path)
        .map_err(|e| LibraryError::Other(format!("Failed to save thumbnail: {}", e)))?;

    Ok(thumbnail_path)
}

/// Get or generate a thumbnail for an artwork path
/// Returns the path to the thumbnail file
pub fn get_or_generate_thumbnail(artwork_path: &Path) -> Result<PathBuf, LibraryError> {
    let thumbnail_path = get_thumbnail_path(artwork_path)?;

    if thumbnail_path.exists() {
        return Ok(thumbnail_path);
    }

    generate_thumbnail(artwork_path)
}

// ---------------------------------------------------------------------------
// Large art tier (on demand — the big immersive/lightbox slots)
// ---------------------------------------------------------------------------

/// Large-tier filename for a source key (path or cache key): the SAME hasher
/// as the 500px tier, with a `_large` suffix so the two tiers sit side by
/// side in `thumbnails/` and never collide.
fn get_large_filename(source_key: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    source_key.hash(&mut hasher);
    format!("{:016x}_large.jpg", hasher.finish())
}

/// The large-tier cache path for a source image (may not exist yet).
pub fn get_large_thumbnail_path(source_path: &Path) -> Result<PathBuf, LibraryError> {
    let thumbnails_dir = get_thumbnails_dir()?;
    let filename = get_large_filename(&source_path.to_string_lossy());
    Ok(thumbnails_dir.join(filename))
}

/// Decode `img` into the bounded large tier and cache it as JPEG. If the
/// source fits inside [`LARGE_ART_PX`] it is re-encoded at NATIVE size —
/// never upscaled. Returns the cache path.
fn save_large_tier(img: image::DynamicImage, out_path: &Path) -> Result<PathBuf, LibraryError> {
    let bounded = if img.width() > LARGE_ART_PX || img.height() > LARGE_ART_PX {
        img.resize(LARGE_ART_PX, LARGE_ART_PX, FilterType::Lanczos3)
    } else {
        img
    };
    bounded
        .save(out_path)
        .map_err(|e| LibraryError::Other(format!("Failed to save large artwork: {}", e)))?;
    Ok(out_path.to_path_buf())
}

/// Large-tier art for a cover FILE, on demand. If the original already fits
/// inside [`LARGE_ART_PX`] the ORIGINAL path is returned (serve native — no
/// duplicate bytes, no upscale); otherwise the bounded decode is cached as
/// `{hash}_large.jpg` beside the 500px thumbs.
pub fn get_or_generate_large_thumbnail(source_path: &Path) -> Result<PathBuf, LibraryError> {
    let large_path = get_large_thumbnail_path(source_path)?;
    if large_path.exists() {
        return Ok(large_path);
    }

    let img = ImageReader::open(source_path)
        .map_err(|e| LibraryError::Other(format!("Failed to open image: {}", e)))?
        .decode()
        .map_err(|e| LibraryError::Other(format!("Failed to decode image: {}", e)))?;

    if img.width() <= LARGE_ART_PX && img.height() <= LARGE_ART_PX {
        return Ok(source_path.to_path_buf());
    }
    save_large_tier(img, &large_path)
}

/// Large-tier art from EMBEDDED picture bytes (the source lives inside an
/// audio file, so "serve native" has no path to hand back — a native-size
/// re-encode is cached instead). `cache_key` is the audio file path, the
/// same key the 500px tier uses, so the two tiers hash to sibling names.
pub fn get_or_generate_large_thumbnail_from_bytes(
    bytes: &[u8],
    cache_key: &str,
) -> Result<PathBuf, LibraryError> {
    let thumbnails_dir = get_thumbnails_dir()?;
    let large_path = thumbnails_dir.join(get_large_filename(cache_key));
    if large_path.exists() {
        return Ok(large_path);
    }

    let cursor = Cursor::new(bytes);
    let img = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| LibraryError::Other(format!("Failed to guess image format: {}", e)))?
        .decode()
        .map_err(|e| LibraryError::Other(format!("Failed to decode image: {}", e)))?;

    save_large_tier(img, &large_path)
}

/// Clear all thumbnails (useful for cache cleanup)
pub fn clear_thumbnails() -> Result<(), LibraryError> {
    let thumbnails_dir = get_thumbnails_dir()?;

    if thumbnails_dir.exists() {
        fs::remove_dir_all(&thumbnails_dir)
            .map_err(|e| LibraryError::Other(format!("Failed to clear thumbnails: {}", e)))?;
        fs::create_dir_all(&thumbnails_dir).map_err(|e| {
            LibraryError::Other(format!("Failed to recreate thumbnails directory: {}", e))
        })?;
    }

    Ok(())
}

/// Get the total size of the thumbnails cache in bytes
pub fn get_cache_size() -> Result<u64, LibraryError> {
    let thumbnails_dir = get_thumbnails_dir()?;

    if !thumbnails_dir.exists() {
        return Ok(0);
    }

    let mut total_size = 0u64;

    for entry in fs::read_dir(&thumbnails_dir)
        .map_err(|e| LibraryError::Other(format!("Failed to read thumbnails directory: {}", e)))?
    {
        if let Ok(entry) = entry {
            if let Ok(metadata) = entry.metadata() {
                total_size += metadata.len();
            }
        }
    }

    Ok(total_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_image(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            w,
            h,
            image::Rgb([120, 60, 200]),
        ))
    }

    fn decoded_size(path: &Path) -> (u32, u32) {
        let img = ImageReader::open(path).unwrap().decode().unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn large_tier_bounds_big_sources_without_distorting() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("large.jpg");
        let saved = save_large_tier(solid_image(3200, 2400), &out).unwrap();
        assert_eq!(saved, out);
        // Fit inside 1600, aspect preserved (4:3 -> 1600x1200).
        assert_eq!(decoded_size(&saved), (LARGE_ART_PX, 1200));
    }

    #[test]
    fn large_tier_never_upscales_small_sources() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("small.jpg");
        let saved = save_large_tier(solid_image(300, 300), &out).unwrap();
        // Native size is kept — a 300px cover stays 300px (the UI probe caps
        // the slot at native; nothing invents pixels).
        assert_eq!(decoded_size(&saved), (300, 300));
    }

    #[test]
    fn large_filename_is_a_sibling_of_the_500px_key() {
        // Same hasher, `_large` suffix: one source -> two tiers, no collision.
        let key = "/music/Album/cover.jpg";
        let thumb = get_thumbnail_filename(Path::new(key));
        let large = get_large_filename(key);
        assert!(thumb.ends_with(".jpg"));
        assert!(large.ends_with("_large.jpg"));
        assert_eq!(
            thumb.strip_suffix(".jpg").unwrap(),
            large.strip_suffix("_large.jpg").unwrap()
        );
    }
}
