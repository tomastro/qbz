// crates/qbzd/src/paths.rs — daemon profile roots (01-architecture.md §4.2).
// The daemon owns a fully separate profile from the desktop: `~/.config/qbzd`,
// `~/.local/share/qbzd`, `~/.cache/qbzd`. It NEVER opens desktop
// `~/.local/share/qbz/**` at runtime (that only happens inside
// `settings export --from desktop`, 04 §4.1 — out of scope here).
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRoots {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
}

impl ProfileRoots {
    /// Resolve the three profile roots.
    ///
    /// - `config_override`: the `--config <path>` argument (a FILE path); its
    ///   parent directory becomes the config root. `None` falls back to
    ///   `dirs::config_dir()/qbzd`.
    /// - `data_root_override`: the already-parsed `qbzd.toml` `data_root`
    ///   value (a container override, e.g. for a Pi SD-card layout). `None`
    ///   falls back to `dirs::data_dir()/qbzd`.
    ///
    /// Cache is `dirs::cache_dir()/qbzd` UNLESS `data_root` was overridden,
    /// in which case cache = `<data_root>/cache` — never
    /// `<data_root>/../qbzd-cache`, which would walk outside the container.
    ///
    /// The config directory is created (mode 0700 on unix) on first use;
    /// data/cache directories are created by their respective owners
    /// (the instance lock creates the data root, cache writers create theirs).
    pub fn resolve(config_override: Option<&Path>, data_root_override: Option<&Path>) -> Self {
        let config = match config_override {
            Some(config_file) => config_file
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            None => default_config_dir(),
        };
        let data = match data_root_override {
            Some(dir) => dir.to_path_buf(),
            None => default_data_dir(),
        };
        let cache = match data_root_override {
            Some(_) => data.join("cache"),
            None => default_cache_dir(),
        };

        ensure_config_dir(&config);

        Self { config, data, cache }
    }
}

fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("qbzd")
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("qbzd")
}

fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("qbzd")
}

#[cfg(unix)]
fn ensure_config_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::create_dir_all(dir) {
        Ok(()) => {
            if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
                log::warn!("could not set 0700 on config dir {}: {e}", dir.display());
            }
        }
        Err(e) => {
            log::warn!("could not create config dir {}: {e}", dir.display());
        }
    }
}

#[cfg(not(unix))]
fn ensure_config_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "qbzd-paths-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn config_override_uses_parent_dir_and_creates_it_0700() {
        let dir = scratch_dir("config-override");
        let _ = std::fs::remove_dir_all(&dir);
        let config_file = dir.join("nested").join("qbzd.toml");

        let roots = ProfileRoots::resolve(Some(&config_file), None);

        assert_eq!(roots.config, dir.join("nested"));
        let meta = std::fs::metadata(&roots.config).expect("config dir created");
        assert!(meta.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_root_override_places_cache_under_it_not_beside_it() {
        let data_dir = scratch_dir("data-override");

        let roots = ProfileRoots::resolve(None, Some(&data_dir));

        assert_eq!(roots.data, data_dir);
        assert_eq!(roots.cache, data_dir.join("cache"));
        // Never derived as a sibling of data_root.
        assert_ne!(roots.cache, data_dir.parent().unwrap().join("qbzd-cache"));
    }

    #[test]
    fn defaults_resolve_under_xdg_roots_without_touching_real_home() {
        // SAFETY: single-threaded within this test; original values restored
        // before returning so no other test observes the override. This test
        // must never touch the real developer $HOME/.config etc.
        let base = scratch_dir("xdg-defaults");
        let xdg_config = base.join("config");
        let xdg_data = base.join("data");
        let xdg_cache = base.join("cache");

        let saved = [
            ("XDG_CONFIG_HOME", std::env::var("XDG_CONFIG_HOME").ok()),
            ("XDG_DATA_HOME", std::env::var("XDG_DATA_HOME").ok()),
            ("XDG_CACHE_HOME", std::env::var("XDG_CACHE_HOME").ok()),
        ];
        std::env::set_var("XDG_CONFIG_HOME", &xdg_config);
        std::env::set_var("XDG_DATA_HOME", &xdg_data);
        std::env::set_var("XDG_CACHE_HOME", &xdg_cache);

        let roots = ProfileRoots::resolve(None, None);

        for (key, prev) in saved {
            match prev {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }

        assert_eq!(roots.config, xdg_config.join("qbzd"));
        assert_eq!(roots.data, xdg_data.join("qbzd"));
        assert_eq!(roots.cache, xdg_cache.join("qbzd"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
