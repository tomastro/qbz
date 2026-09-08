// crates/qbzd/src/lock.rs
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Offset of the Windows lock byte: 1 GiB, far past the few bytes of pid text.
#[cfg(windows)]
const LOCK_BYTE_OFFSET: u32 = 1 << 30;

#[derive(Debug)]
pub struct InstanceLock {
    _file: std::fs::File,
}

#[derive(Debug)]
pub enum LockError {
    AlreadyRunning(Option<u32>),
    Io(String),
}

impl InstanceLock {
    /// flock on <data_root>/qbzd.lock, taken BEFORE the port bind (01 §8.1-4).
    /// Two daemons on one root = one device_uuid presented twice + two session.db
    /// writers — the lock is what protects those invariants, not the port.
    pub fn acquire(data_root: &Path) -> Result<Self, LockError> {
        std::fs::create_dir_all(data_root).map_err(|e| LockError::Io(e.to_string()))?;
        let path = data_root.join("qbzd.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| LockError::Io(e.to_string()))?;
        // Ok(true) = we hold it. Ok(false) = another instance does.
        // Err = the lock could not even be ATTEMPTED, which is not the same
        // thing and must not be reported as AlreadyRunning.
        #[cfg(unix)]
        let locked: Result<bool, std::io::Error> = {
            use std::os::unix::io::AsRawFd;
            // Deliberately NOT narrowed to EWOULDBLOCK: this is byte-identical
            // to the pre-port behaviour, and the Windows port does not move
            // Linux semantics.
            Ok(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 })
        };
        #[cfg(windows)]
        let locked: Result<bool, std::io::Error> = {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
            use windows_sys::Win32::Storage::FileSystem::{
                LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            };
            let mut ov: windows_sys::Win32::System::IO::OVERLAPPED =
                unsafe { std::mem::zeroed() };
            // A LOCK BYTE past any real content, not the whole file. Windows
            // byte-range locks are MANDATORY: locking from offset 0 would make
            // the `read_to_string` below fail for the losing instance, so
            // AlreadyRunning would always carry None instead of the pid that is
            // the whole point of the error. Reserving a region past EOF is the
            // same trick SQLite's Windows VFS uses. flock's advisory semantics
            // need no such care.
            ov.Anonymous.Anonymous.Offset = LOCK_BYTE_OFFSET;
            // SAFETY: `file` is open so its handle is valid for the call; `ov`
            // is fully initialised and outlives the call. The lock is released
            // when `file` drops and the handle closes, like flock.
            let ok = unsafe {
                LockFileEx(
                    file.as_raw_handle() as _,
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    1,
                    0,
                    &mut ov,
                )
            };
            if ok != 0 {
                Ok(true)
            } else {
                let err = std::io::Error::last_os_error();
                // Only ERROR_LOCK_VIOLATION means "another instance holds it".
                // A filesystem that does not implement byte-range locks answers
                // ERROR_INVALID_FUNCTION instead (network shares, \wsl$), and
                // calling that "another daemon is running" would be a lie that
                // blocks startup for good.
                if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
                    Ok(false)
                } else {
                    Err(err)
                }
            }
        };
        match locked {
            Ok(true) => {}
            Ok(false) => {
                let mut pid = String::new();
                let _ = file.read_to_string(&mut pid);
                return Err(LockError::AlreadyRunning(pid.trim().parse().ok()));
            }
            Err(e) => return Err(LockError::Io(e.to_string())),
        }
        file.set_len(0)
            .and_then(|_| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|_| write!(file, "{}", std::process::id()))
            .map_err(|e| LockError::Io(e.to_string()))?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_acquire_fails_with_pid() {
        let dir = tempfile::tempdir().unwrap();
        let _l1 = InstanceLock::acquire(dir.path()).unwrap();
        // flock is per-open-file-description: a second open in the SAME process
        // still conflicts, which is exactly what we need to test.
        match InstanceLock::acquire(dir.path()) {
            Err(LockError::AlreadyRunning(pid)) => assert_eq!(pid, Some(std::process::id())),
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
    }
    #[test]
    fn released_lock_reacquires() {
        let dir = tempfile::tempdir().unwrap();
        drop(InstanceLock::acquire(dir.path()).unwrap());
        assert!(InstanceLock::acquire(dir.path()).is_ok());
    }
}
