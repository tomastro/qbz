//! Linux MPRIS backend via `mpris-server`.
//!
//! The whole reason this exists instead of souvlaki: `mpris-server`'s
//! `RootInterface::desktop_entry()` lets us publish the
//! `org.mpris.MediaPlayer2.DesktopEntry` property as `"com.blitzfc.qbz"`,
//! which is the ONLY mechanism GNOME Shell uses to resolve the application
//! icon for its media widget (DesktopEntry → `<name>.desktop` → `Icon=`).
//! souvlaki never sets it, so GNOME shows no icon. (KDE is lenient and works
//! either way.) `mpris:artUrl` is album art — separate and unaffected.
//!
//! The server runs on a dedicated thread with its own current-thread tokio
//! runtime (the workspace forces zbus 4's `tokio` feature via qbz-audio, so a
//! tokio context must be present); state updates arrive over an async channel.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use mpris_server::zbus::{self, fdo};
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus as MprisStatus, PlayerInterface, Property,
    RootInterface, Server, Signal, Time, TrackId, Volume,
};

use crate::inhibit::SleepInhibitor;
use crate::types::{MediaEvent, MediaIntegration, PlaybackStatus, TrackMeta};

const BUS_SUFFIX: &str = "com.blitzfc.qbz";
const DESKTOP_ENTRY: &str = "com.blitzfc.qbz";
const IDENTITY: &str = "QBZ";

/// Monotonic counter so each track gets a distinct `mpris:trackid` object path
/// (helps clients detect track changes).
static TRACK_SEQ: AtomicU64 = AtomicU64::new(1);

type EventCb = Arc<dyn Fn(MediaEvent) + Send + Sync>;

/// Shared, mutable now-playing state. Read by the MPRIS getter methods (on the
/// zbus task) and written by the update loop (on the same runtime). Never held
/// across an `.await`.
struct State {
    metadata: Metadata,
    status: MprisStatus,
    volume: Volume,
    /// The last position we were TOLD, and when we were told it.
    ///
    /// MPRIS `Position` is read on demand, so what matters is the answer at
    /// READ time, not at push time. Storing the instant alongside lets the
    /// getter extrapolate — `position + (now - stamped)` while Playing — which
    /// is exact to the microsecond no matter how often the app pushes. Without
    /// it the answer is only as fresh as the last push, and the app would have
    /// to choose between a stale clock and a faster poll it does not want.
    position: Time,
    stamped: std::time::Instant,
}

/// Update commands sent from the app to the server thread.
enum Update {
    Metadata(Metadata),
    Playback {
        status: MprisStatus,
        position: Option<Time>,
    },
    Volume(Volume),
    /// Position ONLY — stores, emits nothing. MPRIS excludes `Position` from
    /// `PropertiesChanged` by design, so there is no signal to send; the point
    /// is purely that the next on-demand read is not stale.
    Position(Time),
    /// A DISCONTINUOUS jump. Stores like `Position` and additionally emits the
    /// `Seeked` signal, which is the ONE thing that tells an already-open
    /// client to stop extrapolating and re-read. Without it a widget keeps
    /// counting from wherever it was until it happens to poll again.
    Seeked(Time),
}

/// The cloneable handle returned to the app. Pushing state is a non-blocking
/// channel send from any thread/context.
pub struct LinuxHandle {
    tx: async_channel::Sender<Update>,
}

impl MediaIntegration for LinuxHandle {
    fn set_metadata(&self, meta: &TrackMeta) {
        let _ = self.tx.try_send(Update::Metadata(build_metadata(meta)));
    }

    fn set_playback(&self, status: PlaybackStatus, position: Option<std::time::Duration>) {
        let _ = self.tx.try_send(Update::Playback {
            status: map_status(status),
            position: position.map(|d| Time::from_micros(d.as_micros() as i64)),
        });
    }

    fn set_volume(&self, vol: f64) {
        let _ = self.tx.try_send(Update::Volume(vol.clamp(0.0, 1.0)));
    }

    fn set_position(&self, position: std::time::Duration) {
        let _ = self
            .tx
            .try_send(Update::Position(Time::from_micros(position.as_micros() as i64)));
    }

    fn seeked(&self, position: std::time::Duration) {
        let _ = self
            .tx
            .try_send(Update::Seeked(Time::from_micros(position.as_micros() as i64)));
    }
}

fn map_status(s: PlaybackStatus) -> MprisStatus {
    match s {
        PlaybackStatus::Playing => MprisStatus::Playing,
        PlaybackStatus::Paused => MprisStatus::Paused,
        PlaybackStatus::Stopped => MprisStatus::Stopped,
    }
}

fn build_metadata(meta: &TrackMeta) -> Metadata {
    let seq = TRACK_SEQ.fetch_add(1, Ordering::Relaxed);
    let trackid = TrackId::try_from(format!("/com/blitzfc/qbz/track/{seq}"))
        .unwrap_or(TrackId::NO_TRACK);

    let mut b = Metadata::builder().trackid(trackid).title(meta.title.clone());
    if !meta.artist.is_empty() {
        b = b.artist([meta.artist.clone()]);
    }
    if !meta.album.is_empty() {
        b = b.album(meta.album.clone());
    }
    if let Some(d) = meta.duration {
        b = b.length(Time::from_micros(d.as_micros() as i64));
    }
    if let Some(url) = &meta.art_url {
        b = b.art_url(url.clone());
    }
    if let Some(url) = &meta.url {
        b = b.url(url.clone());
    }
    b.build()
}

/// The MPRIS interface implementation. Getters read shared `State`; the action
/// methods forward to the app via `on_event`.
struct QbzMpris {
    on_event: EventCb,
    state: Arc<Mutex<State>>,
}

impl QbzMpris {
    fn emit(&self, ev: MediaEvent) {
        (self.on_event)(ev);
    }
}

impl RootInterface for QbzMpris {
    async fn raise(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Raise);
        Ok(())
    }
    async fn quit(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Quit);
        Ok(())
    }
    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
        Ok(())
    }
    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn identity(&self) -> fdo::Result<String> {
        Ok(IDENTITY.to_string())
    }
    /// The GNOME app-icon fix: GNOME resolves the icon from this property.
    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok(DESKTOP_ENTRY.to_string())
    }
    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
}

impl PlayerInterface for QbzMpris {
    async fn next(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Next);
        Ok(())
    }
    async fn previous(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Previous);
        Ok(())
    }
    async fn pause(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Pause);
        Ok(())
    }
    async fn play_pause(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Toggle);
        Ok(())
    }
    async fn stop(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Stop);
        Ok(())
    }
    async fn play(&self) -> fdo::Result<()> {
        self.emit(MediaEvent::Play);
        Ok(())
    }
    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        self.emit(MediaEvent::SeekBy(offset.as_micros()));
        Ok(())
    }
    async fn set_position(&self, _track_id: TrackId, position: Time) -> fdo::Result<()> {
        self.emit(MediaEvent::SetPosition(position.as_micros()));
        Ok(())
    }
    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Ok(())
    }
    async fn playback_status(&self) -> fdo::Result<MprisStatus> {
        Ok(self.state.lock().unwrap().status)
    }
    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(LoopStatus::None)
    }
    async fn set_loop_status(&self, _loop_status: LoopStatus) -> zbus::Result<()> {
        Ok(())
    }
    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
        Ok(())
    }
    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn set_shuffle(&self, _shuffle: bool) -> zbus::Result<()> {
        Ok(())
    }
    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.state.lock().unwrap().metadata.clone())
    }
    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.state.lock().unwrap().volume)
    }
    async fn set_volume(&self, volume: Volume) -> zbus::Result<()> {
        self.emit(MediaEvent::SetVolume(volume.clamp(0.0, 1.0)));
        Ok(())
    }
    async fn position(&self) -> fdo::Result<Time> {
        let st = self.state.lock().unwrap();
        // Extrapolate while playing; a paused or stopped player has not moved
        // since it was stamped.
        if st.status == MprisStatus::Playing {
            let elapsed = st.stamped.elapsed().as_micros() as i64;
            Ok(Time::from_micros(st.position.as_micros() + elapsed))
        } else {
            Ok(st.position)
        }
    }
    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

async fn apply(server: &Server<QbzMpris>, state: &Arc<Mutex<State>>, update: Update) {
    match update {
        Update::Metadata(m) => {
            state.lock().unwrap().metadata = m.clone();
            let _ = server.properties_changed([Property::Metadata(m)]).await;
        }
        Update::Playback { status, position } => {
            {
                let mut st = state.lock().unwrap();
                st.status = status;
                if let Some(p) = position {
                    st.position = p;
                    st.stamped = std::time::Instant::now();
                } else {
                    // A status change with no position still re-anchors: the
                    // extrapolation base must not keep running across a pause.
                    st.position = Time::from_micros(
                        st.position.as_micros()
                            + if st.status == MprisStatus::Playing {
                                st.stamped.elapsed().as_micros() as i64
                            } else {
                                0
                            },
                    );
                    st.stamped = std::time::Instant::now();
                }
            }
            let _ = server
                .properties_changed([Property::PlaybackStatus(status)])
                .await;
        }
        Update::Volume(v) => {
            state.lock().unwrap().volume = v;
            let _ = server.properties_changed([Property::Volume(v)]).await;
        }
        Update::Position(p) => {
            let mut st = state.lock().unwrap();
            st.position = p;
            st.stamped = std::time::Instant::now();
        }
        Update::Seeked(p) => {
            {
                let mut st = state.lock().unwrap();
                st.position = p;
                st.stamped = std::time::Instant::now();
            }
            let _ = server.emit(Signal::Seeked { position: p }).await;
        }
    }
}

/// Spawn the MPRIS server on a dedicated thread. Returns `None` if the thread
/// or runtime can't start (the bus registration happens async on the thread;
/// failures there are logged, not surfaced — the app keeps running).
pub fn spawn(on_event: EventCb) -> Option<LinuxHandle> {
    let (tx, rx) = async_channel::unbounded::<Update>();

    let spawned = std::thread::Builder::new()
        .name("qbz-mpris".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("[mpris] runtime build failed: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let state = Arc::new(Mutex::new(State {
                    metadata: Metadata::new(),
                    status: MprisStatus::Stopped,
                    volume: 1.0,
                    position: Time::ZERO,
                    stamped: std::time::Instant::now(),
                }));
                let imp = QbzMpris {
                    on_event,
                    state: state.clone(),
                };
                let server = match Server::new(BUS_SUFFIX, imp).await {
                    Ok(s) => s,
                    Err(e) => {
                        log::error!("[mpris] failed to register org.mpris.MediaPlayer2.{BUS_SUFFIX}: {e}");
                        return;
                    }
                };
                log::info!(
                    "[mpris] registered org.mpris.MediaPlayer2.{BUS_SUFFIX} (DesktopEntry={DESKTOP_ENTRY})"
                );
                // Sleep/idle inhibitor (#522): held while Playing, dropped on
                // Paused/Stopped. Piggybacks on the same playback updates the
                // MPRIS server consumes, so it can never disagree with what
                // the desktop widget shows.
                let mut inhibitor = SleepInhibitor::new();
                while let Ok(update) = rx.recv().await {
                    if let Update::Playback { status, .. } = &update {
                        inhibitor
                            .set_playing(matches!(status, MprisStatus::Playing))
                            .await;
                    }
                    apply(&server, &state, update).await;
                }
                log::debug!("[mpris] update channel closed, server shutting down");
            });
        });

    match spawned {
        Ok(_) => Some(LinuxHandle { tx }),
        Err(e) => {
            log::error!("[mpris] failed to spawn server thread: {e}");
            None
        }
    }
}
