//! SQL fragments for metadata-based album grouping in the Local Library.
//!
//! The metadata-grouped Albums view groups tracks by `album` +
//! `COALESCE(album_artist, artist)` when the album tag is usable, falls
//! back to the existing folder-based `album_group_key` when it's not,
//! and dumps anything else into a single `__unknown_album__` bucket.
//!
//! Both `get_albums_metadata_grouped` and `get_album_tracks_metadata`
//! must produce the same group_key for the same row, so the expression
//! is centralized here.

/// What an "album" IS in the Local Library Albums view — user-selectable
/// (header dropdown + Settings > Local Library). Compilations and box sets
/// favour `Folder`; carefully-tagged libraries may prefer `Metadata`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumGroupMode {
    /// One group per `album|artist` metadata pair (the #411 behavior).
    Metadata,
    /// One group per album-root folder — a compilation with 10 track
    /// artists is ONE album (the owner default).
    Folder,
}

impl AlbumGroupMode {
    /// Parse the persisted/UI string ("folder" | "metadata"). Folder is the
    /// default for anything unrecognized (least surprising for
    /// compilation-heavy or tag-imperfect libraries).
    pub fn from_pref(s: &str) -> Self {
        match s {
            "metadata" => Self::Metadata,
            _ => Self::Folder,
        }
    }

    /// The persisted/UI string form.
    pub fn as_pref(&self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Folder => "folder",
        }
    }
}

/// SQL expression that produces the metadata group key for a row of
/// `local_tracks`. Insert wherever you would otherwise use a column.
pub fn metadata_group_key_sql_expression() -> &'static str {
    r#"CASE
        WHEN album IS NOT NULL
          AND TRIM(album) != ''
          AND album != 'Unknown Album'
        THEN album || '|' || COALESCE(album_artist, artist, 'Unknown Artist')

        WHEN album_group_key IS NOT NULL
          AND album_group_key != ''
        THEN album_group_key

        ELSE '__unknown_album__'
    END"#
}

/// SQL expression that produces the FOLDER group key for a row of
/// `local_tracks` — one album per album-root folder, no metadata splitting.
/// The orphan bucket is kept for rows with no folder key at all.
pub fn folder_group_key_sql_expression() -> &'static str {
    r#"CASE
        WHEN album_group_key IS NOT NULL
          AND album_group_key != ''
        THEN album_group_key

        ELSE '__unknown_album__'
    END"#
}

/// The group-key SQL expression for a mode.
pub fn group_key_sql_expression(mode: AlbumGroupMode) -> &'static str {
    match mode {
        AlbumGroupMode::Metadata => metadata_group_key_sql_expression(),
        AlbumGroupMode::Folder => folder_group_key_sql_expression(),
    }
}

/// Sentinel group_key value used for the orphan bucket. Frontend can
/// special-case this if it needs a localized label.
pub const UNKNOWN_ALBUM_GROUP_KEY: &str = "__unknown_album__";
