//! The renderer TIER — one source of truth for "what is actually drawing, and
//! what may we offer because of it".
//!
//! # The gap this closes
//!
//! Slint resolves `use_gpu_renderer` from `select_slint_backend()` and hands
//! that ONE bool to three features (`crates/qbz/src/main.rs`):
//!
//! | consumer | line | meaning |
//! |---|---|---|
//! | `shader_scenes_available` | `:8557` | the 6 immersive shader scenes — they render BLACK off the GPU tier |
//! | `app_background_available` | `:8563` | the app-wide dynamic background |
//! | `reduce_motion` | `:8597` | `kiosk_profile \|\| !use_gpu_renderer` |
//!
//! This port had none of it. `reduce_motion` shipped as the kiosk half alone
//! (`shell_bridge::reduce_motion_at_boot`), and the immersive contract records
//! the rest as deliberately absent (D11: *"The RENDERER-TIER half is still
//! genuinely absent"*) — which is precisely why the immersive shader scenes
//! could not be offered: there was nothing to gate them on.
//!
//! # Why the truth can only come from QML
//!
//! `main::apply_renderer_preference()` runs BEFORE `QGuiApplication` and knows
//! only what we ASKED FOR. What we actually GOT is decided by QRhi when the
//! scene graph initialises, and is readable only as `GraphicsInfo.api` on a
//! live item. Asking for OpenGL on a box whose driver refuses it lands on
//! software, and nothing before the window exists can know that.
//!
//! So QML reports the resolved api in exactly one place (`Main.qml`'s renderer
//! probe) and this module latches it. Slint has the same split; its
//! `select_slint_backend()` actually initialises the backend, which is why it
//! can return the truth synchronously and we cannot.
//!
//! # Two questions, two answers — do NOT collapse them
//!
//! * *"Can THIS item draw a shader right now?"* → the item's own
//!   `GraphicsInfo.api`, which is per-window and already correct at the six
//!   call sites that use it (`RoundedImage`, `Cortinilla`, `PlaylistCollage`,
//!   `TrackInfoModal`, `ImmersiveView`, `StaticPanel`).
//! * *"Should this FEATURE be offered at all?"* → this tier.
//!
//! They look like duplicates and are not: the first is a drawing decision on
//! one item, the second is a product decision made once. Collapsing the first
//! onto this module would also make every cover in the app wait for a
//! round trip through Rust before it could pick its draw path.
//!
//! # The default is `true`, on purpose
//!
//! Until the probe reports, the tier reads GPU-capable. Both platforms were
//! measured on the GPU (OpenGL RHI on Linux, Metal on macOS, 2026-07-29), so
//! `true` is the honest prior AND it keeps today's behaviour byte for byte
//! until the probe disagrees — a `false` default would step every animation to
//! the reduce-motion cadence for the first frames of every launch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The resolved `GraphicsInfo.api`, lowercased: "opengl" | "metal" | "vulkan"
/// | "d3d11" | "d3d12" | "software" | "null" | "unknown".
static ACTIVE_API: Mutex<String> = Mutex::new(String::new());

/// Does the active backend carry the GPU tier? See the module header for why
/// this starts `true`.
static GPU_TIER: AtomicBool = AtomicBool::new(true);

/// Has a real (non-"unknown") report landed? Guards the one-shot log and lets
/// callers tell "GPU" from "not asked yet".
static RESOLVED: AtomicBool = AtomicBool::new(false);

/// The two api names that mean "no shaders": Qt's software rasteriser, and the
/// null backend the offscreen platform uses.
fn api_is_gpu(api: &str) -> bool {
    !matches!(api, "software" | "null" | "unknown" | "")
}

/// Latch the api QRhi actually gave us. Called once from the QML probe; a
/// repeat with the same value is free, and "unknown" (the pre-resolution
/// value) is ignored rather than latched as "no GPU".
///
/// Returns true when this call CHANGED the tier, so the caller can decide
/// whether anything needs republishing.
pub fn set_active_api(api: &str) -> bool {
    let api = api.trim().to_ascii_lowercase();
    if api.is_empty() || api == "unknown" {
        return false;
    }
    // Headless-verification override: the VNC/offscreen platforms report a
    // "null" probe api even when the scene graph renders through a real RHI
    // (e.g. QSG_RHI_BACKEND=vulkan offscreen), which would hide the shader
    // scenes from exactly the verification runs that need them.
    // QBZ_FORCE_GPU_TIER=1 forces the tier on; it changes nothing else (the
    // resolved api string still reports what the probe saw).
    let forced = std::env::var_os("QBZ_FORCE_GPU_TIER").is_some_and(|v| v == "1");
    let tier = api_is_gpu(&api) || forced;
    let previous_tier = GPU_TIER.swap(tier, Ordering::SeqCst);
    let first = !RESOLVED.swap(true, Ordering::SeqCst);
    {
        let mut slot = ACTIVE_API.lock().unwrap_or_else(|e| e.into_inner());
        if !first && *slot == api {
            return false;
        }
        *slot = api.clone();
    }
    // Log the REQUESTED tier beside the resolved one: a mismatch is the whole
    // reason this probe exists (asking for opengl on a box whose driver
    // refuses it lands on software), and it is what the crash sentinel needs
    // to reason about.
    log::info!(
        "[qbz-qt] renderer: requested={} resolved={api} gpu_tier={tier}",
        crate::settings_qt::pref_str("renderer", "auto"),
    );
    if !tier {
        log::warn!(
            "[qbz-qt] renderer: NO GPU tier — shader scenes and the dynamic \
             background stay hidden, reduce-motion is forced on"
        );
    }
    first || previous_tier != tier
}

/// True when the active backend can run shaders. The Qt analogue of Slint's
/// `use_gpu_renderer`.
pub fn gpu_tier() -> bool {
    GPU_TIER.load(Ordering::SeqCst)
}

/// The resolved api name, or "unknown" before the probe reports.
pub fn active_api() -> String {
    let slot = ACTIVE_API.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_empty() {
        "unknown".to_string()
    } else {
        slot.clone()
    }
}

/// The reduce-motion value, both halves composed — `kiosk || !gpu_tier`,
/// exactly the reference's expression (`main.rs:8597`).
///
/// It lives here rather than in either half's module because BOTH halves write
/// it: the kiosk live toggle and this probe. When the kiosk toggle owned the
/// property alone it simply overwrote the tier's contribution, which is the
/// bug this function exists to make impossible.
pub fn reduce_motion(kiosk: bool) -> bool {
    kiosk || !gpu_tier()
}

// ===========================================================================
// The startup auto-revert sentinel  (PARITY-DEBT #104's owed half)
// ===========================================================================
//
// Forcing a renderer is the one Settings row that can lock the user OUT of
// Settings: pick a backend this machine cannot start and the next launch dies
// before a window exists, with no way to undo the choice. Slint built a
// sentinel for exactly this (`crates/qbz/src/main.rs:7183-7210`); this port
// shipped the row WITHOUT it, which is what PARITY-DEBT #104 records as still
// owed.
//
// The shape: a file armed BEFORE the risky backend init and cleared once the
// session proves it is alive. Finding it still there at startup means the
// previous forced launch never got that far — so the pref reverts to "auto"
// and this launch uses Qt's own default.
//
// FILENAME: `qt_renderer_attempt`, deliberately NOT Slint's
// `renderer_attempt`. Both apps share `<data>/qbz/`, they can be run in either
// order, and one crashing must never revert the OTHER's renderer choice.

/// `<data>/qbz/qt_renderer_attempt` — see above for why the name differs from
/// the Slint sentinel's.
fn sentinel_path() -> Option<std::path::PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("qt_renderer_attempt"))
}

/// Arm the sentinel for a non-auto choice, just before the env that forces it.
fn arm_sentinel(choice: &str) {
    let Some(path) = sentinel_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, choice) {
        log::warn!("[qbz-qt] renderer: could not arm the startup sentinel: {e}");
    }
}

fn clear_sentinel() {
    if let Some(path) = sentinel_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Is either graphics startup sentinel currently armed? Lets the liveness
/// report stay quiet on runs where there is nothing to protect — notably the
/// offscreen smoke, which presents no frames by design and would otherwise log
/// a scary warning on every gate.
pub fn sentinel_armed() -> bool {
    sentinel_path().map(|p| p.exists()).unwrap_or(false)
        || gpu_sentinel_path().map(|p| p.exists()).unwrap_or(false)
}

/// The choice the armed sentinel was protecting, if one survived a launch.
fn armed_choice() -> Option<String> {
    std::fs::read_to_string(sentinel_path()?)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Called before GPU preflight (and again, harmlessly, at the top of
/// `apply_renderer_preference()`), before anything is forced. If a previous
/// forced launch left the sentinel armed it never reached liveness, so the
/// choice is reverted to "auto" and this launch runs on Qt's default.
///
/// Returns true when a revert happened — the caller then ignores the persisted
/// pref for this launch (it has just been rewritten to "auto" anyway).
pub fn revert_if_previous_launch_died() -> bool {
    let Some(choice) = armed_choice() else {
        return false;
    };
    clear_sentinel();
    log::warn!(
        "[qbz-qt] renderer: the previous launch forced '{choice}' and never reached \
         liveness — reverting to auto so the choice cannot lock you out of Settings"
    );
    crate::settings_qt::save_pref("renderer", serde_json::json!("auto"));
    true
}

/// Arm for a choice that genuinely FORCES a backend. The caller decides that —
/// the GPU-tier aliases all resolve to Qt's own default and force nothing, so
/// arming for them would let an unrelated crash silently reset the pref.
pub fn arm_for_choice(choice: &str) {
    arm_sentinel(choice);
    log::info!("[qbz-qt] renderer: startup sentinel armed for '{choice}'");
}

/// Drop any sentinel left over from a previous configuration — this launch
/// forces nothing, so there is nothing to protect and nothing to revert.
pub fn clear_stale_sentinel() {
    clear_sentinel();
}

/// Disarm on proof of LIVENESS — frames actually rendered over a window of
/// time, reported by the QML watchdog. Once-guarded, so the repeat calls a
/// binding might make are free.
pub fn disarm_on_liveness(frames: i32) {
    static DISARMED: AtomicBool = AtomicBool::new(false);
    if DISARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    // Only claim a disarm when something WAS armed. Saying "sentinel disarmed"
    // on every ordinary launch trains the reader to skim past the line, which
    // is precisely the run where it carries information.
    let renderer_armed = sentinel_path().map(|p| p.exists()).unwrap_or(false);
    let gpu_armed = gpu_sentinel_path().map(|p| p.exists()).unwrap_or(false);
    if renderer_armed || gpu_armed {
        let protected = match (renderer_armed, gpu_armed) {
            (true, true) => "renderer + GPU",
            (true, false) => "renderer",
            (false, true) => "GPU",
            (false, false) => unreachable!(),
        };
        log::info!("[qbz-qt] {protected}: startup sentinel disarmed ({frames} frames rendered)");
        clear_sentinel();
        clear_gpu_sentinel();
    } else {
        log::debug!("[qbz-qt] graphics: liveness confirmed ({frames} frames), nothing armed");
    }
}

// ===========================================================================
// Preferred GPU  (PARITY-DEBT #83)
// ===========================================================================
//
// Scoping: `qbz-nix-docs/qt-frontend/2026-08-02-gpu-selection-scoping.md`.
// Qt exposes no public QRhi adapter-selection API. On Linux its supported
// escape hatch is `QT_VK_PHYSICAL_DEVICE_INDEX`, read when the Vulkan QRhi is
// created. The index is process-local and MUST be resolved afresh at boot.
//
// | platform | selectable | how |
// |---|---|---|
// | Linux | YES | Vulkan + `QT_VK_PHYSICAL_DEVICE_INDEX` |
// | Windows D3D | yes, when that port lands | `QT_D3D_ADAPTER_INDEX` |
// | macOS Metal | **NO** | QRhi hardcodes `MTLCreateSystemDefaultDevice` |
//
// WHY A RAW VULKAN PROBE IS WRONG. On the owner's Intel + NVIDIA hybrid, an
// independently-created VkInstance enumerated Intel=0/NVIDIA=1 while Qt's own
// QVulkanInstance enumerated NVIDIA=0/Intel=1 in the same environment. Hybrid
// implicit layers can react to instance/platform details; "both call
// vkEnumeratePhysicalDevices" does not make the indices interchangeable. The
// old code fed the raw index to Qt, selected the opposite GPU, and Wayland
// killed the connection when cross-device dmabuf import failed.
//
// `cxx/qt_vulkan_probe.cpp` enumerates the actual QVulkanDefaultInstance that
// Qt Quick later gives QRhi. It runs after QGuiApplication installs the
// platform integration and before the first QQuickWindow exists. Preferences
// store a stable Vulkan UUID beside the legacy display name; an index is never
// persisted.

/// One physical GPU in this launch's Qt-owned Vulkan instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuInfo {
    /// Exact `QT_VK_PHYSICAL_DEVICE_INDEX` for this process only.
    pub index: u32,
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub identity: String,
    pub discrete: bool,
}

impl GpuInfo {
    /// The dropdown label, in the reference's shape
    /// (`"<name> (discrete|integrated)"`, `crates/qbz/src/main.rs:7427-7438`).
    pub fn label(&self) -> String {
        let class = if self.discrete {
            qbz_i18n::t("discrete")
        } else {
            qbz_i18n::t("integrated")
        };
        format!("{} ({})", self.name, class)
    }
}

#[derive(serde::Deserialize)]
struct QtGpuJson {
    index: u32,
    name: String,
    vendor: u32,
    device: u32,
    #[serde(rename = "type")]
    device_type: u32,
    #[serde(default)]
    uuid: String,
}

unsafe extern "C" {
    fn qbz_qt_vulkan_devices_json() -> *const std::ffi::c_char;
    fn qbz_qt_vulkan_preflight_window() -> i32;
    #[cfg(target_os = "linux")]
    fn qbz_qt_auto_preflight_window() -> i32;
}

#[cfg(target_os = "linux")]
#[path = "renderer_preflight_linux.rs"]
pub mod auto_preflight;

const GPU_PREFLIGHT_CHILD_ENV: &str = "QBZ_INTERNAL_GPU_PREFLIGHT_CHILD";
const GPU_PREFLIGHT_IDENTITY_ENV: &str = "QBZ_INTERNAL_GPU_PREFLIGHT_IDENTITY";
const GPU_PREFLIGHT_NAME_ENV: &str = "QBZ_INTERNAL_GPU_PREFLIGHT_NAME";
const GPU_PREFLIGHT_NOT_FOUND: i32 = 41;
const GPU_PREFLIGHT_NOT_SELECTABLE: i32 = 42;
const GPU_PREFLIGHT_PARENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// The parent only authorizes a forced GPU after a disposable child proved
/// that this exact identity can present Qt Quick frames to the active
/// compositor. Kept in-process: a success from an older topology/driver must
/// never become a permanent allow-list.
static PREFLIGHT_APPROVED_GPU: Mutex<Option<String>> = Mutex::new(None);
static PREFLIGHT_FALLBACK_NOTICE: AtomicBool = AtomicBool::new(false);

fn stable_gpu_identity(raw: &QtGpuJson) -> String {
    let uuid = raw.uuid.trim().to_ascii_lowercase();
    if uuid.len() == 32
        && uuid.bytes().all(|b| b.is_ascii_hexdigit())
        && uuid.bytes().any(|b| b != b'0')
    {
        format!("vk:{uuid}")
    } else {
        format!(
            "pci:{:04x}:{:04x}:{}",
            raw.vendor,
            raw.device,
            raw.name.trim().to_ascii_lowercase()
        )
    }
}

/// Enumerate through Qt's own default Vulkan instance. The C++ side refuses to
/// run before QGuiApplication exists and returns `[]` on unsupported or broken
/// Vulkan setups, so inventory can never become a startup gate.
fn enumerate_qt_gpus() -> Vec<GpuInfo> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    // SAFETY: the C++ function returns a pointer into a process-static
    // QByteArray. This call runs on the GUI thread and copies the bytes before
    // returning.
    let ptr = unsafe { qbz_qt_vulkan_devices_json() };
    if ptr.is_null() {
        return Vec::new();
    }
    let json = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy();
    let raw: Vec<QtGpuJson> = match serde_json::from_str(&json) {
        Ok(list) => list,
        Err(e) => {
            log::warn!("[qbz-qt] gpu: Qt Vulkan inventory was invalid: {e}");
            return Vec::new();
        }
    };

    gpu_inventory_from_raw(raw)
}

fn gpu_inventory_from_raw(raw: Vec<QtGpuJson>) -> Vec<GpuInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for gpu in raw {
        let name = gpu.name.trim().to_string();
        // VkPhysicalDeviceType::CPU is lavapipe/llvmpipe, not another GPU the
        // Preferred-GPU control should offer.
        if name.is_empty() || gpu.device_type == 4 || !seen.insert(gpu.index) {
            continue;
        }
        out.push(GpuInfo {
            index: gpu.index,
            name,
            vendor_id: gpu.vendor,
            device_id: gpu.device,
            identity: stable_gpu_identity(&gpu),
            discrete: gpu.device_type == 2,
        });
    }
    out.sort_by_key(|gpu| gpu.index);
    out
}

/// Real hardware adapters in the exact order this process's Qt Vulkan backend
/// sees them. Empty is supported: Auto can still use OpenGL or software.
pub fn gpus() -> &'static [GpuInfo] {
    static GPUS: std::sync::OnceLock<Vec<GpuInfo>> = std::sync::OnceLock::new();
    GPUS.get_or_init(|| {
        let list = enumerate_qt_gpus();
        if list.is_empty() {
            log::info!(
                "[qbz-qt] gpu: Qt exposed no selectable Vulkan hardware — Auto remains in control"
            );
        } else {
            for gpu in &list {
                log::info!(
                    "[qbz-qt] gpu: Qt device {} '{}' ({}, vendor={:04x} device={:04x})",
                    gpu.index,
                    gpu.name,
                    if gpu.discrete {
                        "discrete"
                    } else {
                        "integrated"
                    },
                    gpu.vendor_id,
                    gpu.device_id,
                );
            }
        }
        list
    })
}

/// True only in the disposable subprocess spawned by
/// [`preflight_saved_gpu_at_boot`]. This is checked at the top of `main`,
/// before single-instance ownership, navigation sentinels, audio or runtime
/// construction.
pub fn gpu_preflight_child_requested() -> bool {
    std::env::var_os(GPU_PREFLIGHT_CHILD_ENV)
        .is_some_and(|value| value.to_string_lossy().trim() == "1")
}

/// Run the child half after its minimal QGuiApplication exists. The C++ helper
/// creates a transparent 2x2 QQuickWindow and keeps swapping long enough for
/// Wayland to accept or fatally reject its DMA-BUFs.
pub fn run_gpu_preflight_child() -> i32 {
    let identity = std::env::var(GPU_PREFLIGHT_IDENTITY_ENV).unwrap_or_default();
    let name = std::env::var(GPU_PREFLIGHT_NAME_ENV).unwrap_or_default();
    std::env::remove_var(GPU_PREFLIGHT_CHILD_ENV);
    std::env::remove_var(GPU_PREFLIGHT_IDENTITY_ENV);
    std::env::remove_var(GPU_PREFLIGHT_NAME_ENV);

    if !cfg!(target_os = "linux") {
        return GPU_PREFLIGHT_NOT_SELECTABLE;
    }
    let list = gpus();
    if list.len() < 2 {
        log::info!("[qbz-qt] gpu preflight: fewer than two selectable devices -> Auto");
        return GPU_PREFLIGHT_NOT_SELECTABLE;
    }
    let Some(gpu) = resolve_gpu_in(list, &name, &identity) else {
        log::warn!("[qbz-qt] gpu preflight: requested device '{name}' ({identity}) is not present");
        return GPU_PREFLIGHT_NOT_FOUND;
    };

    std::env::set_var("QSG_RHI_BACKEND", "vulkan");
    std::env::set_var("QT_VK_PHYSICAL_DEVICE_INDEX", gpu.index.to_string());
    log::info!(
        "[qbz-qt] gpu preflight: probing '{}' ({}) at Qt index {}",
        gpu.name,
        gpu.identity,
        gpu.index
    );
    // SAFETY: the helper is called on the child's GUI thread, after its sole
    // QGuiApplication and before any other QQuickWindow. It owns all temporary
    // Qt objects and returns only after destroying the probe window.
    let result = unsafe { qbz_qt_vulkan_preflight_window() };
    if result == 0 {
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "QBZ_GPU_PREFLIGHT_OK={}", gpu.identity);
        let _ = stdout.flush();
        log::info!(
            "[qbz-qt] gpu preflight: '{}' presented frames successfully",
            gpu.name
        );
    } else {
        log::warn!(
            "[qbz-qt] gpu preflight: '{}' did not prove presentation (code {result})",
            gpu.name
        );
    }
    result
}

/// Resolve a persisted GPU pair to a device. Stable identity wins; without one,
/// accept the model name older Qt builds store and the legacy class keys.
///
/// `None` = Auto, or a value that matches no present device — which is exactly
/// the case that used to let you select a GPU that does not exist.
fn unique_gpu_matching(
    list: &[GpuInfo],
    mut predicate: impl FnMut(&GpuInfo) -> bool,
) -> Option<&GpuInfo> {
    let mut matches = list.iter().filter(|gpu| predicate(gpu));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn resolve_gpu_in<'a>(list: &'a [GpuInfo], pref: &str, identity: &str) -> Option<&'a GpuInfo> {
    let pref = pref.trim();
    if pref.is_empty() || pref == "auto" {
        return None;
    }
    let identity = identity.trim();
    if !identity.is_empty() {
        if let Some(hit) = unique_gpu_matching(list, |gpu| gpu.identity == identity) {
            // A downgraded/older build may update `gpu_power` without knowing
            // about `gpu_identity`. If the readable key clearly names another
            // device (or class), treat the identity as stale. Otherwise the
            // UUID remains authoritative across driver model-name changes.
            let names_another = list.iter().any(|gpu| gpu.name == pref) && hit.name != pref;
            let class_disagrees =
                matches!(pref, "discrete" | "integrated") && (hit.discrete != (pref == "discrete"));
            if !names_another && !class_disagrees {
                return Some(hit);
            }
        } else {
            // Never fall back from a missing UUID to a same-model device: that
            // could be the other of two identical eGPUs.
            return None;
        }
    }
    if let Some(hit) = unique_gpu_matching(list, |gpu| gpu.name == pref) {
        return Some(hit);
    }
    // Legacy class keys — resolve to a device that ACTUALLY EXISTS, or to
    // nothing. "discrete" on a single-GPU laptop resolves to None, and that is
    // the point.
    match pref {
        "discrete" => unique_gpu_matching(list, |gpu| gpu.discrete),
        "integrated" => unique_gpu_matching(list, |gpu| !gpu.discrete),
        _ => None,
    }
}

pub fn resolve_saved_gpu() -> Option<&'static GpuInfo> {
    resolve_gpu_in(
        gpus(),
        &crate::settings_qt::pref_str("gpu_power", "auto"),
        &crate::settings_qt::pref_str("gpu_identity", ""),
    )
}

/// Can this machine actually honour a Preferred-GPU choice?
///
/// Two conditions, and BOTH are the owner's point that a control must never
/// offer what does not exist:
/// * the platform can select at all — false on macOS, where QRhi hardcodes
///   `MTLCreateSystemDefaultDevice` (no env, no API);
/// * there is more than one GPU to choose BETWEEN. On a single-GPU box the row
///   is a dropdown with one real answer, so it hides.
pub fn gpu_selectable() -> bool {
    cfg!(target_os = "linux") && gpus().len() > 1
}

// A GPU override can fail before QML exists (bad adapter, presentation, or a
// cross-device dmabuf import). Keep this separate from `qt_renderer_attempt`
// so a device failure never rewrites the renderer row and vice versa.
fn gpu_sentinel_path() -> Option<std::path::PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("qt_gpu_attempt"))
}

fn clear_gpu_sentinel() {
    if let Some(path) = gpu_sentinel_path() {
        let _ = std::fs::remove_file(path);
    }
}

fn armed_gpu_choice() -> Option<String> {
    std::fs::read_to_string(gpu_sentinel_path()?)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn arm_gpu_sentinel(gpu: &GpuInfo) {
    let Some(path) = gpu_sentinel_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let marker = format!("{}|{}|qt-index={}", gpu.identity, gpu.name, gpu.index);
    match std::fs::write(&path, marker) {
        Ok(()) => log::info!("[qbz-qt] gpu: startup sentinel armed for '{}'", gpu.name),
        Err(e) => log::warn!("[qbz-qt] gpu: could not arm the startup sentinel: {e}"),
    }
}

fn revert_if_previous_gpu_launch_died() -> bool {
    let Some(choice) = armed_gpu_choice() else {
        return false;
    };
    clear_gpu_sentinel();
    log::warn!(
        "[qbz-qt] gpu: the previous launch forced '{choice}' and never reached liveness — \
         reverting Preferred GPU to Auto"
    );
    crate::settings_qt::save_gpu_preference(None);
    PREFLIGHT_FALLBACK_NOTICE.store(true, Ordering::SeqCst);
    true
}

fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.to_string_lossy().trim().is_empty())
}

fn gpu_override_env() -> Option<&'static str> {
    [
        "QT_VK_PHYSICAL_DEVICE_INDEX",
        "DRI_PRIME",
        "__NV_PRIME_RENDER_OFFLOAD",
        "__VK_LAYER_NV_optimus",
        "MESA_VK_DEVICE_SELECT",
        "VK_DRIVER_FILES",
        "VK_ICD_FILENAMES",
    ]
    .into_iter()
    .find(|name| nonempty_env(name))
}

/// Mirror `main::apply_renderer_preference` just far enough to know whether a
/// saved GPU would be ignored anyway. A software/OpenGL renderer choice wins
/// over Preferred GPU, so probing Vulkan in that case would be both wasteful
/// and misleading.
fn renderer_blocks_gpu_preflight() -> bool {
    if nonempty_env("QSG_RHI_BACKEND") || nonempty_env("QT_QUICK_BACKEND") {
        return true;
    }
    let from_env = std::env::var("QBZ_RENDERER")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty() && value != "auto");
    let choice = from_env.unwrap_or_else(|| crate::settings_qt::pref_str("renderer", "auto"));
    renderer_choice_blocks_gpu(&choice)
}

fn renderer_choice_blocks_gpu(choice: &str) -> bool {
    matches!(
        choice.trim().to_ascii_lowercase().as_str(),
        "software" | "cpu" | "soft" | "gl" | "gles" | "femtovg"
    )
}

fn preflight_output_tail(output: &std::process::Output) -> String {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(6)..].join(" | ");
    tail.chars().take(1200).collect()
}

fn approved_identity_from_stdout(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .find_map(|line| line.strip_prefix("QBZ_GPU_PREFLIGHT_OK="))
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .map(str::to_string)
}

fn approved_identity_from(output: &std::process::Output) -> Option<String> {
    approved_identity_from_stdout(&output.stdout)
}

fn spawn_gpu_preflight(name: &str, identity: &str) -> Result<(std::process::Output, bool), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve current executable: {error}"))?;
    let mut command = std::process::Command::new(executable);
    command
        .env(GPU_PREFLIGHT_CHILD_ENV, "1")
        .env(GPU_PREFLIGHT_IDENTITY_ENV, identity)
        .env(GPU_PREFLIGHT_NAME_ENV, name);
    run_preflight_child(&mut command, GPU_PREFLIGHT_PARENT_TIMEOUT)
}

/// Drain into anonymous temporary files rather than undrained pipes: verbose
/// driver diagnostics must not block the child before it can report success.
/// No worker survives cancellation; kill + wait reaps the one child we own.
fn run_preflight_child(
    command: &mut std::process::Command,
    budget: std::time::Duration,
) -> Result<(std::process::Output, bool), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut stdout = tempfile::tempfile().map_err(|e| format!("child stdout: {e}"))?;
    let mut stderr = tempfile::tempfile().map_err(|e| format!("child stderr: {e}"))?;
    let out = stdout.try_clone().map_err(|e| e.to_string())?;
    let err = stderr.try_clone().map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    let mut child = command
        // An activation token is single-use. The invisible probe must never
        // consume the token intended to focus the real main window.
        .env_remove("XDG_ACTIVATION_TOKEN")
        .env_remove("DESKTOP_STARTUP_ID")
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|error| format!("cannot spawn child: {error}"))?;

    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < budget => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot monitor child: {error}"));
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("cannot collect child result: {error}"))?;
    fn tail(file: &mut std::fs::File) -> Result<Vec<u8>, String> {
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(65536)))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(65536)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
    let output = std::process::Output {
        status,
        stdout: tail(&mut stdout)?,
        stderr: tail(&mut stderr)?,
    };
    Ok((output, timed_out))
}

fn set_preflight_approved(identity: Option<String>) {
    let mut slot = PREFLIGHT_APPROVED_GPU
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *slot = identity;
}

fn preflight_approved(identity: &str) -> bool {
    PREFLIGHT_APPROVED_GPU
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_deref()
        == Some(identity)
}

fn reject_saved_gpu(reason: &str) {
    set_preflight_approved(None);
    clear_gpu_sentinel();
    crate::settings_qt::save_gpu_preference(None);
    PREFLIGHT_FALLBACK_NOTICE.store(true, Ordering::SeqCst);
    log::warn!(
        "[qbz-qt] gpu preflight: {reason}; Preferred GPU reverted to Auto before main startup"
    );
}

/// One-shot UI notice consumed after QbzShell registers its Qt-thread hop.
pub fn take_preflight_fallback_notice() -> bool {
    PREFLIGHT_FALLBACK_NOTICE.swap(false, Ordering::SeqCst)
}

/// Validate an explicit saved GPU in a disposable process before this process
/// constructs its runtime, audio thread or QGuiApplication. A fatal Wayland
/// DMA-BUF import error can then kill only the child; the parent persists Auto
/// and continues through the normal safe startup.
pub fn preflight_saved_gpu_at_boot() {
    set_preflight_approved(None);
    if !cfg!(target_os = "linux") {
        return;
    }
    // Renderer recovery may turn a previously forced software/OpenGL choice
    // into Auto. Do it before deciding whether a Vulkan GPU probe is relevant.
    let _ = revert_if_previous_launch_died();
    if revert_if_previous_gpu_launch_died() {
        return;
    }
    if let Some(name) = gpu_override_env() {
        log::info!(
            "[qbz-qt] gpu preflight: {name} is explicitly set; leaving external policy untouched"
        );
        return;
    }
    if renderer_blocks_gpu_preflight() {
        log::info!(
            "[qbz-qt] gpu preflight: explicit software/OpenGL renderer wins; no GPU probe needed"
        );
        return;
    }

    let name = crate::settings_qt::pref_str("gpu_power", "auto");
    let identity = crate::settings_qt::pref_str("gpu_identity", "");
    if name.trim().is_empty() || name == "auto" {
        return;
    }

    log::info!(
        "[qbz-qt] gpu preflight: validating saved device '{name}' ({identity}) in an isolated child"
    );
    let (output, timed_out) = match spawn_gpu_preflight(&name, &identity) {
        Ok(result) => result,
        Err(error) => {
            reject_saved_gpu(&error);
            return;
        }
    };
    if timed_out {
        reject_saved_gpu("child exceeded the 8-second watchdog");
        return;
    }
    if output.status.success() {
        if let Some(approved) = approved_identity_from(&output) {
            log::info!(
                "[qbz-qt] gpu preflight: presentation approved for {approved}; continuing main startup"
            );
            set_preflight_approved(Some(approved));
            return;
        }
        reject_saved_gpu("child exited successfully without an identity proof");
        return;
    }

    let code = output.status.code();
    let detail = preflight_output_tail(&output);
    if code == Some(GPU_PREFLIGHT_NOT_SELECTABLE) {
        reject_saved_gpu("this environment exposes fewer than two selectable GPUs");
    } else if code == Some(GPU_PREFLIGHT_NOT_FOUND) {
        reject_saved_gpu("the saved GPU identity is no longer present");
    } else if detail.is_empty() {
        reject_saved_gpu(&format!("child failed with status {:?}", output.status));
    } else {
        reject_saved_gpu(&format!(
            "child failed with status {:?}; last output: {detail}",
            output.status
        ));
    }
}

/// Apply Preferred GPU after QGuiApplication installs the platform integration
/// and before QQmlApplicationEngine can construct a QQuickWindow. This is the
/// only interval where we can enumerate Qt's actual VkInstance and still set
/// the environment QRhi consumes.
pub fn apply_gpu_preference() {
    let reverted = revert_if_previous_gpu_launch_died();

    // An explicit env always wins — someone who exported one of these meant it,
    // including vendor/ICD selectors that can change the inventory itself.
    if let Some(taken) = gpu_override_env() {
        log::info!("[qbz-qt] gpu: {taken} already set; leaving the choice to it");
        return;
    }
    // `apply_renderer_preference()` runs FIRST and may have forced a backend.
    // A GPU pick needs Vulkan (see below), so it cannot also honour an explicit
    // `software`/`opengl` choice — the more specific, explicitly-chosen
    // RENDERER wins and this says so rather than quietly overriding it.
    if let Some(forced) = ["QSG_RHI_BACKEND", "QT_QUICK_BACKEND"]
        .into_iter()
        .find(|name| nonempty_env(name))
    {
        log::info!(
            "[qbz-qt] gpu: the renderer row already forced {forced:?}; a GPU pick needs \
             Vulkan, so the explicit renderer choice wins"
        );
        return;
    }

    let pref = if reverted {
        "auto".to_string()
    } else {
        crate::settings_qt::pref_str("gpu_power", "auto")
    };
    let identity = if reverted {
        String::new()
    } else {
        crate::settings_qt::pref_str("gpu_identity", "")
    };
    if pref.trim().is_empty() || pref == "auto" {
        clear_gpu_sentinel();
        log::info!("[qbz-qt] gpu: Auto -> Qt/driver environment default (no override)");
        return;
    }
    if !gpu_selectable() {
        clear_gpu_sentinel();
        log::info!(
            "[qbz-qt] gpu: '{pref}' requested, but this environment exposes fewer than two \
             selectable GPUs -> Auto"
        );
        return;
    }
    let Some(gpu) = resolve_gpu_in(gpus(), &pref, &identity) else {
        clear_gpu_sentinel();
        log::warn!(
            "[qbz-qt] gpu: saved device '{pref}' ({identity}) is not present in Qt's inventory -> Auto"
        );
        return;
    };

    // A saved GPU is never allowed to reach the real window merely because it
    // exists. The disposable child must have presented Qt Quick frames from
    // this exact stable identity in the current process launch first.
    if !preflight_approved(&gpu.identity) {
        reject_saved_gpu(&format!(
            "no successful presentation proof exists for '{}' ({})",
            gpu.name, gpu.identity
        ));
        return;
    }

    // Migrate model/class-only preferences once. The model stays for readable
    // backward compatibility; the UUID becomes authoritative.
    if pref != gpu.name || identity != gpu.identity {
        crate::settings_qt::save_gpu_preference(Some(gpu));
        log::info!("[qbz-qt] gpu: migrated Preferred GPU to stable identity");
    }

    // THE VULKAN COUPLING, and why it is the only honest route.
    //
    // MEASURED 2026-08-11 on the Intel-Arc + RTX-4070 hybrid (Wayland,
    // proprietary NVIDIA), with a 6-line QML scene carrying a MultiEffect:
    //
    // | env                | result                                        |
    // |--------------------|-----------------------------------------------|
    // | (none)             | Intel Arc, OpenGL 4.6 desktop — healthy        |
    // | `DRI_PRIME=1`      | **Mesa llvmpipe** — CPU software rendering     |
    // | NVIDIA PRIME pair  | RTX 4070, **OpenGL ES 3.2**                    |
    // | Vulkan + index     | the chosen device, ZERO shader errors          |
    //
    // The GLES context is fatal: Qt's baked `.qsb` shaders carry SPIR-V, GLSL
    // 440 (desktop) and MSL — no GLES variants — so every effect dies with
    // "No GLSL shader code found (versions tried: 320, 310, 300, 100)". That
    // is what killed the NPB Large visualiser when this row first shipped the
    // PRIME envs. `QT_OPENGL=desktop` does not rescue it (measured).
    //
    // Vulkan has none of that problem: the same .qsb already carries SPIR-V.
    // Every explicit device — including Qt index 0 — forces Vulkan, otherwise
    // an OpenGL default could silently use another adapter. The index below
    // came from Qt's own persistent VkInstance for this launch.
    arm_gpu_sentinel(gpu);
    std::env::set_var("QSG_RHI_BACKEND", "vulkan");
    std::env::set_var("QT_VK_PHYSICAL_DEVICE_INDEX", gpu.index.to_string());
    log::info!(
        "[qbz-qt] gpu: '{}' ({}) -> QSG_RHI_BACKEND=vulkan QT_VK_PHYSICAL_DEVICE_INDEX={}",
        gpu.name,
        gpu.identity,
        gpu.index
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only these two api names may drop the tier. Getting this list wrong is
    /// silent: the app just quietly stops offering the shader scenes.
    #[test]
    fn only_software_and_null_lose_the_gpu_tier() {
        for api in ["opengl", "metal", "vulkan", "d3d11", "d3d12"] {
            assert!(api_is_gpu(api), "{api} should carry the GPU tier");
        }
        for api in ["software", "null"] {
            assert!(!api_is_gpu(api), "{api} must NOT carry the GPU tier");
        }
    }

    /// "unknown" is the value GraphicsInfo reports BEFORE the scene graph
    /// resolves. Latching it as "no GPU" would hide the shader scenes on every
    /// launch until something happened to re-report.
    #[test]
    fn unknown_is_not_a_verdict() {
        assert!(!api_is_gpu("unknown"));
        assert!(!api_is_gpu(""));
        // …and it must not be latchable.
        assert!(!set_active_api("unknown"));
        assert!(!set_active_api("   "));
    }

    /// `GPU_TIER` is a process global and cargo runs tests in PARALLEL, so any
    /// test that MUTATES it takes this first. (Three tests in
    /// `dac_wizard_qt` raced exactly this way on 2026-08-11.)
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Both halves compose; neither can erase the other.
    #[test]
    fn reduce_motion_composes_both_halves() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        GPU_TIER.store(true, Ordering::SeqCst);
        assert!(!reduce_motion(false), "GPU + desktop = full motion");
        assert!(reduce_motion(true), "kiosk forces it on regardless of tier");
        GPU_TIER.store(false, Ordering::SeqCst);
        assert!(
            reduce_motion(false),
            "a software tier forces it on by itself"
        );
        assert!(reduce_motion(true));
        GPU_TIER.store(true, Ordering::SeqCst);
    }

    fn raw_gpu(index: u32, name: &str, device_type: u32, uuid: &str) -> QtGpuJson {
        QtGpuJson {
            index,
            name: name.to_string(),
            vendor: 0x10de,
            device: 0x2860,
            device_type,
            uuid: uuid.to_string(),
        }
    }

    #[test]
    fn qt_inventory_keeps_qt_indices_and_filters_cpu_adapters() {
        let list = gpu_inventory_from_raw(vec![
            raw_gpu(2, "lavapipe", 4, ""),
            raw_gpu(1, "Intel Arc", 1, "8680557d080000000002000000000000"),
            raw_gpu(0, "NVIDIA RTX", 2, "4e8f268581e1cbdcac12c7c15fa6624e"),
        ]);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].index, 0);
        assert_eq!(list[0].name, "NVIDIA RTX");
        assert_eq!(list[1].index, 1);
        assert_eq!(list[1].name, "Intel Arc");
    }

    #[test]
    fn stable_identity_wins_over_duplicate_model_names_and_index_changes() {
        let first = GpuInfo {
            index: 1,
            name: "Same GPU".into(),
            vendor_id: 1,
            device_id: 2,
            identity: "vk:first".into(),
            discrete: true,
        };
        let wanted = GpuInfo {
            index: 0,
            name: "Same GPU".into(),
            vendor_id: 1,
            device_id: 2,
            identity: "vk:wanted".into(),
            discrete: true,
        };
        let list = vec![first, wanted];
        assert_eq!(
            resolve_gpu_in(&list, "Same GPU", "vk:wanted").map(|gpu| gpu.index),
            Some(0)
        );
        assert!(resolve_gpu_in(&list, "Same GPU", "vk:missing").is_none());
        assert!(
            resolve_gpu_in(&list, "Same GPU", "").is_none(),
            "a legacy name may not guess between identical devices"
        );
        assert!(
            resolve_gpu_in(&list, "discrete", "").is_none(),
            "a legacy class may not guess between multiple discrete GPUs"
        );
    }

    #[test]
    fn newer_readable_choice_can_replace_identity_left_by_an_older_build() {
        let integrated = GpuInfo {
            index: 0,
            name: "Integrated".into(),
            vendor_id: 1,
            device_id: 1,
            identity: "vk:integrated".into(),
            discrete: false,
        };
        let discrete = GpuInfo {
            index: 1,
            name: "Discrete".into(),
            vendor_id: 2,
            device_id: 2,
            identity: "vk:discrete".into(),
            discrete: true,
        };
        let list = vec![integrated, discrete];
        assert_eq!(
            resolve_gpu_in(&list, "Discrete", "vk:integrated").map(|gpu| gpu.index),
            Some(1)
        );
    }

    #[test]
    fn uuid_fallback_is_deterministic_when_driver_exposes_none() {
        let raw = raw_gpu(0, " Example GPU ", 2, "00000000000000000000000000000000");
        assert_eq!(stable_gpu_identity(&raw), "pci:10de:2860:example gpu");
    }

    #[test]
    fn child_success_marker_requires_an_exact_nonempty_identity() {
        assert_eq!(
            approved_identity_from_stdout(
                b"noise\nQBZ_GPU_PREFLIGHT_OK=vk:device-uuid\nmore noise\n"
            )
            .as_deref(),
            Some("vk:device-uuid")
        );
        assert!(approved_identity_from_stdout(b"QBZ_GPU_PREFLIGHT_OK=   \n").is_none());
        assert!(approved_identity_from_stdout(b"prefix QBZ_GPU_PREFLIGHT_OK=vk:x\n").is_none());
    }

    #[test]
    fn only_renderer_choices_that_preclude_vulkan_block_the_probe() {
        for choice in ["software", "cpu", "soft", "gl", "gles", "femtovg"] {
            assert!(renderer_choice_blocks_gpu(choice), "{choice}");
        }
        for choice in ["auto", "wgpu", "gpu", "hardware", "hw", "unknown"] {
            assert!(!renderer_choice_blocks_gpu(choice), "{choice}");
        }
    }
}
