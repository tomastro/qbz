//! App-wide dynamic ambient background (phase 14) — the
//! palette half of the feature. Ports the album-art triad extraction the
//! Slint pushes to `shader_underlay::set_palette` on every track change
//! (crates/qbz/src/playback.rs:1590-1643 → crates/qbz/src/immersive.rs):
//!
//! - primary / secondary: `spectrum_colors` — coverage-weighted hue
//!   histogram over the cover's CHROMATIC pixels (24 bins, one vote per
//!   pixel, L in 0.10..=0.93, S >= 0.08, >= 4 chromatic pixels else the
//!   teal/purple default). Primary = the most abundant hue cluster;
//!   secondary = a second genuine cluster >= ~45° away and >= 35% of the
//!   peak, else the same hue deeper (never a fabricated rotation).
//! - accent: `lyrics_accent_color` — the dominant hue COMPLEMENTED (+180°),
//!   vivid mid-bright.
//!
//! Input = the CURRENT now-playing artwork (the same cached cover the bar
//! shows), recomputed on track change from playback_qt's refresh — 1:1 the
//! Slint drive point. Decode + downscale run on a spawned task; only the
//! three hex strings cross to the UI thread.
//!
//! The three POC-NOTEs that used to sit here are gone because the gaps they
//! described are closed (2026-08-11) — leaving them would have kept asserting
//! defects the code no longer has:
//! - The scene is now a real shader. `qml/assets/shaders/ambient.frag` is a
//!   line-for-line GLSL port of the WGSL (fbm domain warp, r^2/d^2 metaball
//!   fusion, the smoothstep iso-surface, the saturation push, the grain), and
//!   `AmbientField.qml` keeps the additive-gradient Canvas only as the
//!   software-scene-graph fallback. The old note called that Canvas "a
//!   close-but-simpler read"; it was closer to a different picture, because it
//!   painted over a BLACK base and the reference has a 42%-of-album-colour
//!   floor everywhere.
//! - "Blurred art" is a real, separate mode. `app_background_mode()` returns
//!   the reference's 0/1/2 and AppShell mounts ImmersiveAtmosphere for 2, the
//!   same component the reference reuses; the pref no longer collapses to the
//!   ambient look.
//!
//! What REMAINS a deliberate divergence:
//! - The Slint gates the whole feature to the wgpu renderer tier (its scene is
//!   the wgpu underlay, so off-tier there is nothing to show). Here the field
//!   picks a ShaderEffect or a Canvas from `GraphicsInfo`, so it renders on
//!   every path and the feature is never taken away from a weak GPU.
//! - No audio breathe: the reference multiplies the blob radius by
//!   `1 + 0.12 * level_smooth`, which needs the 30 fps FFT drain. Slint runs
//!   that drain for mode 1 regardless — the same drain renders its shader
//!   texture — while this field is timer-driven, so the tap would be switched
//!   on app-wide purely for the pulse. At level 0 the reference's breathe is
//!   exactly 1.0, so the geometry is unchanged; only the audio reaction is
//!   absent. The shader keeps the `levelSmooth` uniform so wiring it later is
//!   a QML binding, not a shader edit.

use cxx_qt_lib::QString;

// ---------------------------------------------------------------------------
// HSL helpers (immersive.rs rgb_to_hsl / hsl_to_rgb, ported 1:1)
// ---------------------------------------------------------------------------

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1.0e-6 {
        return (0.0, 0.0, l);
    }
    let s = (d / (1.0 - (2.0 * l - 1.0).abs())).clamp(0.0, 1.0);
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h.rem_euclid(360.0), s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

// ---------------------------------------------------------------------------
// Palette extraction (immersive.rs spectrum_colors / lyrics_accent_color)
// ---------------------------------------------------------------------------

/// Coverage-weighted dominant hue bin over the 16x16 sample (shared by both
/// extractors). Returns (best bin, best score, chromatic count).
fn dominant_hue_bin(img: &image::RgbaImage) -> (usize, f32, u32) {
    const BINS: usize = 24;
    let mut hist = [0.0f32; BINS];
    let mut chromatic = 0u32;
    for px in img.pixels() {
        let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);
        if !(0.10..=0.93).contains(&l) || s < 0.08 {
            continue;
        }
        let bin = ((h / 360.0 * BINS as f32) as usize).min(BINS - 1);
        hist[bin] += 1.0;
        chromatic += 1;
    }
    let score_at = |i: usize| hist[i] + 0.5 * (hist[(i + BINS - 1) % BINS] + hist[(i + 1) % BINS]);
    let mut best_i = 0usize;
    let mut best = -1.0f32;
    for (i, _) in hist.iter().enumerate() {
        let sc = score_at(i);
        if sc > best {
            best = sc;
            best_i = i;
        }
    }
    (best_i, best, chromatic)
}

/// primary + secondary (spectrum_colors).
fn spectrum_colors(img: &image::RgbaImage) -> ((u8, u8, u8), (u8, u8, u8)) {
    const BINS: usize = 24;
    let default = ((0, 220, 200), (150, 50, 255));
    let (best_i, best, chromatic) = dominant_hue_bin(img);
    if chromatic < 4 {
        return default;
    }
    let primary_hue = (best_i as f32 + 0.5) * (360.0 / BINS as f32);

    // Rebuild the histogram scoring for the secondary search (kept 1:1 with
    // the Slint, which recomputes scores per candidate).
    let mut hist = [0.0f32; BINS];
    for px in img.pixels() {
        let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);
        if !(0.10..=0.93).contains(&l) || s < 0.08 {
            continue;
        }
        let bin = ((h / 360.0 * BINS as f32) as usize).min(BINS - 1);
        hist[bin] += 1.0;
    }
    let score_at = |i: usize| hist[i] + 0.5 * (hist[(i + BINS - 1) % BINS] + hist[(i + 1) % BINS]);
    let mut sec_i: Option<usize> = None;
    let mut sec_best = 0.0f32;
    for i in 0..BINS {
        let circ = (i as i32 - best_i as i32).rem_euclid(BINS as i32);
        let dist = circ.min(BINS as i32 - circ);
        if dist < 3 {
            continue;
        }
        let sc = score_at(i);
        if sc > sec_best {
            sec_best = sc;
            sec_i = Some(i);
        }
    }

    let primary = hsl_to_rgb(primary_hue, 0.85, 0.58);
    let secondary = match sec_i.filter(|_| sec_best >= best * 0.35) {
        Some(si) => {
            let sec_hue = (si as f32 + 0.5) * (360.0 / BINS as f32);
            hsl_to_rgb(sec_hue, 0.88, 0.62)
        }
        None => hsl_to_rgb(primary_hue, 0.95, 0.40),
    };
    (primary, secondary)
}

/// accent (lyrics_accent_color): dominant hue complemented +180°.
fn lyrics_accent_color(img: &image::RgbaImage) -> (u8, u8, u8) {
    let default = (0x3f, 0xd9, 0xc8);
    let (best_i, _best, chromatic) = dominant_hue_bin(img);
    if chromatic < 4 {
        return default;
    }
    let dominant_hue = (best_i as f32 + 0.5) * (360.0 / 24.0);
    let accent_hue = (dominant_hue + 180.0).rem_euclid(360.0);
    hsl_to_rgb(accent_hue, 0.85, 0.62)
}

fn hex(c: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// AlbumReactive glow color (immersive-port contract §4.3) — 1:1 port of
/// `crates/qbz/src/immersive.rs:63-88`: the most-saturated NON-EXTREME
/// (luminance 50..220) sample of the 8x8 tiny, fallback (100, 100, 255) ≡
/// the `#6464ff59` default. NOTE: its lightness/saturation are NOT
/// `rgb_to_hsl` above — the Slint glow uses its own lum = (max+min)/2 on the
/// 0..255 scale with the two-arm saturation below; keep them separate.
/// Returns RGB; the alpha (0x59) is applied at the publish site.
fn glow_color(tiny: &image::RgbaImage) -> (u8, u8, u8) {
    let mut best_sat = 0.0f32;
    let mut best = (100u8, 100u8, 255u8);

    for px in tiny.pixels() {
        let r = px[0] as f32;
        let g = px[1] as f32;
        let b = px[2] as f32;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let lum = (max + min) / 2.0;
        let sat = if (max - min).abs() < f32::EPSILON {
            0.0
        } else if lum > 127.0 {
            (max - min) / (510.0 - max - min).max(1.0)
        } else {
            (max - min) / (max + min).max(1.0)
        };
        if lum > 50.0 && lum < 220.0 && sat > best_sat {
            best_sat = sat;
            best = (px[0], px[1], px[2]);
        }
    }

    best
}

/// Qt publishes of the glow: Qt colors parse as `#AARRGGBB`, the Slint glow
/// is RRGGBBAA with alpha 0x59 (`#6464ff59` style) — converted HERE, once,
/// at the source, so QML never sees the Slint byte order.
fn glow_hex_qt(c: (u8, u8, u8)) -> String {
    format!("#59{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img_from(pixels: &[(u8, u8, u8)]) -> image::RgbaImage {
        let mut img = image::RgbaImage::new(16, 16);
        for (i, px) in img.pixels_mut().enumerate() {
            let (r, g, b) = pixels[i % pixels.len()];
            *px = image::Rgba([r, g, b, 255]);
        }
        img
    }

    #[test]
    fn dominant_hue_wins_over_speck() {
        // 3/4 red-ish + 1/4 blue-ish: the coverage-weighted primary must be
        // the red hue (not the more saturated speck).
        let img = img_from(&[(200, 30, 30), (200, 30, 30), (200, 30, 30), (30, 30, 220)]);
        let (primary, _secondary) = spectrum_colors(&img);
        let (h, _s, _l) = rgb_to_hsl(primary.0, primary.1, primary.2);
        assert!(h < 15.0 || h > 345.0, "primary hue {h} should be red-ish");
    }

    #[test]
    fn grey_cover_falls_back_to_defaults() {
        let img = img_from(&[(128, 128, 128)]);
        let (primary, secondary) = spectrum_colors(&img);
        assert_eq!(primary, (0, 220, 200));
        assert_eq!(secondary, (150, 50, 255));
        assert_eq!(lyrics_accent_color(&img), (0x3f, 0xd9, 0xc8));
    }

    #[test]
    fn accent_complements_the_dominant_hue() {
        let img = img_from(&[(200, 30, 30), (200, 30, 30), (200, 30, 30), (210, 40, 40)]);
        let accent = lyrics_accent_color(&img);
        let (h, _s, _l) = rgb_to_hsl(accent.0, accent.1, accent.2);
        assert!(
            (170.0..=190.0).contains(&h),
            "accent hue {h} should be cyan-ish"
        );
    }

    #[test]
    fn secondary_stays_on_album_for_mono_covers() {
        // Single-hue cover: the secondary must be the SAME hue, deeper —
        // never a fabricated rotation (immersive.rs comment).
        let img = img_from(&[(220, 40, 120), (200, 30, 110)]);
        let (primary, secondary) = spectrum_colors(&img);
        let (ph, _, _) = rgb_to_hsl(primary.0, primary.1, primary.2);
        let (sh, _, sl) = rgb_to_hsl(secondary.0, secondary.1, secondary.2);
        let dist = (ph - sh).abs().min(360.0 - (ph - sh).abs());
        assert!(
            dist < 30.0,
            "mono cover: secondary hue {sh} vs primary {ph}"
        );
        assert!(sl < 0.45, "mono cover: secondary should be deeper (L={sl})");
    }

    // --- glow_color (contract §4.3; port fidelity vs immersive.rs:63-88) ---

    fn img8_from(pixels: &[(u8, u8, u8)]) -> image::RgbaImage {
        let mut img = image::RgbaImage::new(8, 8);
        for (i, px) in img.pixels_mut().enumerate() {
            let (r, g, b) = pixels[i % pixels.len()];
            *px = image::Rgba([r, g, b, 255]);
        }
        img
    }

    #[test]
    fn glow_picks_the_most_saturated_non_extreme_sample() {
        // Hand-computed against the Slint formula (lum = (max+min)/2 on the
        // 0..255 scale; sat two-arm at lum > 127):
        //   A (200,30,30):  lum=115  in range, sat=(170)/(230)=0.739  <- max
        //   B (60,200,60):  lum=130  in range, sat=(140)/(510-260)=0.56
        //   C (240,230,230): lum=235 EXCLUDED (> 220) despite saturation
        //   D (20,25,20):   lum=22.5 EXCLUDED (< 50)
        //   grey:           sat=0
        let img = img8_from(&[
            (200, 30, 30),
            (60, 200, 60),
            (240, 230, 230),
            (20, 25, 20),
            (128, 128, 128),
        ]);
        assert_eq!(glow_color(&img), (200, 30, 30));
    }

    #[test]
    fn glow_falls_back_when_nothing_qualifies() {
        // All-grey 8x8: sat 0 everywhere -> the (100,100,255) fallback ≡ the
        // #6464ff59 default.
        let img = img8_from(&[(128, 128, 128)]);
        assert_eq!(glow_color(&img), (100, 100, 255));
        // Only extreme-luminance chroma (too bright / too dark): also fallback.
        let img = img8_from(&[(250, 240, 240), (10, 15, 10)]);
        assert_eq!(glow_color(&img), (100, 100, 255));
    }

    #[test]
    fn glow_hex_is_qt_aarrggbb_with_the_slint_alpha() {
        // Slint RRGGBBAA alpha 0x59 -> Qt #AARRGGBB.
        assert_eq!(glow_hex_qt((100, 100, 255)), "#596464ff");
        assert_eq!(glow_hex_qt((200, 30, 30)), "#59c81e1e");
    }
}

/// Recompute the ambient triad from the current track's artwork and publish
/// it to the bridge (ambientPrimary/Secondary/Accent). Called on track
/// change from playback_qt's now-playing refresh. A cover that is not on
/// disk yet is downloaded first (the Slint fetches+decodes itself the same
/// way); a failed download leaves the previous triad in place (the Slint
/// default-until-resolved behavior).
pub fn update_for_artwork(artwork_url: &str) {
    if artwork_url.is_empty() {
        return;
    }
    let url = artwork_url.to_string();
    crate::spawn(async move {
        if crate::artwork_qt::cached_path(&url).is_empty() {
            crate::artwork_qt::download_missing(vec![url.clone()]).await;
        }
        let path = crate::artwork_qt::cached_path(&url);
        if path.is_empty() {
            return;
        }
        // NOT trim_start_matches: cached_path returns a REAL file:// URL,
        // so on Windows that leaves the `/` before the drive letter
        // (`/C:/Users/...`), which opens nothing.
        let path = qbz_models::fs_url::local_path(&path);
        let triad = tokio::task::spawn_blocking(move || {
            let bytes = std::fs::read(&path).ok()?;
            let img = image::load_from_memory(&bytes).ok()?;
            let rgba = img.to_rgba8();
            let tiny =
                image::imageops::resize(&rgba, 16, 16, image::imageops::FilterType::Triangle);
            let (primary, secondary) = spectrum_colors(&tiny);
            let accent = lyrics_accent_color(&tiny);
            // Immersive (contract §4.3): the SAME decode feeds the 8x8 glow
            // sample and the atmosphere asset — both inside this
            // spawn_blocking, never on the Qt thread
            // (atmosphere_qt.rs:126-127).
            let tiny8 = image::imageops::resize(&rgba, 8, 8, image::imageops::FilterType::Triangle);
            let glow = glow_hex_qt(glow_color(&tiny8));
            let atmosphere = crate::atmosphere_qt::for_cover_blocking(&path);
            // B1: the SAME decode feeds the Tunnel Flow palette — the Tauri
            // extractLinePaletteFromArtwork port (tunnelflow_qt.rs) at its
            // 36x36 sample size, off the Qt thread like the triad.
            let tiny36 = image::imageops::resize(
                &rgba,
                crate::tunnelflow_qt::SAMPLE_SIZE,
                crate::tunnelflow_qt::SAMPLE_SIZE,
                image::imageops::FilterType::Triangle,
            );
            let tunnel = crate::tunnelflow_qt::line_palette_json(
                &crate::tunnelflow_qt::line_palette(&tiny36),
            );
            Some((
                hex(primary),
                hex(secondary),
                hex(accent),
                glow,
                atmosphere,
                tunnel,
            ))
        })
        .await
        .ok()
        .flatten();
        if let Some((primary, secondary, accent, glow, atmosphere, tunnel)) = triad {
            log::info!("[qbz-qt] ambient palette: {primary} / {secondary} / {accent}");
            crate::shell_bridge::ui(move |mut b| {
                b.as_mut()
                    .set_ambient_primary(QString::from(primary.as_str()));
                b.as_mut()
                    .set_ambient_secondary(QString::from(secondary.as_str()));
                b.as_mut()
                    .set_ambient_accent(QString::from(accent.as_str()));
            });
            // Publish ONLY on success; on failure (or a missing atmosphere)
            // the previous values stay — never cleared on track change (the
            // flicker fix, playback.rs:2344-2351 semantics; contract §4.3).
            crate::immersive_bridge::ui(move |mut x| {
                x.as_mut().set_glow_color(QString::from(glow.as_str()));
                if let Some(url) = atmosphere {
                    x.as_mut().set_atmosphere_url(QString::from(url.as_str()));
                }
            });
            // B1: the Tunnel Flow palette rides the same success-only rule
            // (one batched QString on QbzShaderScene).
            crate::shader_scene_bridge::publish_tunnel_palette(tunnel);
        }
    });
}
