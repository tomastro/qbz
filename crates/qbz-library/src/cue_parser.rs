//! CUE sheet parser for single-file albums

use std::fs;
use std::path::Path;

use crate::{AudioFormat, AudioProperties, LibraryError, LocalTrack, MetadataExtractor};

/// Parsed CUE sheet
#[derive(Debug, Clone)]
pub struct CueSheet {
    /// Path to the .cue file
    pub file_path: String,
    /// Referenced audio file (resolved to absolute path)
    pub audio_file: String,
    /// Distinct audio files named by FILE directives. QBZ only expands a
    /// sheet into virtual tracks when this contains exactly one entry; a
    /// multi-file CUE describes files that are already split and is treated
    /// as a sidecar instead of a second copy of the album.
    audio_files: Vec<String>,
    /// Album title
    pub title: Option<String>,
    /// Album performer/artist
    pub performer: Option<String>,
    /// Tracks in the CUE sheet
    pub tracks: Vec<CueTrack>,
}

impl CueSheet {
    /// Whether every track in this sheet addresses one shared audio image.
    ///
    /// `cue_to_tracks` models virtual ranges inside a single decoder source.
    /// A CUE with several distinct FILE directives is a metadata sidecar for
    /// already-split files and must not enter that path.
    pub fn is_single_file_image(&self) -> bool {
        self.audio_files.len() == 1
    }

    /// Number of distinct audio files referenced by the sheet.
    pub fn audio_file_count(&self) -> usize {
        self.audio_files.len()
    }
}

/// A track within a CUE sheet
#[derive(Debug, Clone)]
pub struct CueTrack {
    /// Track number
    pub number: u32,
    /// Track title
    pub title: String,
    /// Track performer (if different from album)
    pub performer: Option<String>,
    /// Start time in seconds
    pub start_secs: f64,
}

/// CUE time format (MM:SS:FF where FF is frames, 75 frames per second)
#[derive(Debug, Clone, Copy)]
pub struct CueTime {
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
}

impl CueTime {
    /// Convert to seconds (frames are 1/75 second)
    pub fn to_seconds(&self) -> f64 {
        self.minutes as f64 * 60.0 + self.seconds as f64 + self.frames as f64 / 75.0
    }

    /// Parse "MM:SS:FF" format
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(CueTime {
            minutes: parts[0].parse().ok()?,
            seconds: parts[1].parse().ok()?,
            frames: parts[2].parse().ok()?,
        })
    }
}

/// CUE sheet parser
pub struct CueParser;

impl CueParser {
    /// Parse a CUE file
    pub fn parse(cue_path: &Path) -> Result<CueSheet, LibraryError> {
        log::debug!("Parsing CUE sheet");

        // Try UTF-8 first, then fall back to Latin-1
        let content = fs::read_to_string(cue_path).or_else(|_| {
            let bytes = fs::read(cue_path)?;
            Ok::<String, std::io::Error>(bytes.iter().map(|&b| b as char).collect())
        })?;

        Self::parse_content(&content, cue_path)
    }

    /// Parse CUE content
    fn parse_content(content: &str, cue_path: &Path) -> Result<CueSheet, LibraryError> {
        let mut sheet = CueSheet {
            file_path: cue_path.to_string_lossy().to_string(),
            audio_file: String::new(),
            audio_files: Vec::new(),
            title: None,
            performer: None,
            tracks: Vec::new(),
        };

        let mut current_track: Option<CueTrack> = None;
        let mut in_track = false;

        for line in content.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with("REM") {
                continue;
            }

            // Parse FILE "name" TYPE
            if line.to_uppercase().starts_with("FILE ") {
                if let Some(filename) = Self::extract_quoted(line) {
                    // Resolve path relative to CUE file
                    let resolved = if let Some(parent) = cue_path.parent() {
                        parent.join(&filename).to_string_lossy().into_owned()
                    } else {
                        filename
                    };
                    if sheet.audio_file.is_empty() {
                        sheet.audio_file.clone_from(&resolved);
                    }
                    if !sheet.audio_files.contains(&resolved) {
                        sheet.audio_files.push(resolved);
                    }
                }
            }
            // Parse album-level TITLE (before any TRACK)
            else if line.to_uppercase().starts_with("TITLE ") && !in_track {
                sheet.title = Self::extract_quoted(line);
            }
            // Parse album-level PERFORMER (before any TRACK)
            else if line.to_uppercase().starts_with("PERFORMER ") && !in_track {
                sheet.performer = Self::extract_quoted(line);
            }
            // Parse TRACK NN AUDIO
            else if line.to_uppercase().starts_with("TRACK ") {
                // Save previous track
                if let Some(track) = current_track.take() {
                    sheet.tracks.push(track);
                }

                // Start new track
                in_track = true;
                if let Some(num) = Self::extract_track_number(line) {
                    current_track = Some(CueTrack {
                        number: num,
                        title: format!("Track {}", num),
                        performer: None,
                        start_secs: 0.0,
                    });
                }
            }
            // Parse track TITLE
            else if line.to_uppercase().starts_with("TITLE ") && in_track {
                if let Some(ref mut track) = current_track {
                    if let Some(title) = Self::extract_quoted(line) {
                        track.title = title;
                    }
                }
            }
            // Parse track PERFORMER
            else if line.to_uppercase().starts_with("PERFORMER ") && in_track {
                if let Some(ref mut track) = current_track {
                    track.performer = Self::extract_quoted(line);
                }
            }
            // Parse INDEX 01 MM:SS:FF (track start time)
            else if line.to_uppercase().starts_with("INDEX 01 ") {
                if let Some(ref mut track) = current_track {
                    let time_str = line.get(9..).map(|s| s.trim()).unwrap_or("");
                    if let Some(time) = CueTime::parse(time_str) {
                        track.start_secs = time.to_seconds();
                    }
                }
            }
        }

        // Don't forget the last track
        if let Some(track) = current_track {
            sheet.tracks.push(track);
        }

        // Validate we have an audio file and at least one track
        if sheet.audio_file.is_empty() {
            return Err(LibraryError::CueParse(
                "No FILE directive found in CUE sheet".to_string(),
            ));
        }

        if sheet.tracks.is_empty() {
            return Err(LibraryError::CueParse(
                "No tracks found in CUE sheet".to_string(),
            ));
        }

        log::info!(
            "Parsed CUE: {} tracks, {} audio file(s)",
            sheet.tracks.len(),
            sheet.audio_file_count()
        );

        Ok(sheet)
    }

    /// Extract quoted string: COMMAND "value" -> value
    fn extract_quoted(line: &str) -> Option<String> {
        let start = line.find('"')?;
        let end = line.rfind('"')?;
        if end <= start {
            return None;
        }
        Some(line[start + 1..end].to_string())
    }

    /// Extract track number: "TRACK 01 AUDIO" -> 1
    fn extract_track_number(line: &str) -> Option<u32> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }
        parts[1].parse().ok()
    }
}

/// Convert a CUE sheet into LocalTrack entries
pub fn cue_to_tracks(
    cue: &CueSheet,
    audio_duration_secs: u64,
    format: AudioFormat,
    properties: &AudioProperties,
) -> Vec<LocalTrack> {
    // This converter expresses every row as a time range in one decoder
    // source. Feeding it a multi-file CUE used to bind all rows to the last
    // FILE directive and publish zero-duration duplicates of the split files.
    if !cue.is_single_file_image() || audio_duration_secs == 0 {
        return Vec::new();
    }

    let mut tracks = Vec::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let cue_audio_path = Path::new(&cue.audio_file);
    let (album_group_key, album_group_title) =
        MetadataExtractor::album_group_info(cue_audio_path, cue.title.as_deref());
    let inferred_disc = MetadataExtractor::infer_disc_number(cue_audio_path);

    for (i, cue_track) in cue.tracks.iter().enumerate() {
        // Calculate end time (next track's start or audio end)
        let requested_end = if i + 1 < cue.tracks.len() {
            cue.tracks[i + 1].start_secs
        } else {
            audio_duration_secs as f64
        };
        let end_secs = requested_end.min(audio_duration_secs as f64);

        let range_secs = end_secs - cue_track.start_secs;
        if !range_secs.is_finite() || range_secs <= 0.0 {
            log::warn!(
                "Ignoring unplayable CUE track {}: start={} end={} file={}",
                cue_track.number,
                cue_track.start_secs,
                end_secs,
                cue.audio_file
            );
            continue;
        }
        // A valid sub-second range is still playable; keep its visible
        // duration non-zero while cue_start/end retain frame precision.
        let duration = range_secs.ceil().max(1.0) as u64;

        tracks.push(LocalTrack {
            id: 0,
            file_path: cue.audio_file.clone(),
            title: cue_track.title.clone(),
            artist: cue_track
                .performer
                .clone()
                .or_else(|| cue.performer.clone())
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            album: cue
                .title
                .clone()
                .unwrap_or_else(|| "Unknown Album".to_string()),
            album_artist: cue.performer.clone(),
            album_group_key: album_group_key.clone(),
            album_group_title: album_group_title.clone(),
            track_number: Some(cue_track.number),
            disc_number: inferred_disc,
            year: None,
            genre: None,
            genres: Vec::new(),
            catalog_number: None,
            duration_secs: duration,
            format: format.clone(),
            bit_depth: properties.bit_depth,
            sample_rate: properties.sample_rate,
            channels: properties.channels,
            file_size_bytes: 0,
            cue_file_path: Some(cue.file_path.clone()),
            cue_start_secs: Some(cue_track.start_secs),
            cue_end_secs: Some(end_secs),
            artwork_path: None,
            collection_artwork_path: None,
            last_modified: 0,
            indexed_at: now,
            source: None,
            qobuz_track_id: None,
            isrc: None,
            musicbrainz_recording_id: None,
            musicbrainz_track_id: None,
            musicbrainz_release_id: None,
            musicbrainz_release_group_id: None,
            musicbrainz_artist_id: None,
            is_network_mount: false,
        });
    }

    tracks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cue_time_parse() {
        let time = CueTime::parse("03:45:22").unwrap();
        assert_eq!(time.minutes, 3);
        assert_eq!(time.seconds, 45);
        assert_eq!(time.frames, 22);

        let secs = time.to_seconds();
        assert!((secs - 225.293).abs() < 0.01);
    }

    #[test]
    fn test_extract_quoted() {
        assert_eq!(
            CueParser::extract_quoted("TITLE \"My Song\""),
            Some("My Song".to_string())
        );
        assert_eq!(
            CueParser::extract_quoted("FILE \"album.flac\" WAVE"),
            Some("album.flac".to_string())
        );
    }

    #[test]
    fn test_extract_track_number() {
        assert_eq!(CueParser::extract_track_number("TRACK 01 AUDIO"), Some(1));
        assert_eq!(CueParser::extract_track_number("TRACK 12 AUDIO"), Some(12));
    }

    #[test]
    fn multi_file_cue_is_a_split_file_sidecar_not_an_image() {
        let cue = CueParser::parse_content(
            "PERFORMER \"Opeth\"\n\
             TITLE \"Sorceress\"\n\
             FILE \"01. Persephone.flac\" WAVE\n\
               TRACK 01 AUDIO\n\
                 TITLE \"Persephone\"\n\
                 INDEX 01 00:00:00\n\
             FILE \"02. Sorceress.flac\" WAVE\n\
               TRACK 02 AUDIO\n\
                 TITLE \"Sorceress\"\n\
                 INDEX 01 00:00:00\n",
            Path::new("/music/Sorceress/disc.cue"),
        )
        .unwrap();

        assert_eq!(cue.audio_file_count(), 2);
        assert!(!cue.is_single_file_image());
        assert!(
            cue_to_tracks(&cue, 240, AudioFormat::Flac, &AudioProperties::default()).is_empty()
        );
    }

    #[test]
    fn invalid_or_out_of_bounds_ranges_are_not_published() {
        let cue = CueParser::parse_content(
            "FILE \"album.flac\" WAVE\n\
               TRACK 01 AUDIO\n\
                 TITLE \"Playable\"\n\
                 INDEX 01 00:00:00\n\
               TRACK 02 AUDIO\n\
                 TITLE \"Past EOF\"\n\
                 INDEX 01 10:00:00\n",
            Path::new("/music/album.cue"),
        )
        .unwrap();

        let tracks = cue_to_tracks(&cue, 300, AudioFormat::Flac, &AudioProperties::default());
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Playable");
        assert_eq!(tracks[0].duration_secs, 300);
    }
}
