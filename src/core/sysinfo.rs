//! GPU listing and executable architecture.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedGpu {
    pub name: String,
    pub pci_id: Option<String>,
    pub is_discrete: bool,
}

pub fn list_available_gpus() -> Vec<DetectedGpu> {
    let mut gpus = Vec::new();

    // Try scanning /sys/class/drm/card* to find GPUs
    // This is more reliable than just checking /dev/dri/
    let drm_path = Path::new("/sys/class/drm");
    if let Ok(entries) = std::fs::read_dir(drm_path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") && !name.contains('-') {
                let card_path = entry.path();

                // Read vendor and device IDs if available
                let device_path = card_path.join("device");
                let vendor = std::fs::read_to_string(device_path.join("vendor"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let device = std::fs::read_to_string(device_path.join("device"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();

                let pci_id = if !vendor.is_empty() && !device.is_empty() {
                    Some(format!(
                        "{}:{}",
                        vendor.replace("0x", ""),
                        device.replace("0x", "")
                    ))
                } else {
                    None
                };

                // Heuristic for discrete vs integrated
                // This is a bit simplified, but often works on Linux
                let is_discrete = pci_id.as_ref().is_some_and(|id| {
                    // NVIDIA, AMD (discrete), etc.
                    id.starts_with("10de") || id.starts_with("1002")
                });

                let gpu_name = match pci_id.as_deref() {
                    Some(id) if id.starts_with("10de") => format!("NVIDIA GPU ({})", name),
                    Some(id) if id.starts_with("1002") => format!("AMD GPU ({})", name),
                    Some(id) if id.starts_with("8086") => format!("Intel GPU ({})", name),
                    _ => format!("Unknown GPU ({})", name),
                };

                gpus.push(DetectedGpu {
                    name: gpu_name,
                    pci_id,
                    is_discrete,
                });
            }
        }
    }

    // Fallback if /sys scan failed but we have NVIDIA tools or similar
    if gpus.is_empty() && Path::new("/dev/nvidia0").exists() {
        gpus.push(DetectedGpu {
            name: "NVIDIA Discrete GPU".to_string(),
            pci_id: Some("10de:unknown".to_string()),
            is_discrete: true,
        });
    }

    gpus.sort_by_key(|g| std::cmp::Reverse(g.is_discrete));
    gpus
}

pub fn detect_exe_architecture(exe_path: &Path) -> crate::core::models::ExecutableArchitecture {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(exe_path) {
        Ok(f) => f,
        Err(_) => return crate::core::models::ExecutableArchitecture::Unknown,
    };

    let mut mz_header = [0u8; 2];
    if file.read_exact(&mut mz_header).is_err() || &mz_header != b"MZ" {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    // Offset 0x3C contains the offset to the PE header
    if file.seek(SeekFrom::Start(0x3C)).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    let mut pe_offset_buf = [0u8; 4];
    if file.read_exact(&mut pe_offset_buf).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }
    let pe_offset = u32::from_le_bytes(pe_offset_buf);

    if file.seek(SeekFrom::Start(pe_offset as u64)).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    let mut pe_signature = [0u8; 4];
    if file.read_exact(&mut pe_signature).is_err() || &pe_signature != b"PE\0\0" {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    // COFF Header starts right after PE signature
    // Machine is the first 2 bytes
    let mut machine_buf = [0u8; 2];
    if file.read_exact(&mut machine_buf).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }
    let machine = u16::from_le_bytes(machine_buf);

    match machine {
        0x014c => crate::core::models::ExecutableArchitecture::X86,
        0x8664 => crate::core::models::ExecutableArchitecture::X86_64,
        _ => crate::core::models::ExecutableArchitecture::Unknown,
    }
}
