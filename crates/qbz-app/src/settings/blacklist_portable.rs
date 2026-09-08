//! A portable blacklist: the blocked artists and albums as one JSON file a
//! user can hand to another QBZ user, or carry to another account.
//!
//! Frontend-agnostic on purpose (ADR 006): the Qt shell and the daemon both
//! read and write the same document. The import is ADDITIVE — it never
//! removes an entry, never rewrites one that is already blocked, and never
//! touches the enabled flag — so importing a friend's file twice, or your
//! own old file over a newer list, is always safe.

use serde::{Deserialize, Serialize};

use super::artist_blacklist::BlacklistService;

/// The `kind` discriminator; a file without it is not ours.
pub const KIND: &str = "qbz-blacklist";
/// Bumped only when a reader would misread an older file. Readers accept
/// any version up to their own.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableArtist {
    pub artist_id: u64,
    pub artist_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableAlbum {
    pub album_id: String,
    pub album_title: String,
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub cover_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlacklistBundle {
    pub kind: String,
    pub schema_version: u32,
    /// RFC 3339, informational.
    pub created_at: String,
    #[serde(default)]
    pub artists: Vec<PortableArtist>,
    #[serde(default)]
    pub albums: Vec<PortableAlbum>,
}

/// What an import did. `existing` entries were already blocked and were
/// left exactly as they were (their own notes and dates win).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub artists_added: usize,
    pub artists_existing: usize,
    pub albums_added: usize,
    pub albums_existing: usize,
}

impl ImportReport {
    pub fn added(&self) -> usize {
        self.artists_added + self.albums_added
    }
    pub fn existing(&self) -> usize {
        self.artists_existing + self.albums_existing
    }
}

/// Snapshot the store into a bundle. Ids, names and notes only: the
/// `added_at` stamps stay local (the importer stamps its own).
pub fn export(service: &BlacklistService) -> Result<BlacklistBundle, String> {
    let artists = service
        .get_all()?
        .into_iter()
        .map(|a| PortableArtist {
            artist_id: a.artist_id,
            artist_name: a.artist_name,
            notes: a.notes,
        })
        .collect();
    let albums = service
        .get_all_albums()?
        .into_iter()
        .map(|a| PortableAlbum {
            album_id: a.album_id,
            album_title: a.album_title,
            artist_name: a.artist_name,
            cover_url: a.cover_url,
            notes: a.notes,
        })
        .collect();
    Ok(BlacklistBundle {
        kind: KIND.to_string(),
        schema_version: SCHEMA_VERSION,
        created_at: chrono::Utc::now().to_rfc3339(),
        artists,
        albums,
    })
}

/// Parse and validate a bundle. A file of another kind or from a newer
/// schema is refused rather than half-read.
pub fn parse(text: &str) -> Result<BlacklistBundle, String> {
    let bundle: BlacklistBundle =
        serde_json::from_str(text).map_err(|e| format!("not a QBZ blacklist file: {e}"))?;
    if bundle.kind != KIND {
        return Err(format!("not a QBZ blacklist file (kind `{}`)", bundle.kind));
    }
    if bundle.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "blacklist file schema {} is newer than this QBZ understands ({})",
            bundle.schema_version, SCHEMA_VERSION
        ));
    }
    Ok(bundle)
}

/// Additive merge into the store. Entries already blocked are skipped
/// (`INSERT OR REPLACE` would otherwise overwrite their notes and dates);
/// the enabled flag is not touched.
pub fn import(
    service: &BlacklistService,
    bundle: &BlacklistBundle,
) -> Result<ImportReport, String> {
    // Existence by STORED id, not `is_blacklisted`: that predicate also
    // folds in the enabled flag, and a disabled blacklist must still keep
    // its rows through an import.
    let existing_artists: std::collections::HashSet<u64> = service
        .get_all()?
        .into_iter()
        .map(|a| a.artist_id)
        .collect();
    let existing_albums: std::collections::HashSet<String> = service
        .get_all_albums()?
        .into_iter()
        .map(|a| a.album_id)
        .collect();
    let mut report = ImportReport::default();
    for artist in &bundle.artists {
        if existing_artists.contains(&artist.artist_id) {
            report.artists_existing += 1;
            continue;
        }
        service.add(
            artist.artist_id,
            &artist.artist_name,
            artist.notes.as_deref(),
        )?;
        report.artists_added += 1;
    }
    for album in &bundle.albums {
        if album.album_id.trim().is_empty() {
            continue;
        }
        if existing_albums.contains(&album.album_id) {
            report.albums_existing += 1;
            continue;
        }
        service.add_album(
            &album.album_id,
            &album.album_title,
            &album.artist_name,
            &album.cover_url,
            album.notes.as_deref(),
        )?;
        report.albums_added += 1;
    }
    Ok(report)
}

/// `qbz-blacklist-YYYYMMDD.json`, next to the settings bundle's naming.
pub fn default_filename() -> String {
    format!("qbz-blacklist-{}.json", chrono::Utc::now().format("%Y%m%d"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> BlacklistService {
        BlacklistService::new_in_memory().expect("in-memory blacklist")
    }

    #[test]
    fn export_then_import_into_an_empty_store_is_lossless() {
        let src = store();
        src.add(1, "Anthrax", Some("wrong Anthrax")).unwrap();
        src.add(2, "Nickelback", None).unwrap();
        src.add_album(
            "abc123",
            "Bad Album",
            "Someone",
            "https://x/cover.jpg",
            None,
        )
        .unwrap();
        let bundle = export(&src).unwrap();
        assert_eq!(bundle.kind, KIND);
        assert_eq!(bundle.artists.len(), 2);
        assert_eq!(bundle.albums.len(), 1);

        let text = serde_json::to_string(&bundle).unwrap();
        let dst = store();
        let report = import(&dst, &parse(&text).unwrap()).unwrap();
        assert_eq!(report.artists_added, 2);
        assert_eq!(report.albums_added, 1);
        assert_eq!(report.existing(), 0);
        assert!(dst.is_blacklisted(1));
        assert!(dst.is_album_blacklisted("abc123"));
        let anthrax = dst
            .get_all()
            .unwrap()
            .into_iter()
            .find(|a| a.artist_id == 1)
            .unwrap();
        assert_eq!(anthrax.notes.as_deref(), Some("wrong Anthrax"));
    }

    #[test]
    fn import_is_additive_and_leaves_existing_entries_alone() {
        let dst = store();
        dst.add(1, "Anthrax (thrash)", Some("mine")).unwrap();
        dst.set_enabled(false).unwrap();
        let bundle = BlacklistBundle {
            kind: KIND.into(),
            schema_version: SCHEMA_VERSION,
            created_at: String::new(),
            artists: vec![
                PortableArtist {
                    artist_id: 1,
                    artist_name: "Anthrax".into(),
                    notes: Some("theirs".into()),
                },
                PortableArtist {
                    artist_id: 3,
                    artist_name: "Creed".into(),
                    notes: None,
                },
            ],
            albums: vec![],
        };
        let report = import(&dst, &bundle).unwrap();
        assert_eq!(report.artists_added, 1);
        assert_eq!(report.artists_existing, 1);
        // The existing row kept its own name and notes; the flag is untouched.
        let mine = dst
            .get_all()
            .unwrap()
            .into_iter()
            .find(|a| a.artist_id == 1)
            .unwrap();
        assert_eq!(mine.artist_name, "Anthrax (thrash)");
        assert_eq!(mine.notes.as_deref(), Some("mine"));
        assert!(!dst.is_enabled());
        // Importing the same file again changes nothing.
        let again = import(&dst, &bundle).unwrap();
        assert_eq!(again.added(), 0);
        assert_eq!(again.existing(), 2);
    }

    #[test]
    fn parse_refuses_other_files_and_newer_schemas() {
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"kind":"qbz-settings","schema_version":1,"created_at":""}"#).is_err());
        assert!(parse(&format!(
            r#"{{"kind":"qbz-blacklist","schema_version":{},"created_at":""}}"#,
            SCHEMA_VERSION + 1
        ))
        .is_err());
        let ok = parse(r#"{"kind":"qbz-blacklist","schema_version":1,"created_at":"x"}"#).unwrap();
        assert!(ok.artists.is_empty() && ok.albums.is_empty());
    }
}
