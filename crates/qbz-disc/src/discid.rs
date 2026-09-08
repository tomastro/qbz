//! The MusicBrainz Disc ID, computed from a table of contents.
//!
//! A CD-DA carries no titles. The only thing it has that identifies the RECORD
//! rather than the plastic is the geometry of its tracks, and MusicBrainz's
//! Disc ID is a hash of exactly that — so a disc anyone has ever submitted can
//! be named without reading a byte of audio.
//!
//! Verified end to end against the owner's disc on 2026-08-20: the TOC read by
//! `cdda::read_toc` produced `BeNBMsD8Du5NO2W61Yk.B2jwwIs-`, and MusicBrainz
//! answered with Tool — *Fear Inoculum* (2019-08-30) whose seven track lengths
//! match the measured TOC to the second. That is the whole chain — reader,
//! hash, encoding, lookup — proven at once, which is why it is worth writing
//! down here rather than trusting the spec alone.
//!
//! Spec: <https://musicbrainz.org/doc/Disc_ID_Calculation>

use sha1::{Digest, Sha1};

/// Blocks between the start of the disc and the first track. LBA numbering
/// starts at 0 but track 1 physically begins at 00:02:00, so every offset in
/// the hash is its LBA plus this.
const LEAD_IN: u32 = 150;
/// The hash always covers 99 track slots, whether or not the disc has them.
const TRACK_SLOTS: usize = 99;

/// Compute the Disc ID for a table of contents.
///
/// `starts` are the LBA start addresses of the AUDIO tracks in disc order and
/// `leadout` is the lead-out LBA — both exactly as the drive reports them, in
/// this crate's `Toc`.
///
/// Returns `None` for a disc with no tracks or more than 99, which cannot have
/// a Disc ID at all.
pub fn disc_id(starts: &[u32], leadout: u32) -> Option<String> {
    if starts.is_empty() || starts.len() > TRACK_SLOTS {
        return None;
    }
    // The hash input is TEXT, not packed binary: uppercase hex, fixed width,
    // zero padded. Getting this wrong produces a valid-looking 28-character id
    // that matches nothing, which is a failure mode with no symptom beyond
    // "MusicBrainz never knows any of my discs".
    let mut blob = String::with_capacity(4 + (TRACK_SLOTS + 1) * 8);
    blob.push_str(&format!("{:02X}", 1));
    blob.push_str(&format!("{:02X}", starts.len()));
    blob.push_str(&format!("{:08X}", leadout + LEAD_IN));
    for i in 0..TRACK_SLOTS {
        let v = starts.get(i).map(|s| s + LEAD_IN).unwrap_or(0);
        blob.push_str(&format!("{v:08X}"));
    }

    let digest = Sha1::digest(blob.as_bytes());

    // MusicBrainz's base64 is RFC 4648 with three substitutions. It is not
    // URL-safe base64 (that keeps `=`), so the alphabet is applied by hand.
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(digest);
    Some(
        b64.replace('+', ".")
            .replace('/', "_")
            .replace('=', "-"),
    )
}

/// The URL that resolves a Disc ID to releases, with recordings and artists.
///
/// MusicBrainz requires a descriptive User-Agent and rate-limits anonymous
/// clients to one request a second; a caller that ignores either gets blocked,
/// so both belong to whoever performs the request, not here.
pub fn lookup_url(disc_id: &str) -> String {
    format!(
        "https://musicbrainz.org/ws/2/discid/{disc_id}\
         ?fmt=json&inc=recordings+artist-credits+release-groups"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's disc, and the id MusicBrainz actually answered to.
    const FEAR_INOCULUM_STARTS: [u32; 7] =
        [0, 46_577, 100_047, 157_373, 218_718, 264_120, 285_735];
    const FEAR_INOCULUM_LEADOUT: u32 = 356_568;
    const FEAR_INOCULUM_ID: &str = "BeNBMsD8Du5NO2W61Yk.B2jwwIs-";

    #[test]
    fn the_owners_disc_hashes_to_the_id_musicbrainz_answered_to() {
        // This is not a self-consistency check. The expected value came from
        // a live lookup that returned Tool — Fear Inoculum with all seven
        // track lengths matching the TOC, so if this test fails the reader,
        // the hash or the encoding has drifted from something real.
        assert_eq!(
            disc_id(&FEAR_INOCULUM_STARTS, FEAR_INOCULUM_LEADOUT).as_deref(),
            Some(FEAR_INOCULUM_ID)
        );
    }

    #[test]
    fn an_id_is_always_twenty_eight_characters_of_the_modified_alphabet() {
        let id = disc_id(&FEAR_INOCULUM_STARTS, FEAR_INOCULUM_LEADOUT).unwrap();
        assert_eq!(id.len(), 28);
        // The three substitutions must have happened: no RFC 4648 characters
        // may survive, or the URL is malformed and the lookup 404s.
        assert!(!id.contains('+') && !id.contains('/') && !id.contains('='));
    }

    #[test]
    fn the_lead_in_is_added_or_every_id_is_wrong() {
        // Removing the 150-block lead-in yields a different, useless id. The
        // guard exists because the mistake is invisible: the id still looks
        // right, it just matches nothing on earth.
        let shifted: Vec<u32> = FEAR_INOCULUM_STARTS.iter().map(|s| s + LEAD_IN).collect();
        assert_ne!(
            disc_id(&shifted, FEAR_INOCULUM_LEADOUT + LEAD_IN).as_deref(),
            Some(FEAR_INOCULUM_ID)
        );
    }

    #[test]
    fn a_disc_that_cannot_have_an_id_gets_none_rather_than_a_plausible_string() {
        assert_eq!(disc_id(&[], 100), None);
        let too_many: Vec<u32> = (0..100).collect();
        assert_eq!(disc_id(&too_many, 200_000), None);
    }

    #[test]
    fn two_different_discs_do_not_share_an_id() {
        let other = [0u32, 46_500, 100_047, 157_373, 218_718, 264_120, 285_735];
        assert_ne!(
            disc_id(&other, FEAR_INOCULUM_LEADOUT),
            disc_id(&FEAR_INOCULUM_STARTS, FEAR_INOCULUM_LEADOUT)
        );
    }

    #[test]
    fn the_lookup_url_carries_what_a_title_needs() {
        let u = lookup_url(FEAR_INOCULUM_ID);
        assert!(u.contains(FEAR_INOCULUM_ID));
        // Without `recordings` the answer has releases but no track titles,
        // which is the entire point of asking. `release-groups` matters for a
        // different reason: cover art is far more reliably attached to the
        // GROUP than to one pressing.
        assert!(u.contains("inc=recordings"));
        assert!(u.contains("release-groups"));
        assert!(u.contains("fmt=json"));
    }
}
