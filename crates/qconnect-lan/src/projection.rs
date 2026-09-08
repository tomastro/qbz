use std::sync::{Arc, RwLock};

use crate::{ConnectInfo, DisplayInfo};

#[derive(Debug, Clone)]
struct ProjectionSnapshot {
    display: DisplayInfo,
    connect: ConnectInfo,
}

/// Short, synchronous projection read by the HTTP thread. Runtime adapters
/// update it from their own event loops; handlers never block on async I/O.
#[derive(Debug, Clone)]
pub struct LanProjection {
    inner: Arc<RwLock<ProjectionSnapshot>>,
}

impl LanProjection {
    pub fn new(display: DisplayInfo, connect: ConnectInfo) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ProjectionSnapshot { display, connect })),
        }
    }

    pub fn display_info(&self) -> DisplayInfo {
        self.inner
            .read()
            .expect("LAN projection lock poisoned")
            .display
            .clone()
    }

    pub fn connect_info(&self) -> ConnectInfo {
        self.inner
            .read()
            .expect("LAN projection lock poisoned")
            .connect
            .clone()
    }

    pub fn update_display(&self, display: DisplayInfo) {
        self.inner
            .write()
            .expect("LAN projection lock poisoned")
            .display = display;
    }

    pub fn update_connect(&self, connect: ConnectInfo) {
        self.inner
            .write()
            .expect("LAN projection lock poisoned")
            .connect = connect;
    }

    pub fn set_current_session_id(&self, session_id: Option<String>) {
        self.inner
            .write()
            .expect("LAN projection lock poisoned")
            .connect
            .current_session_id = session_id;
    }
}
