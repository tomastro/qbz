//! What a CD's table of contents IS, independent of how it was read.
//!
//! Linux asks the kernel through ioctls; macOS mounts the disc and leaves a
//! `.TOC.plist` next to the audio files. The two reads share nothing, and the
//! ANSWER has to be identical or the disc store, the MusicBrainz lookup and
//! the rip log would need a second version each.
//!
//! MEASURED, both platforms, the owner's Tool — *Fear Inoculum* (2026-08-21):
//! the Linux ioctl and the macOS plist (minus its 150-sector lead-in) produce
//! byte-identical answers — DiscID `BeNBMsD8Du5NO2W61Yk.B2jwwIs-` and
//! fingerprint `38bef21351f7fca3` from both. That equality is the whole reason
//! these types live here instead of inside one platform's reader.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CdError {
    #[error("no optical drive found")]
    NoDrive,
    #[error("cannot open {0}: {1}")]
    Open(PathBuf, std::io::Error),
    #[error("no disc in the drive")]
    NoDisc,
    #[error("the disc has no audio tracks")]
    NotAudio,
    #[error("the drive refused to read audio at sector {lsn}: {source}")]
    Read {
        lsn: u32,
        #[source]
        source: std::io::Error,
    },
    #[error("reading the table of contents failed: {0}")]
    Toc(std::io::Error),
}

/// One entry of the disc's table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocTrack {
    pub number: u8,
    pub start_lsn: u32,
    /// Sectors up to the next track (or the lead-out for the last one).
    pub sectors: u32,
    /// False for the data track of a mixed-mode disc.
    pub is_audio: bool,
}

impl TocTrack {
    pub fn duration_secs(&self) -> u64 {
        crate::sectors_to_secs(self.sectors)
    }
    /// PCM frames (sample pairs) this track decodes to.
    pub fn frames(&self) -> u64 {
        self.sectors as u64 * 588
    }
}

#[derive(Debug, Clone)]
pub struct Toc {
    pub device: PathBuf,
    pub tracks: Vec<TocTrack>,
    pub leadout_lsn: u32,
}

impl Toc {
    /// The AUDIO tracks, in disc order. A caller that wants to play something
    /// should use this and never `tracks` — the data track of an enhanced CD
    /// lives in the latter.
    pub fn audio_tracks(&self) -> impl Iterator<Item = &TocTrack> {
        self.tracks.iter().filter(|t| t.is_audio)
    }

    /// A stable identity for the MEDIUM, not the drive.
    ///
    /// `/dev/sr0` names a piece of hardware; two different discs in it are two
    /// different sessions, and a persisted queue that resolves against the
    /// drive would happily play track 3 of whatever is loaded now. The digest
    /// is over every track's start and length plus the lead-out, which is what
    /// a disc actually is. (The disc's own MCN would be tidier — the owner's
    /// Fear Inoculum reports all zeros, so it is not dependable.)
    pub fn fingerprint(&self) -> String {
        // FNV-1a: no dependency, and this only has to be stable and
        // collision-resistant enough to tell two albums apart.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |v: u32| {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
        };
        for t in &self.tracks {
            eat(t.start_lsn);
            eat(t.sectors);
            eat(t.number as u32);
        }
        eat(self.leadout_lsn);
        format!("{h:016x}")
    }
}

