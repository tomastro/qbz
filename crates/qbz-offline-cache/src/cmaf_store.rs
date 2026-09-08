//! On-disk layout and I/O for v2 CMAF-bundle offline cache entries.
//!
//! Layout under `<offline_root>/tracks-cmaf/<track_id>/`:
//!
//! ```text
//! init.mp4        — init segment (unencrypted container + FLAC header)
//! segments.bin    — concatenated encrypted audio segments (s=1..=n)
//! manifest.json   — small recovery manifest (segment offsets + n_segments)
//! ```
//!
//! The `manifest.json` is a belt-and-suspenders convenience: SQLite is the
//! authoritative source of truth, but if the DB is ever lost we can still
//! tell the caller how to slice `segments.bin` back into per-segment
//! buffers from the manifest. It's cheap to write and cheap to read.
//!
//! Everything here is intentionally I/O-only — no network, no crypto. The
//! CMAF download itself happens in qbz-qobuz; this module just persists
//! the bytes in a format the playback path can read back efficiently.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use qbz_qobuz::cmaf::CmafRawBundle;
use serde::{Deserialize, Serialize};

const SUBDIR: &str = "tracks-cmaf";
const INIT_FILENAME: &str = "init.mp4";
const SEGMENTS_FILENAME: &str = "segments.bin";
const MANIFEST_FILENAME: &str = "manifest.json";

/// Lightweight sidecar manifest saved next to the bundle. If the SQLite
/// index is ever lost or an integrity check fails, this is enough to
/// reconstruct the per-segment slicing so the decrypt path can still
/// iterate the concatenated `segments.bin`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub version: u8,
    pub track_id: u64,
    pub format_id: u32,
    pub n_segments: u32,
    /// Offset in bytes of each encrypted segment inside `segments.bin`.
    /// `segment_offsets[i]` = start of segment `i+1`; `segment_offsets[n]`
    /// is the total size. Length = `n_segments + 1`.
    pub segment_offsets: Vec<u64>,
}

/// Where the v2 bundle for a given track id lives on disk. Callers use
/// this to build DB rows and to locate existing bundles at playback.
#[derive(Debug, Clone)]
pub struct BundleLayout {
    pub track_dir: PathBuf,
    pub init_path: PathBuf,
    pub segments_path: PathBuf,
    pub manifest_path: PathBuf,
}

impl BundleLayout {
    pub fn new(offline_root: &Path, track_id: u64) -> Self {
        let track_dir = offline_root.join(SUBDIR).join(track_id.to_string());
        Self {
            init_path: track_dir.join(INIT_FILENAME),
            segments_path: track_dir.join(SEGMENTS_FILENAME),
            manifest_path: track_dir.join(MANIFEST_FILENAME),
            track_dir,
        }
    }
}

/// Writes a freshly-downloaded [`CmafRawBundle`] to disk under the track
/// directory, returning the layout + total size of the persisted bytes.
///
/// Note: this does NOT write any key material. The caller is responsible
/// for wrapping `bundle.content_key` / `bundle.infos` via `qbz-secrets`
/// and persisting those blobs to the SQLite row.
pub fn persist_bundle(
    offline_root: &Path,
    track_id: u64,
    bundle: &CmafRawBundle,
) -> Result<(BundleLayout, u64), String> {
    let layout = BundleLayout::new(offline_root, track_id);
    fs::create_dir_all(&layout.track_dir)
        .map_err(|e| format!("Failed to create bundle dir {:?}: {}", layout.track_dir, e))?;

    // Init segment (unencrypted, small)
    write_atomic(&layout.init_path, &bundle.init_bytes)
        .map_err(|e| format!("Failed to write init: {}", e))?;

    // Audio segments: concatenated into a single file, with offsets tracked
    // for the manifest so playback can slice them back apart.
    let segments_tmp = layout.segments_path.with_extension("tmp");
    let file = fs::File::create(&segments_tmp)
        .map_err(|e| format!("Failed to create segments file: {}", e))?;
    let mut writer = BufWriter::new(file);
    let mut offsets: Vec<u64> = Vec::with_capacity(bundle.segments.len() + 1);
    let mut cursor: u64 = 0;
    offsets.push(cursor);
    for seg in &bundle.segments {
        writer
            .write_all(seg)
            .map_err(|e| format!("Failed to write segment: {}", e))?;
        cursor += seg.len() as u64;
        offsets.push(cursor);
    }
    writer
        .flush()
        .map_err(|e| format!("Failed to flush segments: {}", e))?;
    let file = writer
        .into_inner()
        .map_err(|e| format!("Failed to finalize writer: {}", e))?;
    file.sync_all()
        .map_err(|e| format!("Failed to fsync segments: {}", e))?;
    drop(file);
    fs::rename(&segments_tmp, &layout.segments_path)
        .map_err(|e| format!("Failed to rename segments: {}", e))?;

    let manifest = BundleManifest {
        version: 1,
        track_id,
        format_id: bundle.format_id,
        n_segments: bundle.n_segments as u32,
        segment_offsets: offsets.clone(),
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
    write_atomic(&layout.manifest_path, &manifest_json)
        .map_err(|e| format!("Failed to write manifest: {}", e))?;

    let total_bytes = bundle.init_bytes.len() as u64 + cursor + manifest_json.len() as u64;
    log::info!(
        "[OfflineCache/CMAF] Persisted bundle for track {}: init={}B, segments={}B ({} files), manifest={}B, total={:.2} MB",
        track_id,
        bundle.init_bytes.len(),
        cursor,
        bundle.segments.len(),
        manifest_json.len(),
        total_bytes as f64 / (1024.0 * 1024.0),
    );
    Ok((layout, total_bytes))
}

/// Load a bundle back from disk for playback. Returns the init bytes and
/// the per-segment slices of `segments.bin` in order.
pub fn read_bundle(layout: &BundleLayout) -> Result<LoadedBundle, String> {
    let init_bytes = fs::read(&layout.init_path)
        .map_err(|e| format!("Failed to read init {:?}: {}", layout.init_path, e))?;
    let segments_blob = fs::read(&layout.segments_path)
        .map_err(|e| format!("Failed to read segments {:?}: {}", layout.segments_path, e))?;
    let manifest_bytes = fs::read(&layout.manifest_path)
        .map_err(|e| format!("Failed to read manifest {:?}: {}", layout.manifest_path, e))?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("Failed to parse manifest: {}", e))?;

    let n = manifest.n_segments as usize;
    if manifest.segment_offsets.len() != n + 1 {
        return Err(format!(
            "Manifest offsets length {} doesn't match n_segments {}+1",
            manifest.segment_offsets.len(),
            n
        ));
    }
    let mut segments: Vec<Vec<u8>> = Vec::with_capacity(n);
    for i in 0..n {
        let start = manifest.segment_offsets[i] as usize;
        let end = manifest.segment_offsets[i + 1] as usize;
        if end > segments_blob.len() {
            return Err(format!(
                "Segment {} offset {}..{} past blob size {}",
                i + 1,
                start,
                end,
                segments_blob.len()
            ));
        }
        segments.push(segments_blob[start..end].to_vec());
    }

    Ok(LoadedBundle {
        init_bytes,
        segments,
        manifest,
    })
}

/// Remove a bundle from disk (called by eviction / re-download).
pub fn remove_bundle(layout: &BundleLayout) {
    if layout.track_dir.exists() {
        if let Err(e) = fs::remove_dir_all(&layout.track_dir) {
            log::warn!(
                "[OfflineCache/CMAF] Failed to remove bundle dir {:?}: {}",
                layout.track_dir,
                e
            );
        }
    }
}

/// A freshly-loaded bundle ready to be decrypted + played. Owned buffers
/// so the caller can feed the player without holding any file locks.
pub struct LoadedBundle {
    pub init_bytes: Vec<u8>,
    pub segments: Vec<Vec<u8>>,
    pub manifest: BundleManifest,
}

impl LoadedBundle {
    /// Decrypt the bundle into a complete, playable FLAC byte stream.
    ///
    /// The layout is `flac_header || decrypted_frames` — identical to
    /// what the streaming playback path produces, so the player can
    /// consume it via `play_data` exactly like a cached plain FLAC.
    pub fn decrypt_to_flac(&self, content_key: &[u8; 16]) -> Result<Vec<u8>, String> {
        let init_info = qbz_cmaf::parse_init_segment(&self.init_bytes)
            .map_err(|e| format!("Failed to parse init segment: {}", e))?;

        let total = init_info.flac_header.len()
            + init_info
                .segment_table
                .iter()
                .map(|s| s.byte_len as usize)
                .sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&init_info.flac_header);
        qbz_qobuz::cmaf::decrypt_segments_into(&self.segments, content_key, &mut out)?;
        Ok(out)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_bundle() -> CmafRawBundle {
        CmafRawBundle {
            init_bytes: b"init-segment-bytes".to_vec(),
            segments: vec![b"segment-one".to_vec(), b"segment-two".to_vec()],
            content_key: [7u8; 16],
            infos: "infos".to_string(),
            format_id: 6,
            sampling_rate: Some(44100),
            bit_depth: Some(16),
            n_segments: 2,
        }
    }

    /// The bundle is found by deriving its location from the root the cache is
    /// open at, so an application directory that moves underneath it costs
    /// nothing. iOS reassigns an app's data container on reinstall, which is
    /// what makes this more than theoretical: every absolute path recorded at
    /// download time names a directory that no longer exists, while the bundle
    /// itself is untouched.
    #[test]
    fn a_bundle_is_found_after_its_root_moves() {
        let old_root = tempfile::tempdir().unwrap();
        let (written, _) = persist_bundle(old_root.path(), 42, &raw_bundle()).unwrap();

        // What a reinstall does: same bytes, same layout, different prefix.
        let new_parent = tempfile::tempdir().unwrap();
        let new_root = new_parent.path().join("audio");
        std::fs::rename(old_root.path(), &new_root).unwrap();
        assert!(
            !written.segments_path.exists(),
            "the recorded path is stale"
        );

        let loaded = read_bundle(&BundleLayout::new(&new_root, 42)).unwrap();

        assert_eq!(loaded.init_bytes, b"init-segment-bytes");
        assert_eq!(loaded.segments.len(), 2);
        assert_eq!(loaded.segments[1], b"segment-two");
    }

    #[test]
    fn the_layout_is_the_same_one_persist_wrote_to() {
        let root = tempfile::tempdir().unwrap();
        persist_bundle(root.path(), 7, &raw_bundle()).unwrap();

        // Playback derives rather than reading the row back, so the two must
        // agree for every track: this is what makes deriving safe.
        //
        // Asserted against the filesystem rather than against what
        // `persist_bundle` returned. That value is built by the same pure
        // function this derives from, so comparing the two compares a value
        // with itself: it stays green even when both are wrong, which is no
        // guard at all.
        let derived = BundleLayout::new(root.path(), 7);
        assert!(
            derived.init_path.exists(),
            "nothing at the derived init path {:?}",
            derived.init_path
        );
        assert!(
            derived.segments_path.exists(),
            "nothing at the derived segments path {:?}",
            derived.segments_path
        );
        assert!(
            derived.manifest_path.exists(),
            "nothing at the derived manifest path {:?}",
            derived.manifest_path
        );
    }
}
