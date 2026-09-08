//! API endpoint definitions

pub const BASE_URL: &str = "https://www.qobuz.com/api.json/0.2";

/// Endpoint paths
pub mod paths {
    // User
    pub const USER_GET: &str = "/user/get";
    pub const USER_LOGIN: &str = "/user/login";

    // OAuth
    pub const OAUTH_CALLBACK: &str = "/oauth/callback";

    // Track
    pub const TRACK_GET: &str = "/track/get";
    pub const TRACK_GET_LIST: &str = "/track/getList";
    pub const TRACK_SEARCH: &str = "/track/search";
    pub const TRACK_GET_FILE_URL: &str = "/track/getFileUrl";
    // Lyrics (v9.9.0.0-beta delta — qobuz-api-inferred-openapi-v9.9.0.0-beta.yaml:759-806).
    // Returns URL(s) to the lyrics document; the document itself is a second
    // GET against the returned `lyrics_url` (Qobuz CDN host).
    pub const TRACK_LYRICS_URL: &str = "/track/lyricsUrl";

    // Album
    pub const ALBUM_GET: &str = "/album/get";
    pub const ALBUM_SEARCH: &str = "/album/search";
    pub const ALBUM_GET_FEATURED: &str = "/album/getFeatured";
    pub const ALBUM_SUGGEST: &str = "/album/suggest";
    pub const DYNAMIC_SUGGEST: &str = "/dynamic/suggest";

    // Radio (generated track lists)
    pub const RADIO_ALBUM: &str = "/radio/album";
    pub const RADIO_ARTIST: &str = "/radio/artist";
    pub const RADIO_TRACK: &str = "/radio/track";

    // Artist
    pub const ARTIST_GET: &str = "/artist/get";
    pub const ARTIST_SEARCH: &str = "/artist/search";
    pub const ARTIST_GET_SIMILAR: &str = "/artist/getSimilarArtists";
    pub const ARTIST_PAGE: &str = "/artist/page";
    pub const ARTIST_RELEASES_GRID: &str = "/artist/getReleasesGrid";
    pub const ARTIST_RELEASES_LIST: &str = "/artist/getReleasesList";
    pub const ARTIST_STORY: &str = "/artist/story";

    // Playlist
    pub const PLAYLIST_GET: &str = "/playlist/get";
    pub const PLAYLIST_SEARCH: &str = "/playlist/search";
    pub const PLAYLIST_GET_USER_PLAYLISTS: &str = "/playlist/getUserPlaylists";
    pub const PLAYLIST_CREATE: &str = "/playlist/create";
    pub const PLAYLIST_DELETE: &str = "/playlist/delete";
    pub const PLAYLIST_ADD_TRACKS: &str = "/playlist/addTracks";
    pub const PLAYLIST_DELETE_TRACKS: &str = "/playlist/deleteTracks";
    pub const PLAYLIST_UPDATE: &str = "/playlist/update";
    // Reorder tracks WITHIN a playlist. Not in
    // qobuz-api-inferred-openapi-v10.0.0.0-beta.yaml; the shape comes from the
    // DESKTOP web-player capture
    // (qbz-nix-docs/qobuz-api-delta-desktop-8.2.0-b015.md:95), not from the
    // decompiled Android route table, which CLAUDE.md forbids inferring
    // endpoints from. Distinct from /playlist/updatePlaylistsPosition, which
    // reorders PLAYLISTS.
    pub const PLAYLIST_UPDATE_TRACKS_POSITION: &str = "/playlist/updateTracksPosition";
    pub const PLAYLIST_GET_TAGS: &str = "/playlist/getTags";
    pub const PLAYLIST_SUBSCRIBE: &str = "/playlist/subscribe";
    pub const PLAYLIST_UNSUBSCRIBE: &str = "/playlist/unsubscribe";

    // Favorites
    pub const FAVORITE_GET_USER_FAVORITES: &str = "/favorite/getUserFavorites";
    pub const FAVORITE_CREATE: &str = "/favorite/create";
    pub const FAVORITE_DELETE: &str = "/favorite/delete";
    // New releases from followed artists / labels / awards — the "Radar de
    // Novedades" feed in the mobile Qobuz client. Path confirmed in
    // qbz-nix-docs/qobuz-api-inferred-openapi-v9.7.0.3.yaml
    // (source: x20/b.java Retrofit interface).
    pub const FAVORITE_GET_NEW_RELEASES: &str = "/favorite/getNewReleases";

    // Purchase (DRM-free purchases library)
    pub const PURCHASE_GET_USER_PURCHASES: &str = "/purchase/getUserPurchases";
    // NOT in the OpenAPI spec — code is the source of truth for its
    // {albums:{items,total,offset,limit}, tracks:{...}} envelope. The UI reads
    // only `.total` per type.
    pub const PURCHASE_GET_USER_PURCHASES_IDS: &str = "/purchase/getUserPurchasesIds";

    // Label (v9.7.0.3 API — /label/get removed, use /label/getAlbums
    // + /label/page for the same coverage).
    pub const LABEL_PAGE: &str = "/label/page";
    pub const LABEL_EXPLORE: &str = "/label/explore";
    pub const LABEL_GET_ALBUMS: &str = "/label/getAlbums";
    pub const LABEL_GET_NEXT_RELEASES: &str = "/label/getNextReleases";
    pub const LABEL_GET_AWARDED_RELEASES: &str = "/label/getAwardedReleases";
    pub const LABEL_GET_PLAYLISTS: &str = "/label/getPlaylists";
    pub const LABEL_GET_TOP_ARTISTS: &str = "/label/getTopArtists";
    pub const LABEL_STORY: &str = "/label/story";
    pub const LABEL_GET_LIST: &str = "/label/getList";

    // Award
    pub const AWARD_PAGE: &str = "/award/page";
    pub const AWARD_GET_ALBUMS: &str = "/award/getAlbums";
    pub const AWARD_EXPLORE: &str = "/award/explore";

    // Genre
    pub const GENRE_LIST: &str = "/genre/list";

    // Session (CMAF streaming)
    pub const SESSION_START: &str = "/session/start";

    // Qobuz Connect cloud authentication. Both are form-encoded POSTs; see
    // qobuz-api-inferred-openapi-v10.0.0.0-beta.yaml.
    pub const QWS_CREATE_TOKEN: &str = "/qws/createToken";
    pub const QWS_DELEGATE_AUTH: &str = "/qws/delegateAuth";

    // File (CMAF streaming)
    pub const FILE_URL: &str = "/file/url";

    // Catalog (combined search)
    pub const CATALOG_SEARCH: &str = "/catalog/search";

    // Discover (home page content)
    pub const DISCOVER_INDEX: &str = "/discover/index";
    pub const DISCOVER_PLAYLISTS: &str = "/discover/playlists";
    pub const DISCOVER_NEW_RELEASES: &str = "/discover/newReleases";
    pub const DISCOVER_IDEAL_DISCOGRAPHY: &str = "/discover/idealDiscography";
    pub const DISCOVER_MOST_STREAMED: &str = "/discover/mostStreamed";
    pub const DISCOVER_QOBUZISSIMS: &str = "/discover/qobuzissims";
    pub const DISCOVER_ALBUM_OF_THE_WEEK: &str = "/discover/albumOfTheWeek";
    pub const DISCOVER_PRESS_AWARD: &str = "/discover/pressAward";
}

/// Build full URL for an endpoint
pub fn build_url(endpoint: &str) -> String {
    format!("{}{}", BASE_URL, endpoint)
}
