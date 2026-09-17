//! Android-only Qt shell glue for the frontend-neutral USB Direct sink.

use jni::objects::{GlobalRef, JClass, JLongArray, JObject, JValue};
use jni::JavaVM;
use qbz_audio::android_usb_direct::{
    AndroidUsbDirectConfig, AndroidUsbDirectStream, PcmFormat,
};
use std::sync::{Arc, OnceLock};

static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();
static USB_DIRECT_BRIDGE_CLASS: OnceLock<GlobalRef> = OnceLock::new();
static TLS_INITIALIZED: OnceLock<()> = OnceLock::new();
static FACTORY_INSTALLED: OnceLock<()> = OnceLock::new();

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_qt_QbzActivity_initializeNativeBridge(
    mut env: jni::JNIEnv,
    _class: JClass,
    context: JObject,
) {
    let result = (|| -> Result<(), String> {
        let vm = env
            .get_java_vm()
            .map_err(|error| format!("failed to obtain Android JavaVM: {error}"))?;
        let _ = JAVA_VM.set(vm);

        // Cache the UsbDirectBridge class while on the application's classloader thread
        if USB_DIRECT_BRIDGE_CLASS.get().is_none() {
            match env.find_class("dev/qbz/android/qt/UsbDirectBridge") {
                Ok(class) => match env.new_global_ref(class) {
                    Ok(global_ref) => {
                        let _ = USB_DIRECT_BRIDGE_CLASS.set(global_ref);
                        log::info!("[android_usb_qt] Successfully cached UsbDirectBridge GlobalRef");
                    }
                    Err(e) => {
                        log::error!("[android_usb_qt] Failed to create GlobalRef for UsbDirectBridge: {e}");
                    }
                },
                Err(e) => {
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_describe();
                        let _ = env.exception_clear();
                    }
                    log::error!("[android_usb_qt] Failed to find UsbDirectBridge class: {e}");
                }
            }
        }

        // Ensure directories and environment variables for Rust's dirs crate
        let files_dir = "/data/user/0/dev.qbz.android.qt/files";
        let cache_dir = "/data/user/0/dev.qbz.android.qt/cache";
        let _ = std::fs::create_dir_all(files_dir);
        let _ = std::fs::create_dir_all(cache_dir);
        std::env::set_var("HOME", files_dir);
        std::env::set_var("XDG_DATA_HOME", files_dir);
        std::env::set_var("XDG_CACHE_HOME", cache_dir);
        std::env::set_var("XDG_CONFIG_HOME", files_dir);
        unsafe {
            libc::setenv(b"HOME\0".as_ptr() as *const _, b"/data/user/0/dev.qbz.android.qt/files\0".as_ptr() as *const _, 1);
            libc::setenv(b"XDG_DATA_HOME\0".as_ptr() as *const _, b"/data/user/0/dev.qbz.android.qt/files\0".as_ptr() as *const _, 1);
            libc::setenv(b"XDG_CACHE_HOME\0".as_ptr() as *const _, b"/data/user/0/dev.qbz.android.qt/cache\0".as_ptr() as *const _, 1);
            libc::setenv(b"XDG_CONFIG_HOME\0".as_ptr() as *const _, b"/data/user/0/dev.qbz.android.qt/files\0".as_ptr() as *const _, 1);
        }
        log::info!("[android_usb_qt] Data directories initialized: HOME={files_dir}, XDG_DATA_HOME={files_dir}");

        // reqwest/rustls needs Android's Network Security trust manager. This
        // is the same one-time initialization used by the earlier Java probe,
        // now performed by the thin Qt Activity before any login request.
        if TLS_INITIALIZED.get().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let raw_env = env.get_raw() as *mut jni22::sys::JNIEnv;
            let raw_context = context.as_raw() as jni22::sys::jobject;
            let mut hosted_env = unsafe { jni22::EnvUnowned::from_raw(raw_env) };
            let outcome = hosted_env
                .with_env(|env22| -> jni22::errors::Result<()> {
                    let context22 =
                        unsafe { jni22::objects::JObject::from_raw(env22, raw_context) };
                    rustls_platform_verifier::android::init_with_env(env22, context22)
                })
                .into_outcome();
            match outcome {
                jni22::Outcome::Ok(()) => {
                    let _ = TLS_INITIALIZED.set(());
                }
                jni22::Outcome::Err(error) => {
                    log::warn!("[android_usb_qt] rustls_platform_verifier init failed: {error:?}");
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_describe();
                        let _ = env.exception_clear();
                    }
                }
                jni22::Outcome::Panic(_payload) => {
                    log::warn!("[android_usb_qt] rustls_platform_verifier init panicked");
                }
            }
        }

        install()
    })();
    if let Err(error) = result {
        let _ = env.throw_new("java/lang/IllegalStateException", error);
    }
}

fn install() -> Result<(), String> {
    if JAVA_VM.get().is_none() {
        return Err("Qt Activity did not provide the Android JavaVM".to_string());
    }
    if FACTORY_INSTALLED.get().is_some() {
        return Ok(());
    }
    qbz_audio::android_usb_direct::install_factory(Arc::new(open_stream))?;
    let _ = FACTORY_INSTALLED.set(());
    Ok(())
}

fn open_stream(
    sample_rate: u32,
    channels: u16,
    bit_depth: u32,
) -> Result<Arc<dyn qbz_audio::backend::DirectSink>, String> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| "Android JavaVM is unavailable".to_string())?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("failed to attach USB writer to JVM: {error}"))?;
    let class = USB_DIRECT_BRIDGE_CLASS
        .get()
        .ok_or_else(|| "UsbDirectBridge class is not initialized".to_string())?;

    let value = env
        .call_static_method(
            class,
            "openStream",
            "(III)[J",
            &[
                JValue::Int(sample_rate as i32),
                JValue::Int(channels as i32),
                JValue::Int(bit_depth as i32),
            ],
        )
        .map_err(|error| java_error(&mut env, "USB bridge call failed", error))?;
    let object = value
        .l()
        .map_err(|error| format!("USB bridge returned a non-array value: {error}"))?;
    if object.is_null() {
        return Err("USB DAC is unavailable or permission has not been granted".to_string());
    }
    let array = JLongArray::from(object);
    let len = env
        .get_array_length(&array)
        .map_err(|error| format!("failed to read USB bridge result: {error}"))?;
    if len != 8 {
        return Err(format!("USB bridge returned {len} values, expected 8"));
    }
    let mut values = [0_i64; 8];
    env.get_long_array_region(&array, 0, &mut values)
        .map_err(|error| format!("failed to copy USB bridge result: {error}"))?;

    let actual_rate = values[7] as u32;
    let actual_bits = values[6] as u32;
    if actual_rate != sample_rate || actual_bits != bit_depth {
        return Err(format!(
            "DAC format confirmation mismatch: source {sample_rate} Hz/{bit_depth}-bit, DAC {actual_rate} Hz/{actual_bits}-bit"
        ));
    }

    let inner = AndroidUsbDirectStream::new(AndroidUsbDirectConfig {
        java_fd: values[0] as i32,
        data_endpoint: values[1] as u8,
        feedback_endpoint: values[2] as u8,
        max_packet_size: values[3] as usize,
        service_rate: values[4] as u32,
        format: PcmFormat {
            sample_rate,
            channels,
            subslot_bytes: values[5] as u8,
            bit_resolution: actual_bits as u8,
        },
    })
    .map_err(any_err_to_string)?;
    Ok(Arc::new(ManagedUsbDirectStream { inner }))
}

fn any_err_to_string(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct ManagedUsbDirectStream {
    inner: AndroidUsbDirectStream,
}

impl qbz_audio::backend::DirectSink for ManagedUsbDirectStream {
    fn write_f32(&self, samples: &[f32]) -> Result<(), String> {
        self.inner.write_f32(samples)
    }

    fn drain(&self) -> Result<(), String> {
        self.inner.drain()
    }

    fn stop(&self) -> Result<(), String> {
        self.inner.stop()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn playback_delay_frames(&self) -> Result<u64, String> {
        self.inner.playback_delay_frames()
    }

    fn log_label(&self) -> &'static str {
        "Android USB Direct"
    }
}

impl Drop for ManagedUsbDirectStream {
    fn drop(&mut self) {
        let Some(vm) = JAVA_VM.get() else { return };
        let Ok(mut env) = vm.attach_current_thread() else { return };
        let Some(class) = USB_DIRECT_BRIDGE_CLASS.get() else { return };
        if let Err(error) = env.call_static_method(
            class,
            "closeStream",
            "()V",
            &[],
        ) {
            let message = java_error(&mut env, "USB bridge close failed", error);
            log::warn!("{message}");
        }
    }
}

/// Query the connected USB Audio DAC name from UsbDirectBridge.
pub fn connected_dac_name() -> Option<String> {
    let vm = JAVA_VM.get()?;
    let mut env = vm.attach_current_thread().ok()?;
    let class = match USB_DIRECT_BRIDGE_CLASS.get() {
        Some(c) => c,
        None => {
            log::warn!("[android_usb_qt] UsbDirectBridge class is not cached yet");
            return None;
        }
    };
    let value = match env.call_static_method(
        class,
        "getConnectedDeviceName",
        "()Ljava/lang/String;",
        &[],
    ) {
        Ok(v) => v,
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::warn!("[android_usb_qt] getConnectedDeviceName failed: {e}");
            return None;
        }
    };
    let obj = match value.l() {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[android_usb_qt] getConnectedDeviceName returned invalid object: {e}");
            return None;
        }
    };
    if obj.is_null() {
        return None;
    }
    let jstr: jni::objects::JString = obj.into();
    let rust_str = env.get_string(&jstr).ok()?;
    let s: String = rust_str.into();
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

fn java_error(env: &mut jni::JNIEnv<'_>, context: &str, error: jni::errors::Error) -> String {
    let detail = if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        " (Java exception was logged)"
    } else {
        ""
    };
    format!("{context}: {error}{detail}")
}

pub fn minimize_activity() {
    if let Some(vm) = JAVA_VM.get() {
        if let Ok(mut env) = vm.attach_current_thread() {
            if let Ok(class) = env.find_class("dev/qbz/android/qt/QbzActivity") {
                let _ = env.call_static_method(class, "minimize", "()V", &[]);
            }
        }
    }
}
