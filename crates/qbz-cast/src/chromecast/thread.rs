//! Chromecast connection handler running in a dedicated thread.
//!
//! Since rust_cast uses Rc (not Arc), it cannot be shared across threads.
//! This module provides a thread-safe wrapper using channels.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::chromecast::device::CastDeviceConnection;
use crate::chromecast::{CastPositionInfo, CastStatus, MediaMetadata};
use crate::CastError;

/// Commands sent to the Chromecast thread
pub enum CastCommand {
    Connect {
        ip: String,
        port: u16,
        reply: Sender<Result<(), CastError>>,
    },
    Disconnect {
        reply: Sender<Result<(), CastError>>,
    },
    GetStatus {
        reply: Sender<Result<CastStatus, CastError>>,
    },
    GetMediaPosition {
        reply: Sender<Result<CastPositionInfo, CastError>>,
    },
    LoadMedia {
        url: String,
        content_type: String,
        metadata: MediaMetadata,
        reply: Sender<Result<(), CastError>>,
    },
    Play {
        reply: Sender<Result<(), CastError>>,
    },
    Pause {
        reply: Sender<Result<(), CastError>>,
    },
    Stop {
        reply: Sender<Result<(), CastError>>,
    },
    SetVolume {
        volume: f32,
        reply: Sender<Result<(), CastError>>,
    },
    Seek {
        position_secs: f64,
        reply: Sender<Result<(), CastError>>,
    },
    Shutdown,
}

const TEARDOWN_REPLY_TIMEOUT: Duration = Duration::from_millis(750);

struct ChromecastThreadLifetime {
    sender: Sender<CastCommand>,
    _thread: JoinHandle<()>,
    valid: AtomicBool,
    command_gate: Mutex<()>,
}

impl Drop for ChromecastThreadLifetime {
    fn drop(&mut self) {
        self.valid.store(false, Ordering::Release);
        let _ = self.sender.send(CastCommand::Shutdown);
    }
}

/// Thread-safe handle to communicate with the Chromecast thread. Clones share
/// one validity fence; a teardown timeout invalidates every clone before the
/// blocked worker is detached from its async caller.
#[derive(Clone)]
pub struct ChromecastHandle {
    lifetime: Arc<ChromecastThreadLifetime>,
}

impl ChromecastHandle {
    /// Start the Chromecast handler thread
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            chromecast_thread_main(receiver);
        });

        Self {
            lifetime: Arc::new(ChromecastThreadLifetime {
                sender,
                _thread: thread,
                valid: AtomicBool::new(true),
                command_gate: Mutex::new(()),
            }),
        }
    }

    fn send_command(&self, command: CastCommand) -> Result<(), CastError> {
        let _gate = self
            .lifetime
            .command_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.lifetime.valid.load(Ordering::Acquire) {
            return Err(CastError::NotConnected);
        }
        self.lifetime
            .sender
            .send(command)
            .map_err(|_| CastError::Connection("Thread communication error".to_string()))
    }

    /// Fence all clones and request worker shutdown. A command already inside
    /// rust-cast may finish later, but no handle can enqueue another command.
    pub fn invalidate(&self) {
        let _gate = self
            .lifetime
            .command_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.lifetime.valid.swap(false, Ordering::AcqRel) {
            let _ = self.lifetime.sender.send(CastCommand::Shutdown);
        }
    }

    /// Connect to a Chromecast device
    pub fn connect(&self, ip: String, port: u16) -> Result<(), CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::Connect {
            ip,
            port,
            reply: reply_tx,
        })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }

    /// Disconnect from the current device
    pub fn disconnect(&self) -> Result<(), CastError> {
        let reply_rx = {
            let _gate = self
                .lifetime
                .command_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !self.lifetime.valid.swap(false, Ordering::AcqRel) {
                return Ok(());
            }
            let (reply_tx, reply_rx) = mpsc::channel();
            self.lifetime
                .sender
                .send(CastCommand::Disconnect { reply: reply_tx })
                .map_err(|_| CastError::Connection("Thread communication error".to_string()))?;
            // Disconnect permanently fences this handle. Queue shutdown now
            // so the worker exits after the disconnect even when the caller
            // retains an invalid clone.
            let _ = self.lifetime.sender.send(CastCommand::Shutdown);
            reply_rx
        };
        recv_teardown_reply(reply_rx, TEARDOWN_REPLY_TIMEOUT, "chromecast-disconnect")
    }

    /// Get device status
    pub fn get_status(&self) -> Result<CastStatus, CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::GetStatus { reply: reply_tx })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }

    /// Load media for playback
    pub fn load_media(
        &self,
        url: String,
        content_type: String,
        metadata: MediaMetadata,
    ) -> Result<(), CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::LoadMedia {
            url,
            content_type,
            metadata,
            reply: reply_tx,
        })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }

    /// Play
    pub fn play(&self) -> Result<(), CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::Play { reply: reply_tx })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }

    /// Pause
    pub fn pause(&self) -> Result<(), CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::Pause { reply: reply_tx })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }

    /// Stop
    pub fn stop(&self) -> Result<(), CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::Stop { reply: reply_tx })?;
        let result = recv_teardown_reply(reply_rx, TEARDOWN_REPLY_TIMEOUT, "chromecast-stop");
        if matches!(
            &result,
            Err(CastError::Timeout(_) | CastError::Connection(_))
        ) {
            self.invalidate();
        }
        result
    }

    /// Set volume
    pub fn set_volume(&self, volume: f32) -> Result<(), CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::SetVolume {
            volume,
            reply: reply_tx,
        })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }

    /// Seek
    pub fn seek(&self, position_secs: f64) -> Result<(), CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::Seek {
            position_secs,
            reply: reply_tx,
        })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }

    /// Get media position for seekbar updates
    pub fn get_media_position(&self) -> Result<CastPositionInfo, CastError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send_command(CastCommand::GetMediaPosition { reply: reply_tx })?;
        reply_rx
            .recv()
            .map_err(|_| CastError::Connection("Thread response error".to_string()))?
    }
}

impl Default for ChromecastHandle {
    fn default() -> Self {
        Self::new()
    }
}

fn recv_teardown_reply<T>(
    reply: Receiver<Result<T, CastError>>,
    timeout: Duration,
    operation: &'static str,
) -> Result<T, CastError> {
    match reply.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(CastError::Timeout(operation.to_string())),
        Err(RecvTimeoutError::Disconnected) => Err(CastError::Connection(
            "Chromecast worker response unavailable".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teardown_reply_preserves_worker_result() {
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok::<_, CastError>(7_u8)).unwrap();

        assert_eq!(
            recv_teardown_reply(receiver, Duration::ZERO, "test-operation").unwrap(),
            7
        );
    }

    #[test]
    fn teardown_reply_timeout_is_typed_and_sanitized() {
        let (_sender, receiver) = mpsc::channel::<Result<(), CastError>>();

        let result = recv_teardown_reply(receiver, Duration::ZERO, "test-operation");
        assert!(matches!(
            result,
            Err(CastError::Timeout(operation)) if operation == "test-operation"
        ));
    }

    #[test]
    fn teardown_reply_disconnect_is_typed_and_sanitized() {
        let (sender, receiver) = mpsc::channel::<Result<(), CastError>>();
        drop(sender);

        let result = recv_teardown_reply(receiver, Duration::ZERO, "test-operation");
        assert!(matches!(
            result,
            Err(CastError::Connection(message))
                if message == "Chromecast worker response unavailable"
        ));
    }

    #[test]
    fn invalidation_fences_every_clone() {
        let handle = ChromecastHandle::new();
        let clone = handle.clone();

        handle.invalidate();

        assert!(matches!(clone.play(), Err(CastError::NotConnected)));
    }
}

// Google Cast drops a control connection that goes roughly 10s without a
// heartbeat PING. Ping every 5s — the cadence the protocol expects — so a
// cast connection left idle between connect and load (e.g. while a track
// downloads) is not closed by the receiver. A 25s interval left idle
// connections dead, surfacing as EPIPE on the next LOAD (issue #439).
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Main loop for the Chromecast thread
fn chromecast_thread_main(receiver: Receiver<CastCommand>) {
    let mut connection: Option<CastDeviceConnection> = None;

    loop {
        let command = match receiver.recv_timeout(HEARTBEAT_INTERVAL) {
            Ok(cmd) => cmd,
            Err(RecvTimeoutError::Timeout) => {
                if let Some(conn) = connection.as_ref() {
                    if let Err(err) = conn.heartbeat() {
                        log::warn!("Chromecast heartbeat failed: {}", err);
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break, // Channel closed
        };

        match command {
            CastCommand::Connect { ip, port, reply } => {
                let result = CastDeviceConnection::connect(&ip, port);
                match result {
                    Ok(conn) => {
                        connection = Some(conn);
                        let _ = reply.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }

            CastCommand::Disconnect { reply } => {
                let result = if let Some(ref mut conn) = connection {
                    conn.disconnect()
                } else {
                    Ok(())
                };
                connection = None;
                let _ = reply.send(result);
            }

            CastCommand::GetStatus { reply } => {
                let result = match connection.as_ref() {
                    Some(conn) => conn.get_status(),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::GetMediaPosition { reply } => {
                let result = match connection.as_mut() {
                    Some(conn) => conn.get_media_position(),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::LoadMedia {
                url,
                content_type,
                metadata,
                reply,
            } => {
                let result = match connection.as_mut() {
                    Some(conn) => conn.load_media(&url, &content_type, metadata),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::Play { reply } => {
                let result = match connection.as_mut() {
                    Some(conn) => conn.play(),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::Pause { reply } => {
                let result = match connection.as_mut() {
                    Some(conn) => conn.pause(),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::Stop { reply } => {
                let result = match connection.as_mut() {
                    Some(conn) => conn.stop(),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::SetVolume { volume, reply } => {
                let result = match connection.as_mut() {
                    Some(conn) => conn.set_volume(volume),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::Seek {
                position_secs,
                reply,
            } => {
                let result = match connection.as_mut() {
                    Some(conn) => conn.seek(position_secs),
                    None => Err(CastError::NotConnected),
                };
                let _ = reply.send(result);
            }

            CastCommand::Shutdown => {
                if let Some(mut conn) = connection.take() {
                    let _ = conn.disconnect();
                }
                break;
            }
        }
    }
}
