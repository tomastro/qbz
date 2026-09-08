//! Pure build-policy checks, also exercised by the Qt unit-test gate.

use std::path::Path;

pub fn use_prebuilt_shaders(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(value) => Err(format!(
            "QBZ_PREBUILT_SHADERS must be 0 or 1, got {value:?}"
        )),
    }
}

/// Prebuilt mode must not turn a missing pack into a successful build.
/// Shader compilation/variant coverage remains the separate shader bake gate.
pub fn require_prebuilt_shader(source: &Path) -> Result<(), String> {
    let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("");
    let pack = source.with_extension(format!("{extension}.qsb"));
    match std::fs::metadata(&pack) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => Ok(()),
        _ => Err(format!(
            "missing or empty prebuilt shader {}; re-bake with qsb before building",
            pack.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "qbz-shader-policy-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn prebuilt_mode_requires_an_explicit_valid_choice() {
        assert!(!use_prebuilt_shaders(None).unwrap());
        assert!(!use_prebuilt_shaders(Some("0")).unwrap());
        assert!(use_prebuilt_shaders(Some("1")).unwrap());
        assert!(use_prebuilt_shaders(Some("true")).is_err());
        assert!(use_prebuilt_shaders(Some("")).is_err());
    }

    #[test]
    fn prebuilt_mode_rejects_missing_empty_and_directory_packs() {
        let fixture = Fixture::new();
        let source = fixture.0.join("effect.frag");
        let pack = fixture.0.join("effect.frag.qsb");
        assert!(require_prebuilt_shader(&source).is_err());
        std::fs::write(&pack, []).unwrap();
        assert!(require_prebuilt_shader(&source).is_err());
        std::fs::remove_file(&pack).unwrap();
        std::fs::create_dir(&pack).unwrap();
        assert!(require_prebuilt_shader(&source).is_err());
    }

    #[test]
    fn prebuilt_mode_preserves_both_shader_stages_byte_for_byte() {
        let fixture = Fixture::new();
        for stage in ["frag", "vert"] {
            let source = fixture.0.join(format!("effect.{stage}"));
            let pack = fixture.0.join(format!("effect.{stage}.qsb"));
            let bytes = b"prebuilt fixture";
            std::fs::write(&pack, bytes).unwrap();
            require_prebuilt_shader(&source).unwrap();
            assert_eq!(std::fs::read(&pack).unwrap(), bytes);
        }
    }
}
