//! Configurable weights for different relationship types
//!
//! These weights determine how strongly different types of relationships
//! contribute to artist similarity vectors.

use serde::{Deserialize, Serialize};

/// Weights for different relationship types when building artist vectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipWeights {
    // === MusicBrainz artist-to-artist relationships ===
    /// Weight for band membership (artist was/is member of group)
    /// Strongest connection - same creative unit
    pub member_of_band: f32,

    /// Weight for collaboration (artists worked together)
    pub collaboration: f32,

    /// Weight for "is person" in a group (reverse of member_of_band)
    pub has_member: f32,

    /// Weight for founder relationship
    pub founder: f32,

    // === MusicBrainz recording/release relationships ===
    /// Weight for performer credit on a recording
    pub performer: f32,

    /// Weight for composer credit
    pub composer: f32,

    /// Weight for producer credit
    pub producer: f32,

    /// Weight for conductor credit
    pub conductor: f32,

    /// Weight for engineer/mixer credit
    pub engineer: f32,

    // === Qobuz relationships ===
    /// Weight for Qobuz similar artists
    pub qobuz_similar: f32,

    // === Tag-based relationships ===
    /// Weight for shared MusicBrainz tags (genres)
    pub shared_tag: f32,

    // === Behavioral relationships ===
    /// Weight for user listening affinity (artists played together)
    pub user_affinity: f32,
}

impl Default for RelationshipWeights {
    fn default() -> Self {
        Self {
            // Band relationships - strongest
            member_of_band: 1.0,
            has_member: 0.9,
            founder: 0.85,
            collaboration: 0.8,

            // Credit relationships - medium
            performer: 0.6,
            composer: 0.55,
            producer: 0.5,
            conductor: 0.5,
            engineer: 0.3,

            // Qobuz similarity - good signal
            qobuz_similar: 0.7,

            // Tags - weak but useful
            shared_tag: 0.3,

            // User behavior - medium
            user_affinity: 0.5,
        }
    }
}

impl RelationshipWeights {
    /// Create weights optimized for discovering band-related artists
    pub fn band_focused() -> Self {
        Self {
            member_of_band: 1.0,
            has_member: 1.0,
            founder: 0.9,
            collaboration: 0.7,
            performer: 0.4,
            composer: 0.3,
            producer: 0.2,
            conductor: 0.3,
            engineer: 0.1,
            qobuz_similar: 0.5,
            shared_tag: 0.2,
            user_affinity: 0.3,
        }
    }

    /// Create weights optimized for sound-alike discovery
    pub fn similarity_focused() -> Self {
        Self {
            member_of_band: 0.6,
            has_member: 0.5,
            founder: 0.5,
            collaboration: 0.7,
            performer: 0.5,
            composer: 0.4,
            producer: 0.3,
            conductor: 0.3,
            engineer: 0.2,
            qobuz_similar: 1.0, // Prioritize Qobuz similarity
            shared_tag: 0.5,
            user_affinity: 0.6,
        }
    }

    /// Get weight for a MusicBrainz relationship type
    pub fn weight_for_mb_relation(&self, relation_type: &str) -> f32 {
        match relation_type.to_lowercase().as_str() {
            // Band relationships
            "member of band" | "member_of_band" => self.member_of_band,
            "has member" | "has_member" => self.has_member,
            "founder" | "founded" => self.founder,

            // Collaboration
            "collaboration" | "collaborated" | "collaborator" => self.collaboration,

            // Performance credits
            "performer"
            | "vocal"
            | "instrument"
            | "performing orchestra"
            | "orchestra"
            | "chorus master" => self.performer,

            // Composition
            "composer" | "writer" | "lyricist" | "librettist" | "arranger" => self.composer,

            // Production
            "producer" | "executive producer" | "co-producer" => self.producer,
            "conductor" => self.conductor,
            "engineer" | "mix" | "mixer" | "mastering" | "recording" => self.engineer,

            // Unknown - use small default weight
            _ => 0.2,
        }
    }

    /// Get weight for a source type string
    pub fn weight_for_source(&self, source: &str) -> f32 {
        match source {
            "qobuz_similar" => self.qobuz_similar,
            "shared_tag" => self.shared_tag,
            "user_affinity" => self.user_affinity,
            s if s.starts_with("mb:") => {
                let rel_type = &s[3..];
                self.weight_for_mb_relation(rel_type)
            }
            _ => 0.2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_weights() {
        let weights = RelationshipWeights::default();

        assert_eq!(weights.member_of_band, 1.0);
        assert!(weights.qobuz_similar > weights.shared_tag);
        assert!(weights.collaboration > weights.engineer);
    }

    #[test]
    fn test_mb_relation_weight() {
        let weights = RelationshipWeights::default();

        assert_eq!(weights.weight_for_mb_relation("member of band"), 1.0);
        assert_eq!(weights.weight_for_mb_relation("collaboration"), 0.8);
        assert_eq!(weights.weight_for_mb_relation("producer"), 0.5);

        // Unknown type gets default
        assert_eq!(weights.weight_for_mb_relation("unknown_type"), 0.2);
    }

    #[test]
    fn test_source_weight() {
        let weights = RelationshipWeights::default();

        assert_eq!(weights.weight_for_source("qobuz_similar"), 0.7);
        assert_eq!(weights.weight_for_source("mb:member of band"), 1.0);
        assert_eq!(weights.weight_for_source("mb:collaboration"), 0.8);
    }
}
