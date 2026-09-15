//! Thin Android `usbfs` edge.
//!
//! `UsbManager.openDevice()` remains responsible for permission, interface
//! claims, clock controls and alternate-setting selection. Java passes its
//! file descriptor to Rust; this type duplicates it and submits isochronous
//! URBs directly, without AudioTrack/AAudio/AudioFlinger.

use libc::{c_int, c_void};
use std::collections::VecDeque;
use std::io;
use std::mem::size_of;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{pack_f32_le, pack_i32_le, PacketPlanner, PcmFormat};

const USBDEVFS_URB_TYPE_ISO: u8 = 0;
const USBDEVFS_URB_ISO_ASAP: u32 = 0x02;
const USBDEVFS_MAX_ISO_PACKETS: usize = 128;

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(direction: u32, kind: u8, number: u8, size: usize) -> c_int {
    ((direction << IOC_DIRSHIFT)
        | ((kind as u32) << IOC_TYPESHIFT)
        | ((number as u32) << IOC_NRSHIFT)
        | ((size as u32) << IOC_SIZESHIFT)) as c_int
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UsbDevFsIsoPacketDesc {
    length: u32,
    actual_length: u32,
    status: i32,
}

#[repr(C)]
struct UsbDevFsUrb {
    kind: u8,
    endpoint: u8,
    status: c_int,
    flags: u32,
    buffer: *mut c_void,
    buffer_length: c_int,
    actual_length: c_int,
    start_frame: c_int,
    number_of_packets: c_int,
    error_count: c_int,
    signr: u32,
    user_context: *mut c_void,
}

#[repr(C)]
struct IsoUrbStorage {
    urb: UsbDevFsUrb,
    packets: [UsbDevFsIsoPacketDesc; USBDEVFS_MAX_ISO_PACKETS],
}

const USBDEVFS_SUBMITURB: c_int = ioc(IOC_READ, b'U', 10, size_of::<UsbDevFsUrb>());
const USBDEVFS_DISCARDURB: c_int = ioc(IOC_NONE, b'U', 11, 0);
const USBDEVFS_REAPURB: c_int = ioc(IOC_WRITE, b'U', 12, size_of::<*mut c_void>());
const USBDEVFS_REAPURBNDELAY: c_int = ioc(IOC_WRITE, b'U', 13, size_of::<*mut c_void>());
const USBDEVFS_GET_SPEED: c_int = ioc(IOC_NONE, b'U', 31, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoCompletion {
    pub bytes: usize,
    pub packet_errors: usize,
}

/// Owns a duplicate of Android's `UsbDeviceConnection` file descriptor.
pub struct AndroidUsbFs {
    fd: OwnedFd,
}

#[derive(Debug, Clone, Copy)]
pub struct AndroidUsbDirectConfig {
    pub java_fd: RawFd,
    pub data_endpoint: u8,
    pub feedback_endpoint: u8,
    pub max_packet_size: usize,
    pub service_rate: u32,
    pub format: PcmFormat,
}

const NUM_URBS: usize = 32;
const PACKETS_PER_URB: usize = 64;
const REAP_TIMEOUT: Duration = Duration::from_secs(3);

struct UrbSlot {
    storage: Box<IsoUrbStorage>,
    payload: Vec<u8>,
    in_flight: bool,
}

// SAFETY: an `UrbSlot` owns both allocations referenced by the raw pointers in
// its `UsbDevFsUrb`. `storage` is boxed, so moving the slot never changes the
// URB address submitted to usbfs; `payload` is never grown or otherwise
// reallocated while that URB is in flight. All mutation, submission, reaping,
// and cancellation is serialized by `AndroidUsbDirectStream::state`. Moving
// the complete slot to the dedicated QBZ direct-writer thread therefore does
// not invalidate or concurrently expose either pointer.
unsafe impl Send for UrbSlot {}

struct WriteState {
    planner: PacketPlanner,
    pending: VecDeque<u8>,
    pack_scratch: Vec<u8>,
    slots: Vec<UrbSlot>,
    active_urbs: usize,
}

/// Blocking PCM sink used by QBZ's existing direct-writer thread.
///
/// Keeps thirty-two 64-microframe URBs (about 256ms of audio) continuously in
/// flight. This absorbs scheduler stalls while display power state changes.
pub struct AndroidUsbDirectStream {
    usb: AndroidUsbFs,
    data_endpoint: u8,
    feedback_endpoint: u8,
    format: PcmFormat,
    state: Mutex<WriteState>,
}

impl AndroidUsbDirectStream {
    pub fn new(config: AndroidUsbDirectConfig) -> Result<Self, String> {
        let planner =
            PacketPlanner::new(config.format, config.service_rate, config.max_packet_size)
                .map_err(|error| error.to_string())?;
        let usb = AndroidUsbFs::duplicate_from_java_fd(config.java_fd)
            .map_err(|error| format!("failed to duplicate Android USB fd: {error}"))?;

        let mut slots = Vec::with_capacity(NUM_URBS);
        for _ in 0..NUM_URBS {
            slots.push(UrbSlot {
                storage: Box::new(IsoUrbStorage {
                    urb: UsbDevFsUrb {
                        kind: 0,
                        endpoint: 0,
                        status: 0,
                        flags: 0,
                        buffer: ptr::null_mut(),
                        buffer_length: 0,
                        actual_length: 0,
                        start_frame: 0,
                        number_of_packets: 0,
                        error_count: 0,
                        signr: 0,
                        user_context: ptr::null_mut(),
                    },
                    packets: [UsbDevFsIsoPacketDesc::default(); USBDEVFS_MAX_ISO_PACKETS],
                }),
                payload: Vec::with_capacity(PACKETS_PER_URB * config.max_packet_size),
                in_flight: false,
            });
        }

        Ok(Self {
            usb,
            data_endpoint: config.data_endpoint,
            feedback_endpoint: config.feedback_endpoint,
            format: config.format,
            state: Mutex::new(WriteState {
                planner,
                pending: VecDeque::new(),
                pack_scratch: Vec::new(),
                slots,
                active_urbs: 0,
            }),
        })
    }

    pub fn write_f32(&self, samples: &[f32]) -> Result<(), String> {
        if samples.len() % self.format.channels as usize != 0 {
            return Err("USB direct write ended on a partial PCM frame".to_string());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "USB direct writer state is poisoned".to_string())?;
        pack_f32_le(samples, self.format, &mut state.pack_scratch);
        let packed = std::mem::take(&mut state.pack_scratch);
        state.pending.extend(packed.iter().copied());
        state.pack_scratch = packed;
        self.flush_ready_packets(&mut state)
    }

    pub fn write_i32(&self, samples: &[i32]) -> Result<(), String> {
        if samples.len() % self.format.channels as usize != 0 {
            return Err("USB direct write ended on a partial PCM frame".to_string());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "USB direct writer state is poisoned".to_string())?;
        pack_i32_le(samples, self.format, &mut state.pack_scratch);
        let packed = std::mem::take(&mut state.pack_scratch);
        state.pending.extend(packed.iter().copied());
        state.pack_scratch = packed;
        self.flush_ready_packets(&mut state)
    }

    fn reap_completed_urb(&self, timeout: Duration) -> Result<*mut UsbDevFsUrb, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut completed_ptr: *mut UsbDevFsUrb = ptr::null_mut();
            let reaped = unsafe {
                libc::ioctl(
                    self.usb.raw_fd(),
                    USBDEVFS_REAPURBNDELAY,
                    (&mut completed_ptr as *mut *mut UsbDevFsUrb).cast::<c_void>(),
                )
            };
            if reaped == 0 {
                return Ok(completed_ptr);
            }
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if err.kind() != io::ErrorKind::WouldBlock {
                return Err(format!("usbfs reap URB failed: {err}"));
            }
            if Instant::now() >= deadline {
                return Err("USB isochronous transfer timed out".to_string());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn drain(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "USB direct writer state is poisoned".to_string())?;

        if !state.pending.is_empty() {
            let mut temp_planner = state.planner.clone();
            let mut total_bytes = 0usize;
            for _ in 0..PACKETS_PER_URB {
                let req = match temp_planner.next_packet_bytes() {
                    Ok(b) => b,
                    Err(e) => return Err(e.to_string()),
                };
                total_bytes += req;
            }
            if state.pending.len() < total_bytes {
                state.pending.resize(total_bytes, 0);
            }
            self.flush_ready_packets(&mut state)?;
        }

        while state.active_urbs > 0 {
            let completed_ptr = self.reap_completed_urb(REAP_TIMEOUT)?;

            if let Some(slot_idx) = state.slots.iter().position(|s| {
                &s.storage.urb as *const UsbDevFsUrb == completed_ptr as *const UsbDevFsUrb
            }) {
                state.slots[slot_idx].in_flight = false;
                state.active_urbs -= 1;
            }
        }

        Ok(())
    }

    pub fn stop(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "USB direct writer state is poisoned".to_string())?;

        state.pending.clear();

        for slot in &mut state.slots {
            if slot.in_flight {
                unsafe {
                    libc::ioctl(
                        self.usb.raw_fd(),
                        USBDEVFS_DISCARDURB,
                        (&mut slot.storage.urb as *mut UsbDevFsUrb).cast::<c_void>(),
                    );
                }
            }
        }

        while state.active_urbs > 0 {
            if let Ok(completed_ptr) = self.reap_completed_urb(Duration::from_millis(500)) {
                if let Some(slot_idx) = state.slots.iter().position(|s| {
                    &s.storage.urb as *const UsbDevFsUrb == completed_ptr as *const UsbDevFsUrb
                }) {
                    state.slots[slot_idx].in_flight = false;
                    state.active_urbs -= 1;
                }
            } else {
                for slot in &mut state.slots {
                    slot.in_flight = false;
                }
                state.active_urbs = 0;
                break;
            }
        }

        Ok(())
    }

    pub const fn sample_rate(&self) -> u32 {
        self.format.sample_rate
    }

    pub const fn channels(&self) -> u16 {
        self.format.channels
    }

    pub const fn feedback_endpoint(&self) -> u8 {
        self.feedback_endpoint
    }

    pub fn playback_delay_frames(&self) -> Result<u64, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "USB direct writer state is poisoned".to_string())?;
        Ok((state.pending.len() / self.format.bytes_per_frame()) as u64)
    }

    fn flush_ready_packets(&self, state: &mut WriteState) -> Result<(), String> {
        loop {
            while state.active_urbs < NUM_URBS {
                let mut temp_planner = state.planner.clone();
                let mut lengths = Vec::with_capacity(PACKETS_PER_URB);
                let mut total_bytes = 0usize;
                for _ in 0..PACKETS_PER_URB {
                    let req = match temp_planner.next_packet_bytes() {
                        Ok(b) => b,
                        Err(e) => return Err(e.to_string()),
                    };
                    lengths.push(req as u16);
                    total_bytes += req;
                }

                if state.pending.len() < total_bytes {
                    break;
                }

                for _ in 0..PACKETS_PER_URB {
                    let _ = state.planner.next_packet_bytes();
                }

                let slot_idx = match state.slots.iter().position(|s| !s.in_flight) {
                    Some(idx) => idx,
                    None => break,
                };

                let slot = &mut state.slots[slot_idx];
                slot.payload.clear();
                slot.payload.extend(state.pending.drain(..total_bytes));

                slot.storage.urb = UsbDevFsUrb {
                    kind: USBDEVFS_URB_TYPE_ISO,
                    endpoint: self.data_endpoint,
                    status: 0,
                    flags: USBDEVFS_URB_ISO_ASAP,
                    buffer: slot.payload.as_mut_ptr().cast(),
                    buffer_length: total_bytes as c_int,
                    actual_length: 0,
                    start_frame: 0,
                    number_of_packets: lengths.len() as c_int,
                    error_count: 0,
                    signr: 0,
                    user_context: ptr::null_mut(),
                };

                for (desc, &len) in slot.storage.packets.iter_mut().zip(&lengths) {
                    desc.length = u32::from(len);
                    desc.actual_length = 0;
                    desc.status = 0;
                }

                let ret = unsafe {
                    libc::ioctl(
                        self.usb.raw_fd(),
                        USBDEVFS_SUBMITURB,
                        (&mut slot.storage.urb as *mut UsbDevFsUrb).cast::<c_void>(),
                    )
                };
                if ret < 0 {
                    return Err(format!(
                        "usbfs submit URB failed: {}",
                        io::Error::last_os_error()
                    ));
                }

                slot.in_flight = true;
                state.active_urbs += 1;
            }

            if state.active_urbs == NUM_URBS {
                let completed_ptr = self.reap_completed_urb(REAP_TIMEOUT)?;

                let slot_idx = state
                    .slots
                    .iter()
                    .position(|s| {
                        &s.storage.urb as *const UsbDevFsUrb == completed_ptr as *const UsbDevFsUrb
                    })
                    .ok_or_else(|| "usbfs reaped an unexpected URB pointer".to_string())?;

                state.slots[slot_idx].in_flight = false;
                state.active_urbs -= 1;
            } else {
                break;
            }
        }
        Ok(())
    }
}

impl Drop for AndroidUsbDirectStream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl AndroidUsbFs {
    /// Duplicate the descriptor so closing the Java connection and dropping
    /// this Rust transport have independent, deterministic lifetimes.
    pub fn duplicate_from_java_fd(java_fd: RawFd) -> io::Result<Self> {
        if java_fd < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "negative UsbDeviceConnection file descriptor",
            ));
        }
        // SAFETY: `dup` neither borrows nor takes ownership of `java_fd`.
        let fd = unsafe { libc::dup(java_fd) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful `dup` returns a new descriptor owned by us.
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    pub fn usb_speed(&self) -> io::Result<i32> {
        // SAFETY: GET_SPEED has no pointer argument and does not outlive fd.
        let result = unsafe { libc::ioctl(self.raw_fd(), USBDEVFS_GET_SPEED) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    /// Submit and synchronously reap one isochronous URB.
    ///
    /// Production pacing keeps several calls in flight from its dedicated USB
    /// worker. This primitive deliberately owns no Android UI or JNI state.
    pub fn transfer_iso(
        &self,
        endpoint: u8,
        packet_lengths: &[u16],
        payload: &mut [u8],
    ) -> io::Result<IsoCompletion> {
        if packet_lengths.is_empty() || packet_lengths.len() > USBDEVFS_MAX_ISO_PACKETS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "isochronous URB packet count must be 1..=128",
            ));
        }
        let requested: usize = packet_lengths
            .iter()
            .map(|&length| usize::from(length))
            .sum();
        if requested > payload.len() || requested > c_int::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "isochronous payload is shorter than its packet table",
            ));
        }

        let mut storage = Box::new(IsoUrbStorage {
            urb: UsbDevFsUrb {
                kind: USBDEVFS_URB_TYPE_ISO,
                endpoint,
                status: 0,
                flags: USBDEVFS_URB_ISO_ASAP,
                buffer: payload.as_mut_ptr().cast(),
                buffer_length: requested as c_int,
                actual_length: 0,
                start_frame: 0,
                number_of_packets: packet_lengths.len() as c_int,
                error_count: 0,
                signr: 0,
                user_context: ptr::null_mut(),
            },
            packets: [UsbDevFsIsoPacketDesc::default(); USBDEVFS_MAX_ISO_PACKETS],
        });
        for (descriptor, &length) in storage.packets.iter_mut().zip(packet_lengths) {
            descriptor.length = u32::from(length);
        }

        // SAFETY: storage and payload remain pinned/alive until REAPURB returns.
        let submitted = unsafe {
            libc::ioctl(
                self.raw_fd(),
                USBDEVFS_SUBMITURB,
                (&mut storage.urb as *mut UsbDevFsUrb).cast::<c_void>(),
            )
        };
        if submitted < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut completed: *mut UsbDevFsUrb = ptr::null_mut();
        loop {
            // SAFETY: kernel writes one pointer into `completed`; the submitted
            // URB remains alive and this fd is owned for the entire call.
            let reaped = unsafe {
                libc::ioctl(
                    self.raw_fd(),
                    USBDEVFS_REAPURB,
                    (&mut completed as *mut *mut UsbDevFsUrb).cast::<c_void>(),
                )
            };
            if reaped == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                // Best-effort cancellation before storage is released.
                unsafe {
                    libc::ioctl(
                        self.raw_fd(),
                        USBDEVFS_DISCARDURB,
                        (&mut storage.urb as *mut UsbDevFsUrb).cast::<c_void>(),
                    );
                }
                return Err(error);
            }
        }

        if completed != &mut storage.urb {
            return Err(io::Error::other("usbfs reaped an unexpected URB pointer"));
        }
        let packet_count = storage.urb.number_of_packets.max(0) as usize;
        let packets = &storage.packets[..packet_count.min(packet_lengths.len())];
        let packet_errors = packets.iter().filter(|packet| packet.status != 0).count()
            + usize::from(storage.urb.status != 0);
        let bytes = packets
            .iter()
            .map(|packet| packet.actual_length as usize)
            .sum();
        Ok(IsoCompletion {
            bytes,
            packet_errors,
        })
    }

    fn raw_fd(&self) -> RawFd {
        use std::os::fd::AsRawFd;
        self.fd.as_raw_fd()
    }
}
