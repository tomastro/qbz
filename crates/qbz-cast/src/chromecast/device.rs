//! Chromecast device connection and media control

use serde::{Deserialize, Serialize};

use rust_cast::channels::media::{
    IdleReason, Image, Media, Metadata, MusicTrackMediaMetadata, PlayerState, StreamType,
};
use rust_cast::channels::receiver::{CastDeviceApp, Status as ReceiverStatus};
use rust_cast::CastDevice;

use crate::CastError;

const DEFAULT_RECEIVER_ID: &str = "receiver-0";

/// Media metadata for casting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork_url: Option<String>,
    pub duration_secs: Option<u64>,
}

/// Media position info for seekbar updates
#[derive(Debug, Clone, Serialize)]
pub struct CastPositionInfo {
    pub position_secs: f64,
    pub duration_secs: f64,
    pub player_state: String,
    pub idle_reason: Option<String>,
}

/// Device status for frontend
#[derive(Debug, Clone, Serialize)]
pub struct CastStatus {
    pub applications: Vec<CastApplication>,
    pub is_active_input: bool,
    pub is_stand_by: bool,
    pub volume_level: Option<f32>,
    pub volume_muted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CastApplication {
    pub app_id: String,
    pub display_name: String,
    pub status_text: String,
}

struct CastSession {
    #[allow(dead_code)]
    app_id: String,
    session_id: String,
    transport_id: String,
    media_session_id: Option<i32>,
}

/// Wrapper around rust-cast for managing Chromecast devices
pub struct CastDeviceConnection {
    device: CastDevice<'static>,
    session: Option<CastSession>,
}

impl CastDeviceConnection {
    /// Connect to a Chromecast by IP and port
    pub fn connect(ip: &str, port: u16) -> Result<Self, CastError> {
        let device = CastDevice::connect_without_host_verification(ip.to_string(), port)
            .map_err(|e| CastError::Connection(e.to_string()))?;

        device
            .connection
            .connect(DEFAULT_RECEIVER_ID)
            .map_err(|e| CastError::Connection(e.to_string()))?;

        let _ = device.heartbeat.ping();

        Ok(Self {
            device,
            session: None,
        })
    }

    /// Keep the connection alive with a heartbeat ping
    pub fn heartbeat(&self) -> Result<(), CastError> {
        self.device
            .heartbeat
            .ping()
            .map_err(|e| CastError::Connection(e.to_string()))
    }

    /// Disconnect from device
    pub fn disconnect(&mut self) -> Result<(), CastError> {
        // CLOSE the media receiver app, not just our transport to it. A bare
        // transport disconnect leaves the Default Media Receiver running on
        // the device with our last item on its screen (and, if it was
        // playing, still playing) — the "remote queue never clears" symptom.
        // Tearing the app down is what the official senders do on "stop
        // casting".
        if let Some(session) = &self.session {
            let _ = self.device.receiver.stop_app(session.session_id.as_str());
            let _ = self
                .device
                .connection
                .disconnect(session.transport_id.as_str());
        }
        let _ = self.device.connection.disconnect(DEFAULT_RECEIVER_ID);
        self.session = None;
        Ok(())
    }

    /// Get device status
    pub fn get_status(&self) -> Result<CastStatus, CastError> {
        let status = self
            .device
            .receiver
            .get_status()
            .map_err(|e| CastError::Connection(e.to_string()))?;
        Ok(Self::map_status(status))
    }

    /// Launch media receiver app
    pub fn launch_media_app(&mut self) -> Result<String, CastError> {
        let app = self
            .device
            .receiver
            .launch_app(&CastDeviceApp::DefaultMediaReceiver)
            .map_err(|e| CastError::Connection(e.to_string()))?;

        self.device
            .connection
            .connect(app.transport_id.as_str())
            .map_err(|e| CastError::Connection(e.to_string()))?;

        self.session = Some(CastSession {
            app_id: app.app_id.clone(),
            session_id: app.session_id.clone(),
            transport_id: app.transport_id.clone(),
            media_session_id: None,
        });

        Ok(app.session_id)
    }

    /// Load media URL for playback
    pub fn load_media(
        &mut self,
        url: &str,
        content_type: &str,
        metadata: MediaMetadata,
    ) -> Result<(), CastError> {
        self.ensure_session()?;

        let session = self.session.as_ref().ok_or(CastError::NotConnected)?;
        let media = Media {
            content_id: url.to_string(),
            stream_type: StreamType::Buffered,
            content_type: content_type.to_string(),
            duration: metadata.duration_secs.map(|d| d as f32),
            metadata: Some(Self::map_metadata(metadata)),
        };

        let status = self
            .device
            .media
            .load(
                session.transport_id.as_str(),
                session.session_id.as_str(),
                &media,
            )
            .map_err(|e| CastError::Media(e.to_string()))?;

        if let Some(entry) = status.entries.first() {
            if let Some(session) = self.session.as_mut() {
                session.media_session_id = Some(entry.media_session_id);
            }
        }

        Ok(())
    }

    /// Play current media session
    pub fn play(&mut self) -> Result<(), CastError> {
        let (destination, media_session_id) = self.ensure_media_session()?;
        self.device
            .media
            .play(destination.as_str(), media_session_id)
            .map_err(|e| CastError::Media(e.to_string()))?;
        Ok(())
    }

    /// Pause current media session
    pub fn pause(&mut self) -> Result<(), CastError> {
        let (destination, media_session_id) = self.ensure_media_session()?;
        self.device
            .media
            .pause(destination.as_str(), media_session_id)
            .map_err(|e| CastError::Media(e.to_string()))?;
        Ok(())
    }

    /// Stop current media session
    pub fn stop(&mut self) -> Result<(), CastError> {
        let (destination, media_session_id) = self.ensure_media_session()?;
        self.device
            .media
            .stop(destination.as_str(), media_session_id)
            .map_err(|e| CastError::Media(e.to_string()))?;
        Ok(())
    }

    /// Volume control (0.0 - 1.0)
    pub fn set_volume(&mut self, volume: f32) -> Result<(), CastError> {
        let clamped = volume.clamp(0.0, 1.0);
        self.device
            .receiver
            .set_volume(clamped)
            .map_err(|e| CastError::Connection(e.to_string()))?;
        Ok(())
    }

    /// Seek to position
    pub fn seek(&mut self, position_secs: f64) -> Result<(), CastError> {
        let (destination, media_session_id) = self.ensure_media_session()?;
        self.device
            .media
            .seek(
                destination.as_str(),
                media_session_id,
                Some(position_secs as f32),
                None,
            )
            .map_err(|e| CastError::Media(e.to_string()))?;
        Ok(())
    }

    /// Get current media position for seekbar updates
    pub fn get_media_position(&mut self) -> Result<CastPositionInfo, CastError> {
        // No media app launched yet: the receiver is connected and healthy,
        // there is simply nothing to read. Callers must not read this as
        // "device gone" (the Qt poll did, and dropped every idle connect).
        let session = self.session.as_ref().ok_or(CastError::NoMediaSession)?;
        let destination = &session.transport_id;

        let status = self
            .device
            .media
            .get_status(destination.as_str(), None)
            .map_err(|e| CastError::Media(e.to_string()))?;

        // Get the first media entry (current playing item)
        if let Some(entry) = status.entries.first() {
            let player_state = match entry.player_state {
                PlayerState::Idle => "IDLE",
                PlayerState::Playing => "PLAYING",
                PlayerState::Paused => "PAUSED",
                PlayerState::Buffering => "BUFFERING",
            };

            let idle_reason = entry.idle_reason.as_ref().map(|r| match r {
                IdleReason::Cancelled => "CANCELLED".to_string(),
                IdleReason::Interrupted => "INTERRUPTED".to_string(),
                IdleReason::Finished => "FINISHED".to_string(),
                IdleReason::Error => "ERROR".to_string(),
            });

            let duration = entry.media.as_ref().and_then(|m| m.duration).unwrap_or(0.0) as f64;

            let position = entry.current_time.unwrap_or(0.0) as f64;

            Ok(CastPositionInfo {
                position_secs: position,
                duration_secs: duration,
                player_state: player_state.to_string(),
                idle_reason,
            })
        } else {
            // No media playing
            Ok(CastPositionInfo {
                position_secs: 0.0,
                duration_secs: 0.0,
                player_state: "IDLE".to_string(),
                idle_reason: None,
            })
        }
    }

    fn ensure_session(&mut self) -> Result<(), CastError> {
        if self.session.is_some() {
            return Ok(());
        }
        self.launch_media_app().map(|_| ())
    }

    /// Transport commands NEVER launch the receiver app: `play`/`pause`/
    /// `stop` on a receiver with nothing loaded used to launch the Default
    /// Media Receiver just to ask it for a media session it could not have,
    /// which then sat there IDLE — and an IDLE report is what the poll reads
    /// as "track ended". Only `load_media` launches the app.
    fn ensure_media_session(&mut self) -> Result<(String, i32), CastError> {
        if self.session.is_none() {
            return Err(CastError::NoMediaSession);
        }

        let destination = self
            .session
            .as_ref()
            .ok_or(CastError::NotConnected)?
            .transport_id
            .clone();

        if let Some(id) = self
            .session
            .as_ref()
            .and_then(|session| session.media_session_id)
        {
            return Ok((destination, id));
        }

        let status = self
            .device
            .media
            .get_status(destination.as_str(), None)
            .map_err(|e| CastError::Media(e.to_string()))?;

        let entry = status
            .entries
            .first()
            .ok_or_else(|| CastError::Media("No media session available".to_string()))?;

        if let Some(session) = self.session.as_mut() {
            session.media_session_id = Some(entry.media_session_id);
        }

        Ok((destination, entry.media_session_id))
    }

    fn map_status(status: ReceiverStatus) -> CastStatus {
        let applications = status
            .applications
            .iter()
            .map(|app| CastApplication {
                app_id: app.app_id.clone(),
                display_name: app.display_name.clone(),
                status_text: app.status_text.clone(),
            })
            .collect();

        CastStatus {
            applications,
            is_active_input: status.is_active_input,
            is_stand_by: status.is_stand_by,
            volume_level: status.volume.level,
            volume_muted: status.volume.muted,
        }
    }

    fn map_metadata(metadata: MediaMetadata) -> Metadata {
        let mut images = Vec::new();
        if let Some(url) = metadata.artwork_url.clone() {
            images.push(Image::new(url));
        }

        Metadata::MusicTrack(MusicTrackMediaMetadata {
            album_name: Some(metadata.album),
            title: Some(metadata.title),
            album_artist: None,
            artist: Some(metadata.artist),
            composer: None,
            track_number: None,
            disc_number: None,
            images,
            release_date: None,
        })
    }
}
