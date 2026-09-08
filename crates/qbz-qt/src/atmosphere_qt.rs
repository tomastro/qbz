//! Blurred artwork "atmosphere" behind the album and artist headers.
//!
//! Port of `qbz/src/immersive.rs::generate_atmosphere` + `color_adjust` +
//! `vignette`. This is the header treatment Slint actually SHOWS; its flat
//! two-band gradient is only the fallback arm for when the atmosphere is
//! unavailable (`AlbumPageView.slint:233` — `header-atmosphere.width <= 0`).
//! The port had shipped that fallback as if it were the design, which is why
//! the header read as a plain ugly gradient.
//!
//! Nothing here needs a shader: the result is a 128x128 RGBA image, so it works
//! on the software path and on the Pi exactly like any other picture. It is
//! written once per cover into the cache dir and the QML shows it as an Image.
//!
//! The pipeline, stage for stage with the Slint original:
//!   source -> 8x8 (Triangle) -> 128x128 (CatmullRom) -> blur 16 ->
//!   colour adjust -> vignette 0.20
//! The 8x8 step is not an optimization, it IS the look: everything past it is
//! working from eight pixels of colour, which is what makes the field read as
//! atmosphere rather than as a blurred cover.

use std::path::PathBuf;

use image::{imageops, Rgba, RgbaImage};

/// Saturation / brightness / contrast, `immersive.rs::saturate_brightness_contrast`.
fn saturate_brightness_contrast(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (mut rf, mut gf, mut bf) = (r as f32, g as f32, b as f32);
    let gray = (rf + gf + bf) / 3.0;
    let sat = 2.45;
    rf = gray + (rf - gray) * sat;
    gf = gray + (gf - gray) * sat;
    bf = gray + (bf - gray) * sat;
    let brightness = 0.92;
    let contrast = 1.18;
    let adjust = |v: f32| ((v * brightness - 128.0) * contrast + 128.0).clamp(0.0, 255.0) as u8;
    (adjust(rf), adjust(gf), adjust(bf))
}

/// `immersive.rs::color_adjust` — saturate, then normalize each channel's range
/// toward [18, 250], blended by how bright the image already is.
fn color_adjust(mut img: RgbaImage) -> RgbaImage {
    let (mut min_r, mut min_g, mut min_b) = (255.0f32, 255.0f32, 255.0f32);
    let (mut max_r, mut max_g, mut max_b) = (0.0f32, 0.0f32, 0.0f32);
    let mut total_brightness = 0.0f32;
    let count = (img.width() * img.height()).max(1) as f32;

    for px in img.pixels_mut() {
        let (r, g, b) = saturate_brightness_contrast(px[0], px[1], px[2]);
        px[0] = r;
        px[1] = g;
        px[2] = b;
        let (rf, gf, bf) = (r as f32, g as f32, b as f32);
        min_r = min_r.min(rf);
        min_g = min_g.min(gf);
        min_b = min_b.min(bf);
        max_r = max_r.max(rf);
        max_g = max_g.max(gf);
        max_b = max_b.max(bf);
        total_brightness += (rf + gf + bf) / 3.0;
    }

    let norm_strength = (total_brightness / count / 80.0).min(1.0);
    let target_min = 18.0f32;
    let target_range = 232.0f32;
    let range_r = (max_r - min_r).max(1.0);
    let range_g = (max_g - min_g).max(1.0);
    let range_b = (max_b - min_b).max(1.0);

    for px in img.pixels_mut() {
        let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
        let norm_r = target_min + ((r - min_r) / range_r) * target_range;
        let norm_g = target_min + ((g - min_g) / range_g) * target_range;
        let norm_b = target_min + ((b - min_b) / range_b) * target_range;
        let lift_r = (r * 1.5).min(255.0);
        let lift_g = (g * 1.5).min(255.0);
        let lift_b = (b * 1.5).min(255.0);
        // The red channel carries the extra 1.08 in the original — keep it.
        px[0] = (lift_r + (norm_r - lift_r) * norm_strength)
            .mul_add(1.08, 0.0)
            .min(255.0) as u8;
        px[1] = (lift_g + (norm_g - lift_g) * norm_strength).min(255.0) as u8;
        px[2] = (lift_b + (norm_b - lift_b) * norm_strength).min(255.0) as u8;
    }
    img
}

/// `immersive.rs::vignette` — radial darkening from 20% to 70% of the shorter side.
fn vignette(mut img: RgbaImage, intensity: f32) -> RgbaImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let inner = w.min(h) * 0.20;
    let outer = w.min(h) * 0.70;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        let d = (dx * dx + dy * dy).sqrt();
        let t = ((d - inner) / (outer - inner)).clamp(0.0, 1.0);
        let factor = 1.0 - intensity * t;
        *px = Rgba([
            (px[0] as f32 * factor) as u8,
            (px[1] as f32 * factor) as u8,
            (px[2] as f32 * factor) as u8,
            px[3],
        ]);
    }
    img
}

/// The five stages, on an already-decoded cover.
fn atmosphere_from(src: &RgbaImage) -> RgbaImage {
    let tiny = imageops::resize(src, 8, 8, imageops::FilterType::Triangle);
    let scaled = imageops::resize(&tiny, 128, 128, imageops::FilterType::CatmullRom);
    let blurred = imageops::blur(&scaled, 16.0);
    vignette(color_adjust(blurred), 0.20)
}

fn cache_dir() -> Option<PathBuf> {
    let dir = dirs::cache_dir()?.join("qbz").join("atmosphere");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Generate (or reuse) the atmosphere for a cover already on disk. Returns a
/// `file://` url for QML, or None when the cover cannot be read.
///
/// Blocking: decodes and resizes. Call it from `spawn_blocking`, never on the
/// Qt thread.
pub fn for_cover_blocking(cover_path: &str) -> Option<String> {
    // NOT strip_prefix: a real file:// URL leaves the `/` before the drive on
    // Windows (`/C:/Users/...`), so image::open finds nothing and the header
    // atmosphere - and therefore the whole album/artist header gradient -
    // silently never appears.
    let src_path = qbz_models::fs_url::local_path(cover_path);
    let src_path = src_path.as_str();
    let dir = cache_dir()?;

    // Key on the cover path, like the artwork cache does.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&src_path, &mut hasher);
    let key = format!("{:016x}.png", std::hash::Hasher::finish(&hasher));
    let out = dir.join(&key);
    if out.is_file() {
        return Some(crate::artwork_qt::file_url(&out.to_string_lossy()));
    }

    // Same trap as artwork_qt::scaled_path: the cache uses `.img`, and
    // `image::open` guesses by extension.
    let img = image::ImageReader::open(src_path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .to_rgba8();
    let atmo = atmosphere_from(&img);
    atmo.save(&out).ok()?;
    Some(crate::artwork_qt::file_url(&out.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([rgb[0], rgb[1], rgb[2], 255]))
    }

    #[test]
    fn atmosphere_is_128_square() {
        let out = atmosphere_from(&flat(600, 600, [90, 40, 140]));
        assert_eq!((out.width(), out.height()), (128, 128));
    }

    #[test]
    fn vignette_darkens_the_corner_more_than_the_centre() {
        let out = atmosphere_from(&flat(64, 64, [200, 120, 60]));
        let centre = out.get_pixel(64, 64);
        let corner = out.get_pixel(2, 2);
        let lum = |p: &Rgba<u8>| p[0] as u32 + p[1] as u32 + p[2] as u32;
        assert!(
            lum(corner) < lum(centre),
            "corner {corner:?} vs centre {centre:?}"
        );
    }

    #[test]
    fn alpha_survives_the_pipeline() {
        let out = atmosphere_from(&flat(32, 32, [10, 200, 90]));
        assert!(out.pixels().all(|p| p[3] == 255));
    }
}
