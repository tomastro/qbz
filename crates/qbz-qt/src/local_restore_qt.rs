//! Per-profile Local Library navigation preferences. The in-memory history
//! still owns Back/Forward; this small document carries browser choices over
//! a process restart without persisting the rest of the navigation stack.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static WRITE: Mutex<()> = Mutex::new(());
const MAX_STATE_BYTES: usize = 64 * 1024;
static STARTUP_CHECKED: AtomicBool = AtomicBool::new(false);
static CURRENT_ALBUM: Mutex<Option<(PathBuf, AlbumRoute)>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AlbumRoute {
    pub id: String,
    #[serde(default)]
    pub filter_json: String,
}

impl AlbumRoute {
    fn valid(&self) -> bool {
        !self.id.trim().is_empty()
            && self.id.len() <= 8192
            && self.filter_json.len() <= MAX_STATE_BYTES
            && (self.filter_json.is_empty()
                || serde_json::from_str::<Value>(&self.filter_json)
                    .is_ok_and(|value| value.is_object()))
    }
}

pub fn note_album_request(id: &str, filter_json: &str) {
    let Some(path) = path() else { return };
    let route = AlbumRoute {
        id: id.into(),
        filter_json: filter_json.into(),
    };
    if route.valid() {
        *CURRENT_ALBUM.lock().unwrap_or_else(|e| e.into_inner()) = Some((path, route));
    }
}

fn save_session_at(path: &Path, view: &str, route: Option<AlbumRoute>) {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut doc) = crate::settings_qt::read_json_object(path) else {
        return;
    };
    let before = doc.get("album").cloned();
    let route = route.filter(|route| view == "localalbum" && route.valid());
    if let Some(route) = route {
        doc.insert("album".into(), serde_json::json!(route));
    } else {
        doc.remove("album");
    }
    if before.as_ref() != doc.get("album") {
        crate::settings_qt::write_json_object_atomic(path, &doc);
    }
}

/// Capture the final route on clean exit, before the existing shutdown work.
/// Visiting Home/Settings/another album afterwards must not leave an old
/// local-album request masquerading as the last page. A login-only launch
/// never replaces the saved session, and another account has a separate file.
pub fn save_session_on_exit() {
    if !STARTUP_CHECKED.load(Ordering::Acquire) {
        return;
    }
    let Some(path) = path() else { return };
    let route = CURRENT_ALBUM
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(owner, _)| owner == &path)
        .map(|(_, route)| route.clone());
    save_session_at(&path, &crate::nav_qt::current_view(), route);
}

fn startup_album_at(
    path: &Path,
    remember: bool,
    crash_level: u8,
    link: bool,
    kiosk: bool,
) -> Option<AlbumRoute> {
    if !remember || crash_level >= 2 || link || kiosk {
        return None;
    }
    let doc = crate::settings_qt::read_json_object(path)?;
    let route: AlbumRoute = serde_json::from_value(doc.get("album")?.clone()).ok()?;
    route.valid().then_some(route)
}

/// Only the first session entry of this process can restore a detail. An
/// explicit launcher link outranks remembered navigation, even while offline.
pub fn take_startup_album() -> Option<AlbumRoute> {
    if STARTUP_CHECKED.swap(true, Ordering::AcqRel) {
        return None;
    }
    startup_album_at(
        &path()?,
        crate::settings_qt::pref_str("startup_page", "home") == "remember",
        crate::nav_qt::crash_level(),
        crate::deep_link_qt::has_pending(),
        crate::kiosk_profile_qt::active(),
    )
}

fn path() -> Option<PathBuf> {
    use qbz_app::user_data::UserDataPaths;
    // Same identity as local_state::db_path, including the guest profile.
    let user = UserDataPaths::load_last_user_id().unwrap_or(0);
    Some(
        UserDataPaths::data_dir_for(user)
            .ok()?
            .join("local_navigation_qt.json"),
    )
}

fn browser_state(json: &str) -> Option<Value> {
    if json.len() > MAX_STATE_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(json).ok()?;
    let input = value.as_object()?;
    let mut state = Map::new();
    for key in [
        "activeTab",
        "genresSearch",
        "genreYearsSearch",
        "genreArtistsSearch",
        "genreAlbumsSearch",
        "genresView",
        "genresSort",
        "explorerColumns",
    ] {
        if let Some(Value::String(text)) = input.get(key) {
            state.insert(key.into(), Value::String(text.clone()));
        }
    }
    for key in [
        "selectedGenres",
        "selectedGenreYears",
        "selectedGenreArtists",
        "selectedGenreAlbums",
    ] {
        if let Some(Value::Object(selections)) = input.get(key) {
            state.insert(
                key.into(),
                Value::Object(
                    selections
                        .iter()
                        .filter(|(_, selected)| selected.as_bool() == Some(true))
                        .map(|(key, selected)| (key.clone(), selected.clone()))
                        .collect(),
                ),
            );
        }
    }
    if let Some(Value::Bool(collapsed)) = input.get("genresBrowserCollapsed") {
        state.insert("genresBrowserCollapsed".into(), Value::Bool(*collapsed));
    }
    if state
        .get("activeTab")
        .and_then(Value::as_str)
        .is_some_and(|tab| !["genres", "albums", "artists", "folders", "tracks"].contains(&tab))
    {
        state.remove("activeTab");
    }
    Some(Value::Object(state))
}

fn save_browser_at(path: &Path, json: &str) {
    let Some(state) = browser_state(json) else {
        return;
    };
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut doc) = crate::settings_qt::read_json_object(path) else {
        return;
    };
    if doc.get("browser") != Some(&state) {
        doc.insert("browser".into(), state);
        crate::settings_qt::write_json_object_atomic(path, &doc);
    }
}

pub fn save_browser(json: &str) {
    if let Some(path) = path() {
        save_browser_at(&path, json);
    }
}

fn browser_at(path: &Path) -> Option<String> {
    let doc = crate::settings_qt::read_json_object(path)?;
    browser_state(&doc.get("browser")?.to_string()).map(|state| state.to_string())
}

pub fn browser_json() -> String {
    // A saved UI state must not trap the next boot after a crash. The file
    // remains intact; ordinary navigation can overwrite it with fresh choices.
    if crate::nav_qt::crash_level() >= 2 {
        return String::new();
    }
    path()
        .and_then(|path| browser_at(&path))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clean_exit_restores_the_album_but_a_later_home_exit_clears_it() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("users/1/local_navigation_qt.json");
        let other = temp.path().join("users/2/local_navigation_qt.json");
        let route = AlbumRoute {
            id: "local:album with spaces".into(),
            filter_json: r#"{"sources":["local"],"quality":["hires"]}"#.into(),
        };
        save_browser_at(
            &file,
            r#"{"activeTab":"albums","selectedGenreYears":{"2001":true}}"#,
        );
        save_session_at(&file, "localalbum", Some(route.clone()));
        assert_eq!(
            startup_album_at(&file, true, 1, false, false),
            Some(route.clone())
        );
        assert!(startup_album_at(&other, true, 1, false, false).is_none());
        for (remember, crash, link, kiosk) in [
            (false, 1, false, false),
            (true, 2, false, false),
            (true, 1, true, false),
            (true, 1, false, true),
        ] {
            assert!(startup_album_at(&file, remember, crash, link, kiosk).is_none());
        }
        assert_eq!(
            startup_album_at(&file, true, 1, false, false),
            Some(route.clone()),
            "bypassing restore must preserve the saved file"
        );
        save_session_at(&file, "home", Some(route));
        assert!(startup_album_at(&file, true, 1, false, false).is_none());
        assert_eq!(
            serde_json::from_str::<Value>(&browser_at(&file).unwrap()).unwrap()["activeTab"],
            "albums"
        );
    }

    #[test]
    fn invalid_saved_album_context_falls_back_to_the_ordinary_entry() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("local_navigation_qt.json");
        for album in [
            json!({"id":""}),
            json!({"id":7}),
            json!({"id":"local:a", "filter_json":"[1]"}),
            json!({"id":"local:a", "filter_json":"bad"}),
        ] {
            std::fs::write(&file, json!({"album":album}).to_string()).unwrap();
            assert!(startup_album_at(&file, true, 1, false, false).is_none());
        }
    }

    #[test]
    fn browser_choices_survive_reopen_and_stay_in_their_profile() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("users/1/local_navigation_qt.json");
        let second = temp.path().join("users/2/local_navigation_qt.json");
        let choices = json!({"activeTab":"genres", "genresSearch":"rock",
            "genreYearsSearch":"199", "genreArtistsSearch":"band", "genreAlbumsSearch":"live",
            "selectedGenres":{"rock":true}, "selectedGenreYears":{"1994":true},
            "selectedGenreArtists":{"band":true}, "selectedGenreAlbums":{"local:a":true},
            "explorerColumns":"both", "genresView":"grid"});
        save_browser_at(&first, &choices.to_string());
        assert_eq!(
            serde_json::from_str::<Value>(&browser_at(&first).unwrap()).unwrap(),
            choices
        );
        assert!(browser_at(&second).is_none());
        save_browser_at(&second, r#"{"activeTab":"albums","selectedGenres":{}}"#);
        assert_eq!(
            serde_json::from_str::<Value>(&browser_at(&first).unwrap()).unwrap(),
            choices
        );
        save_browser_at(
            &first,
            r#"{"activeTab":"genres","selectedGenres":{},"genresSearch":""}"#,
        );
        let reopened: Value = serde_json::from_str(&browser_at(&first).unwrap()).unwrap();
        assert_eq!(reopened["selectedGenres"], json!({}));
        assert_eq!(reopened["genresSearch"], "");
    }

    #[test]
    fn bad_state_cannot_replace_choices_or_other_document_fields() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("local_navigation_qt.json");
        std::fs::write(&file, r#"{"future":42,"browser":{"activeTab":"genres"}}"#).unwrap();
        for invalid in [
            "null".to_string(),
            "[1]".into(),
            "{".into(),
            "x".repeat(MAX_STATE_BYTES + 1),
        ] {
            save_browser_at(&file, &invalid);
        }
        assert_eq!(
            browser_at(&file).as_deref(),
            Some(r#"{"activeTab":"genres"}"#)
        );
        save_browser_at(
            &file,
            r#"{"activeTab":"bad-route","selectedGenres":{"ok":true,"bad":"true"},"unrelated":7}"#,
        );
        let doc: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(doc["future"], 42);
        assert_eq!(doc["browser"], json!({"selectedGenres":{"ok":true}}));
        std::fs::write(&file, "broken document").unwrap();
        assert!(browser_at(&file).is_none());
        save_browser_at(&file, r#"{"activeTab":"albums"}"#);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "broken document");
    }
}
