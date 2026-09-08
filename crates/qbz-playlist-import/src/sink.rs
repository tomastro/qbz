//! Frontend-agnostic progress reporting.
//!
//! Replaces the Tauri `AppHandle::emit("import:phase" | "import:progress")`
//! sites. Emission is fire-and-forget and infallible — the Tauri emits were
//! all `let _ =`.

use serde::Serialize;

use crate::models::ImportProgress;

/// Import phase, mirroring the Tauri `import:phase` payloads
/// (`{"phase": "matching" | "creating" | "adding"}`).
///
/// `Creating` and `Adding` re-fire once per created playlist part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportPhase {
    Matching,
    Creating,
    /// NOTE (owner decision, deliberate deviation from the Tauri UI):
    /// frontends should render the status line during this phase as
    /// "Adding tracks: {current} / {total}" — the Tauri modal reused the
    /// "Matching tracks…" string here; do not carry that quirk.
    Adding,
}

/// One event from the import pipeline, mirroring the two Tauri events.
#[derive(Debug, Clone, Serialize)]
pub enum ImportEvent {
    Phase(ImportPhase),
    /// Existing wire model, unchanged shape. While matching: one per track
    /// (high frequency), `current_track` = "Artist - Title". While adding:
    /// one per 50-track chunk (chunk counts, not tracks), `current_track` =
    /// "Part i/n" iff multi-part.
    Progress(ImportProgress),
}

/// Receives progress events from [`crate::import_public_playlist`].
pub trait ImportProgressSink: Send + Sync {
    fn emit(&self, event: ImportEvent);
}

impl<F: Fn(ImportEvent) + Send + Sync> ImportProgressSink for F {
    fn emit(&self, event: ImportEvent) {
        self(event)
    }
}
