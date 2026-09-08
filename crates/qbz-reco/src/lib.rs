//! Frontend-agnostic artist-vector recommendation engine (ADR-006).
//!
//! Cleanroom port of Tauri's `src-tauri/src/artist_vectors/` into a shared
//! crate so the Slint frontend (and any headless caller) can produce a
//! playlist's "Suggested Songs" without a `tauri::State` dependency.
//!
//! The vector modules originated in the Tauri implementation. The dead
//! cosine-similarity / `find_nearest` path is dropped (production ranks by
//! summed relationship weight), and artist resolution uses seed-relative
//! identity evidence rather than a global genre blocklist.

mod artist_guardrail;
mod builder;
mod sparse_vector;
mod store;
mod suggestions;
mod weights;

pub use artist_guardrail::{
    resolve_candidate, resolve_seed_context, validate_candidate, ArtistFacts, ArtistLookup,
    ArtistLookupFuture, SeedContext,
};
pub use builder::{ArtistVectorBuilder, BuildResult};
pub use sparse_vector::SparseVector;
pub use store::{ArtistVectorStore, SimilarArtist, VECTOR_TTL_SECS};
pub use suggestions::{
    extract_artist_mbids, SuggestedTrack, SuggestionConfig, SuggestionResult, SuggestionsEngine,
};
pub use weights::RelationshipWeights;
