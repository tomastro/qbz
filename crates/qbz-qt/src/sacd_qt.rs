//! Open a SACD disc image as an in-memory ephemeral session.
//!
//! Unlike a CD, a SACD names itself: the Master TOC carries the album title
//! and artist, and the area TOC carries a title per track. So this asks no
//! network and invents nothing — every string below came off the disc.
//!
//! Scope: the stereo DSD64 area of a raw Scarlet Book or hybrid image, either
//! flat or DST-compressed. A multichannel-only, malformed or unsupported image
//! is reported, never approximated into silence.
//!
//! The rows themselves come from `qbz_library::sacd_scan::build_image_rows`,
//! the same parser/row builder the folder scan uses. Catalogue ownership does
//! not: only the folder scanner may import those rows into `library.db`.

use crate::local_ephemeral;

/// Read an image and publish its stereo area as the ephemeral session.
/// Returns the track count, or an already-translated error for a toast.
pub fn open_image(path: &std::path::Path) -> Result<usize, String> {
    let labels = qbz_library::SacdLabels {
        album: qbz_i18n::t("SACD"),
        track: qbz_i18n::t("Track {}"),
    };
    let rows = qbz_library::build_image_rows(path, &labels).map_err(|e| {
        log::warn!("[qbz-qt] sacd: selected image unusable: {e}");
        match e {
            qbz_disc::sacd::SacdError::NoStereoArea => {
                qbz_i18n::t("That image has no stereo audio area.")
            }
            qbz_disc::sacd::SacdError::MissingMasterToc
            | qbz_disc::sacd::SacdError::NotRegularFile(_) => {
                qbz_i18n::t("That file is not a SACD image.")
            }
            qbz_disc::sacd::SacdError::Iso(_) => qbz_i18n::t("Could not read the disc."),
            _ => qbz_i18n::t("That SACD image is damaged or unsupported."),
        }
    })?;

    // Which disc this is, so a correction can be written under a key that
    // outlives the session — and found again if the .iso moves.
    crate::disc_identity::set(crate::disc_identity::DiscIdentity {
        fingerprint: rows.fingerprint.clone(),
        // A SACD has no MusicBrainz DiscID. It is not missing data — the
        // format has no such thing.
        disc_id: None,
        kind: crate::disc_identity::DiscKind::Sacd,
    });

    let count = rows.tracks.len();
    log::info!(
        "[qbz-qt] sacd: {count} tracks, {:.0}s, album {:?}",
        rows.total_playtime_secs,
        rows.album
    );

    local_ephemeral::adopt_tracks(&rows.album, rows.tracks);
    Ok(count)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sacd_row_carries_the_shape_a_dsd_badge_expects() {
        // The badge reads (format, depth, rate). A disc track has to look
        // exactly like a .dsf row here, or Local Library would need a second
        // rule for discs — and a second rule is a second thing to forget.
        let r = qbz_disc::SacdRef {
            image: std::path::PathBuf::from("/m/d.iso"),
            track: 4,
        };
        assert_eq!(r.to_path_string(), "sacd:/m/d.iso#4");
        assert!(qbz_disc::SacdRef::is_sacd_path(&r.to_path_string()));
    }
}
