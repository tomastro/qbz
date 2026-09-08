//! Framework-agnostic graphics environment detection.
//!
//! Detects the host display server, GPU vendors, desktop, and VM status.
//! Consumed by the diagnostics panel. (`use serde::Serialize` stays — the
//! `Environment` struct still derives it.)

use serde::Serialize;

/// Detected environment information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Environment {
    pub display_server: String,
    pub gpu_nvidia: bool,
    pub gpu_amd: bool,
    pub gpu_intel: bool,
    pub gpu_name: String,
    pub desktop: String,
    pub is_vm: bool,
}

pub fn detect_environment() -> Environment {
    let display_server = detect_display_server();
    let gpu_nvidia = is_nvidia_gpu();
    let gpu_amd = is_amd_gpu();
    let gpu_intel = is_intel_gpu();
    let gpu_name = detect_gpu_name(gpu_nvidia, gpu_amd, gpu_intel);
    let desktop = detect_desktop();
    let is_vm = is_virtual_machine();

    Environment {
        display_server,
        gpu_nvidia,
        gpu_amd,
        gpu_intel,
        gpu_name,
        desktop,
        is_vm,
    }
}

fn detect_display_server() -> String {
    let is_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland");

    if is_wayland {
        "Wayland".to_string()
    } else {
        "X11".to_string()
    }
}

fn is_nvidia_gpu() -> bool {
    std::path::Path::new("/proc/driver/nvidia/version").exists()
        || std::fs::read_to_string("/proc/modules")
            .map(|m| m.lines().any(|l| l.starts_with("nvidia")))
            .unwrap_or(false)
}

fn is_amd_gpu() -> bool {
    std::path::Path::new("/sys/module/amdgpu").exists()
        || std::fs::read_to_string("/proc/modules")
            .map(|m| m.lines().any(|l| l.starts_with("amdgpu")))
            .unwrap_or(false)
}

fn is_intel_gpu() -> bool {
    std::path::Path::new("/sys/module/i915").exists()
        || std::fs::read_to_string("/proc/modules")
            .map(|m| m.lines().any(|l| l.starts_with("i915")))
            .unwrap_or(false)
}

fn is_virtual_machine() -> bool {
    if let Ok(product) = std::fs::read_to_string("/sys/class/dmi/id/product_name") {
        let p = product.trim().to_lowercase();
        if p.contains("virtualbox")
            || p.contains("vmware")
            || p.contains("qemu")
            || p.contains("bochs")
            || p.contains("hyper-v")
        {
            return true;
        }
    }
    if let Ok(vendor) = std::fs::read_to_string("/sys/class/dmi/id/sys_vendor") {
        let v = vendor.trim().to_lowercase();
        if v.contains("innotek")
            || v.contains("vmware")
            || v.contains("qemu")
            || v.contains("xen")
            || v.contains("parallels")
        {
            return true;
        }
    }
    if let Ok(h) = std::fs::read_to_string("/sys/hypervisor/type") {
        if !h.trim().is_empty() {
            return true;
        }
    }
    false
}

pub fn detect_gpu_name(nvidia: bool, amd: bool, intel: bool) -> String {
    // Hybrid laptops have more than one of these set; join the names so
    // diagnostics surface the full picture instead of returning only the
    // first vendor matched.
    let mut parts: Vec<String> = Vec::new();
    if nvidia {
        parts.push(nvidia_name());
    }
    if amd {
        parts.push(amd_name());
    }
    if intel {
        parts.push(intel_name());
    }
    if parts.is_empty() {
        "Unknown / None detected".to_string()
    } else {
        parts.join(" + ")
    }
}

fn nvidia_name() -> String {
    if let Ok(version) = std::fs::read_to_string("/proc/driver/nvidia/version") {
        if let Some(line) = version.lines().next() {
            return format!("NVIDIA ({})", line.trim());
        }
    }
    "NVIDIA (driver loaded)".to_string()
}

fn amd_name() -> String {
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") && !name.contains('-') {
                let model_path = entry.path().join("device/product_name");
                if let Ok(model) = std::fs::read_to_string(&model_path) {
                    let model = model.trim();
                    if !model.is_empty() {
                        return format!("AMD {}", model);
                    }
                }
            }
        }
    }
    "AMD (amdgpu driver loaded)".to_string()
}

fn intel_name() -> String {
    "Intel (i915/xe driver loaded)".to_string()
}

fn detect_desktop() -> String {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let session = std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default();
    let de = std::env::var("DESKTOP_SESSION").unwrap_or_default();

    if !desktop.is_empty() {
        desktop
    } else if !session.is_empty() {
        session
    } else if !de.is_empty() {
        de
    } else {
        "Unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_name_combines_hybrid_vendors() {
        let name = detect_gpu_name(true, false, true);

        assert!(name.contains("NVIDIA"));
        assert!(name.contains("Intel"));
        assert!(name.contains(" + "));
    }
}
