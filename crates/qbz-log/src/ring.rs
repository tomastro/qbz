//! Bounded in-memory ring buffer of the most recent [`LogLine`]s.
//!
//! FIFO with a hard cap of [`RING_CAP`]: at capacity the oldest line is dropped.
//! The critical section is kept tiny (push/pop only — never held across I/O) so the
//! audio/tokio threads that log never block meaningfully. Lock poisoning is tolerated
//! (`into_inner`) because a single corrupt line must never wedge logging globally.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use crate::line::LogLine;

/// Maximum number of lines retained in the ring.
pub const RING_CAP: usize = 5000;

static RING: OnceLock<Arc<Mutex<VecDeque<LogLine>>>> = OnceLock::new();

fn ring() -> &'static Arc<Mutex<VecDeque<LogLine>>> {
    RING.get_or_init(|| Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAP))))
}

/// Append a line, dropping the oldest if the ring is full. O(1).
pub fn push(line: LogLine) {
    let r = ring();
    let mut guard = r.lock().unwrap_or_else(|p| p.into_inner());
    if guard.len() == RING_CAP {
        guard.pop_front();
    }
    guard.push_back(line);
}

/// Clone the current contents (oldest first) for the viewer/bundle. Clones under lock.
pub fn snapshot() -> Vec<LogLine> {
    let r = ring();
    let guard = r.lock().unwrap_or_else(|p| p.into_inner());
    guard.iter().cloned().collect()
}

/// Empty the ring (the "Clear" action). Does not touch the on-disk file.
pub fn clear() {
    let r = ring();
    let mut guard = r.lock().unwrap_or_else(|p| p.into_inner());
    guard.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;

    fn mk(i: i64) -> LogLine {
        LogLine {
            ts: i,
            level: Level::Info,
            target: "t".into(),
            message: format!("line {i}"),
        }
    }

    #[test]
    fn ring_caps_and_is_fifo() {
        clear();
        let total = RING_CAP as i64 + 10;
        for i in 0..total {
            push(mk(i));
        }
        let snap = snapshot();
        // Capacity is enforced.
        assert_eq!(snap.len(), RING_CAP);
        // Oldest 10 (ts 0..10) were dropped, so the first retained line is ts 10.
        assert_eq!(snap.first().unwrap().ts, 10);
        // Newest line is last.
        assert_eq!(snap.last().unwrap().ts, total - 1);
    }
}
