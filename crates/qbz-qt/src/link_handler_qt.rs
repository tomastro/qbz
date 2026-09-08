//! Settings > Integrations > "Open Qobuz links in QBZ" — the OS-level
//! registration of QBZ as the handler for the official client's
//! `qobuzapp://` scheme, as a user choice rather than a side effect of
//! installing.
//!
//! WHY IT IS A SETTING. On Linux there is no official Qobuz desktop client,
//! so the launcher files claiming `x-scheme-handler/qobuzapp` displace
//! nobody. On macOS and Windows the official client exists and registers
//! the same scheme; an install that silently takes it over is the kind of
//! thing a user on a platform QBZ is only starting to reach (Windows) reads
//! as abusive. So `qbz://` — OUR scheme — stays registered by the packages
//! unconditionally, and `qobuzapp://` is claimed only when this toggle is on.
//!
//! DEFAULTS are per platform: on where it costs nobody anything (Linux) or
//! where the user base has already made the switch (macOS), off on Windows.
//! The default is only a DEFAULT: nothing here re-claims the scheme on every
//! launch. The registration moves when the user flips the toggle, and at
//! startup the only reconciliation is the conservative one — a registration
//! that points at THIS executable while the toggle is off is dropped (the
//! 2.0.x Windows MSI registered `qobuzapp` at install time; an upgraded
//! install must not keep a claim the user never made).
//!
//! Per platform:
//!   - Windows: `HKCU\Software\Classes\qobuzapp` (per-user, no elevation),
//!     the same keys the MSI writes for `qbz`. Off deletes the tree ONLY when
//!     its `shell\open\command` names this executable — another app's claim
//!     is never touched.
//!   - macOS: Launch Services. The bundle keeps declaring the scheme in
//!     `CFBundleURLTypes` (LS only lets a declared handler be default); on
//!     makes QBZ the default handler, off hands the default to the first
//!     OTHER registered handler (the official client when installed) and is
//!     a no-op when there is none.
//!   - Linux: `xdg-mime default com.blitzfc.qbz.desktop
//!     x-scheme-handler/qobuzapp` on; off is a no-op — there is no one to
//!     hand the scheme back to, and the desktop file keeps advertising it.
//!
//! The pref lives in the shared `ui_prefs.json` like the other Integrations
//! opt-ins (`discord_rpc_enabled`, `musicbrainz_enabled`).

use crate::settings_qt::{pref_bool, save_pref};

pub const PREF_KEY: &str = "qobuz_links_enabled";
const SCHEME: &str = "qobuzapp";

/// Platform default — see the module docs.
pub fn default_enabled() -> bool {
    !cfg!(target_os = "windows")
}

pub fn is_enabled() -> bool {
    pref_bool(PREF_KEY, default_enabled())
}

/// The toggle: persist, then move the registration.
pub fn set_enabled(value: bool) -> Result<(), String> {
    save_pref(PREF_KEY, serde_json::json!(value));
    let result = if value { claim() } else { release() };
    match &result {
        Ok(()) => log::info!("[qbz-qt] {PREF_KEY} -> {value}"),
        Err(e) => log::warn!("[qbz-qt] {PREF_KEY} -> {value}: registration failed: {e}"),
    }
    result
}

/// Once per session, from shell entry. Conservative on purpose: never
/// claims, only drops a claim that names this executable while the toggle
/// is off (Windows — the 2.0.x MSI wrote it at install time).
pub fn reconcile_at_startup() {
    if is_enabled() {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        if windows::command_names_this_exe() {
            match windows::delete_tree() {
                Ok(()) => log::info!(
                    "[qbz-qt] {SCHEME}:// handler pointed at this executable with the setting \
                     off (2.0.x installer) — dropped"
                ),
                Err(e) => log::warn!("[qbz-qt] could not drop the {SCHEME}:// handler: {e}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn claim() -> Result<(), String> {
    let status = std::process::Command::new("xdg-mime")
        .args([
            "default",
            "com.blitzfc.qbz.desktop",
            &format!("x-scheme-handler/{SCHEME}"),
        ])
        .status()
        .map_err(|e| format!("xdg-mime: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("xdg-mime exited with {status}"))
    }
}

#[cfg(target_os = "linux")]
fn release() -> Result<(), String> {
    // Nothing to hand the scheme back to; the desktop entry keeps advertising
    // it and the user's mimeapps.list keeps whatever it says.
    Ok(())
}

#[cfg(target_os = "windows")]
fn claim() -> Result<(), String> {
    windows::write_tree()
}

#[cfg(target_os = "windows")]
fn release() -> Result<(), String> {
    if windows::command_names_this_exe() {
        windows::delete_tree()
    } else {
        // Someone else's registration (the official client): leave it.
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn claim() -> Result<(), String> {
    macos::set_default_handler(macos::BUNDLE_ID)
}

#[cfg(target_os = "macos")]
fn release() -> Result<(), String> {
    match macos::other_handler() {
        Some(other) => macos::set_default_handler(&other),
        None => Ok(()),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn claim() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn release() -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows: HKCU\Software\Classes\qobuzapp
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod windows {
    use super::SCHEME;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE,
        REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn key_path() -> String {
        format!("Software\\Classes\\{SCHEME}")
    }

    fn exe_path() -> Result<String, String> {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| format!("current_exe: {e}"))
    }

    /// Create (or open) `subkey` under HKCU and write its default value.
    fn write_default(subkey: &str, value: &str) -> Result<HKEY, String> {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(subkey);
        // SAFETY: valid null-terminated wide strings; out-pointer to a local.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(format!("RegCreateKeyExW({subkey}) = {rc}"));
        }
        set_string(key, None, value)?;
        Ok(key)
    }

    fn set_string(key: HKEY, name: Option<&str>, value: &str) -> Result<(), String> {
        let data = wide(value);
        let name_w = name.map(wide);
        let name_ptr = name_w.as_ref().map(|v| v.as_ptr()).unwrap_or(std::ptr::null());
        // SAFETY: `data` outlives the call; byte length includes the terminator.
        let rc = unsafe {
            RegSetValueExW(
                key,
                name_ptr,
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(format!("RegSetValueExW({}) = {rc}", name.unwrap_or("@")));
        }
        Ok(())
    }

    /// The same four entries `packaging/windows/qbz.wxs` writes for `qbz`.
    pub(super) fn write_tree() -> Result<(), String> {
        let exe = exe_path()?;
        let root = key_path();
        let key = write_default(&root, "URL:Qobuz App Protocol")?;
        let r = set_string(key, Some("URL Protocol"), "");
        // SAFETY: key came from RegCreateKeyExW.
        unsafe { RegCloseKey(key) };
        r?;
        let icon = write_default(&format!("{root}\\DefaultIcon"), &format!("\"{exe}\",0"))?;
        unsafe { RegCloseKey(icon) };
        let cmd = write_default(
            &format!("{root}\\shell\\open\\command"),
            &format!("\"{exe}\" \"%1\""),
        )?;
        unsafe { RegCloseKey(cmd) };
        Ok(())
    }

    /// The registered `shell\open\command`, if any.
    fn command_value() -> Option<String> {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(&format!("{}\\shell\\open\\command", key_path()));
        // SAFETY: valid wide string; out-pointer to a local.
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let mut len: u32 = 0;
        let mut ty: u32 = 0;
        // SAFETY: size probe with a null buffer, per the RegQueryValueExW contract.
        let rc = unsafe {
            RegQueryValueExW(
                key,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut ty,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if rc != ERROR_SUCCESS || len == 0 {
            unsafe { RegCloseKey(key) };
            return None;
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        // SAFETY: `buf` has `len` bytes.
        let rc = unsafe {
            RegQueryValueExW(
                key,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut ty,
                buf.as_mut_ptr() as *mut u8,
                &mut len,
            )
        };
        unsafe { RegCloseKey(key) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Does the current `qobuzapp` claim point at THIS executable?
    pub(super) fn command_names_this_exe() -> bool {
        let (Some(cmd), Ok(exe)) = (command_value(), exe_path()) else {
            return false;
        };
        cmd.to_lowercase().contains(&exe.to_lowercase())
    }

    pub(super) fn delete_tree() -> Result<(), String> {
        let path = wide(&key_path());
        // SAFETY: valid wide string.
        let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
        if rc != ERROR_SUCCESS {
            return Err(format!("RegDeleteTreeW = {rc}"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// macOS: Launch Services
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::SCHEME;
    use std::ffi::{c_char, c_void, CString};

    pub(super) const BUNDLE_ID: &str = "com.blitzfc.qbz";

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFIndex = isize;
    type OSStatus = i32;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            cstr: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFStringGetCString(
            s: CFStringRef,
            buffer: *mut c_char,
            size: CFIndex,
            encoding: u32,
        ) -> u8;
        fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn LSSetDefaultHandlerForURLScheme(scheme: CFStringRef, bundle_id: CFStringRef)
            -> OSStatus;
        fn LSCopyAllHandlersForURLScheme(scheme: CFStringRef) -> CFArrayRef;
    }

    struct CfString(CFStringRef);
    impl CfString {
        fn new(s: &str) -> Result<Self, String> {
            let c = CString::new(s).map_err(|e| e.to_string())?;
            // SAFETY: `c` is a valid C string for the duration of the call.
            let r = unsafe {
                CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8)
            };
            if r.is_null() {
                return Err("CFStringCreateWithCString returned NULL".into());
            }
            Ok(Self(r))
        }
    }
    impl Drop for CfString {
        fn drop(&mut self) {
            // SAFETY: created by CFStringCreateWithCString, owned here.
            unsafe { CFRelease(self.0) }
        }
    }

    fn cf_to_string(s: CFStringRef) -> Option<String> {
        let mut buf = vec![0 as c_char; 512];
        // SAFETY: `buf` is `size` bytes long.
        let ok = unsafe {
            CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as CFIndex, K_CF_STRING_ENCODING_UTF8)
        };
        if ok == 0 {
            return None;
        }
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8(bytes).ok()
    }

    pub(super) fn set_default_handler(bundle_id: &str) -> Result<(), String> {
        let scheme = CfString::new(SCHEME)?;
        let bundle = CfString::new(bundle_id)?;
        // SAFETY: both CFStrings are live for the call.
        let status = unsafe { LSSetDefaultHandlerForURLScheme(scheme.0, bundle.0) };
        if status != 0 {
            return Err(format!("LSSetDefaultHandlerForURLScheme({bundle_id}) = {status}"));
        }
        Ok(())
    }

    /// The first registered handler for the scheme that is not QBZ.
    pub(super) fn other_handler() -> Option<String> {
        let scheme = CfString::new(SCHEME).ok()?;
        // SAFETY: the returned array is owned (Copy rule) and released below.
        let handlers = unsafe { LSCopyAllHandlersForURLScheme(scheme.0) };
        if handlers.is_null() {
            return None;
        }
        let mut found = None;
        // SAFETY: `handlers` is a live CFArray of CFStrings.
        unsafe {
            let n = CFArrayGetCount(handlers);
            for i in 0..n {
                let item = CFArrayGetValueAtIndex(handlers, i);
                if let Some(id) = cf_to_string(item) {
                    if !id.eq_ignore_ascii_case(BUNDLE_ID) {
                        found = Some(id);
                        break;
                    }
                }
            }
            CFRelease(handlers);
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one rule that must never drift: Windows is opt-IN, the rest
    /// opt-out.
    #[test]
    fn windows_defaults_off_others_on() {
        assert_eq!(default_enabled(), !cfg!(target_os = "windows"));
    }
}
