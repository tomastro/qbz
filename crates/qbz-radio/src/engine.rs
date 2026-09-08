use super::db::{RadioDb, RadioTrackRef};

pub struct RadioEngine {
    db: RadioDb,
}

impl RadioEngine {
    pub fn new(db: RadioDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &RadioDb {
        &self.db
    }

    fn splitmix64(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn base_weight(distance: u8) -> u32 {
        match distance {
            0 => 6,
            1 => 3,
            2 => 1,
            _ => 0,
        }
    }

    fn effective_weight(track: &RadioTrackRef, reseed_tick: bool) -> u32 {
        let mut w = Self::base_weight(track.distance);
        if w == 0 {
            return 0;
        }

        if reseed_tick {
            if track.distance == 0 {
                w = w.saturating_add(6);
            }
            if track.source == "curated_playlist" {
                w = w.saturating_add(2);
            }
        }
        w
    }

    pub fn next_track(&self, session_id: &str) -> Result<RadioTrackRef, String> {
        let session = self.db.load_session(session_id)?;

        let mut spacing = session.artist_spacing;
        let candidates = loop {
            let recent_artists = self.db.get_recent_artist_ids(session_id, spacing)?;
            let cands = self.db.get_unused_candidates(session_id, &recent_artists)?;
            if !cands.is_empty() || spacing == 0 {
                break cands;
            }
            spacing = spacing.saturating_sub(1);
        };

        if candidates.is_empty() {
            return Err("Radio session exhausted: no eligible tracks left".to_string());
        }

        let next_index = session.selection_count.saturating_add(1);
        let reseed_tick = session.reseed_every > 0 && (next_index % session.reseed_every == 0);

        let mut total_weight: u64 = 0;
        let mut weights: Vec<u32> = Vec::with_capacity(candidates.len());
        for t in &candidates {
            let w = Self::effective_weight(t, reseed_tick);
            weights.push(w);
            total_weight = total_weight.saturating_add(w as u64);
        }

        if total_weight == 0 {
            let chosen = candidates
                .first()
                .cloned()
                .ok_or_else(|| "Radio selection failed: no candidates".to_string())?;
            self.db
                .mark_played(session_id, session.selection_count, &chosen)?;
            return Ok(chosen);
        }

        let r = Self::splitmix64(session.rng_seed ^ (next_index as u64));
        let mut pick = (r % total_weight) as u64;

        let mut chosen_idx = 0usize;
        for (idx, w) in weights.iter().enumerate() {
            let w = *w as u64;
            if pick < w {
                chosen_idx = idx;
                break;
            }
            pick = pick.saturating_sub(w);
        }

        let chosen = candidates
            .get(chosen_idx)
            .cloned()
            .ok_or_else(|| "Radio selection failed: chosen candidate missing".to_string())?;

        self.db
            .mark_played(session_id, session.selection_count, &chosen)?;

        Ok(chosen)
    }
}
