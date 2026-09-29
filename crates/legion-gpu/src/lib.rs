//! GPU detection (PCI/DRM sysfs) and NVIDIA monitoring (NVML).
//!
//! * Display controllers are enumerated from `/sys/bus/pci/devices/*/class`
//!   (class `0x03xxxx`).
//! * Names come from `/proc/driver/nvidia/gpus/*/information` (NVIDIA),
//!   NVML, or the system `pci.ids` database.
//! * The graphics mode is inferred from which GPU's DRM card has the
//!   internal panel (eDP/LVDS) connected. This is read from the cached DRM
//!   connector `status` attribute and does not trigger a probe.
//! * NVML is only queried when the dGPU is runtime-active, because
//!   initialising NVML wakes a suspended GPU and costs battery. Callers can
//!   force it with [`GpuOptions::wake_gpu`].
//!
//! NVML is used for **reading only**; no clocks or power limits are changed.

use std::path::Path;

use legion_core::model::{GpuDevice, GpuInfo, GpuMode, GpuVendor, NvmlStatus};
use legion_core::sysfs::{self, SysRoot};

const PCI_DEVICES: &str = "/sys/bus/pci/devices";
const DRM_CLASS: &str = "/sys/class/drm";
const PCI_IDS: [&str; 3] = [
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/usr/share/pci.ids",
];

/// Options controlling GPU probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuOptions {
    /// Query NVML at all (disabled for fixtures/tests).
    pub use_nvml: bool,
    /// Query NVML even if the GPU is runtime-suspended (wakes it up).
    pub wake_gpu: bool,
}

impl Default for GpuOptions {
    fn default() -> Self {
        Self {
            use_nvml: true,
            wake_gpu: false,
        }
    }
}

fn vendor_of(id: &str) -> GpuVendor {
    match id {
        "10de" => GpuVendor::Nvidia,
        "8086" => GpuVendor::Intel,
        "1002" => GpuVendor::Amd,
        _ => GpuVendor::Other,
    }
}

fn vendor_prefix(v: GpuVendor) -> &'static str {
    match v {
        GpuVendor::Nvidia => "NVIDIA",
        GpuVendor::Intel => "Intel",
        GpuVendor::Amd => "AMD",
        GpuVendor::Other => "",
    }
}

fn strip_hex(s: &str) -> String {
    s.trim().trim_start_matches("0x").to_ascii_lowercase()
}

/// Looks up a device name in `pci.ids` text.
pub fn lookup_pci_ids(text: &str, vendor: &str, device: &str) -> Option<String> {
    let mut in_vendor = false;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return None;
            }
            in_vendor = line
                .get(..4)
                .is_some_and(|v| v.eq_ignore_ascii_case(vendor));
            continue;
        }
        if in_vendor && !line.starts_with("\t\t") {
            let l = &line[1..];
            if l.get(..4).is_some_and(|d| d.eq_ignore_ascii_case(device)) {
                return Some(l[4..].trim().to_owned());
            }
        }
    }
    None
}

fn pci_ids_name(root: &SysRoot, vendor: &str, device: &str) -> Option<String> {
    PCI_IDS
        .iter()
        .find_map(|p| std::fs::read_to_string(root.path(p)).ok())
        .and_then(|t| lookup_pci_ids(&t, vendor, device))
}

/// Parses the driver version out of `/proc/driver/nvidia/version`.
pub fn parse_nvidia_version(text: &str) -> Option<String> {
    let first = text.lines().next()?;
    first
        .split_whitespace()
        .find(|t| t.contains('.') && t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .map(str::to_owned)
}

/// Parses `Model:` from `/proc/driver/nvidia/gpus/<addr>/information`.
pub fn parse_nvidia_information(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("Model:"))
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty())
}

fn pci_address_of(path: &Path) -> Option<String> {
    std::fs::canonicalize(path)
        .ok()?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Internal-panel status per PCI address: `Some(true)` if a connected
/// eDP/LVDS/DSI connector exists on that GPU's DRM card.
fn internal_panel(root: &SysRoot, pci: &str) -> Option<bool> {
    let entries = root.list(DRM_CLASS);
    let cards: Vec<&String> = entries
        .iter()
        .filter(|n| n.starts_with("card") && !n.contains('-'))
        .filter(|n| {
            pci_address_of(&root.path(format!("{DRM_CLASS}/{n}/device"))).as_deref() == Some(pci)
        })
        .collect();
    if cards.is_empty() {
        return None;
    }
    let connected = cards.iter().any(|card| {
        entries.iter().any(|c| {
            c.strip_prefix(card.as_str())
                .and_then(|r| r.strip_prefix('-'))
                .is_some_and(|conn| {
                    (conn.starts_with("eDP") || conn.starts_with("LVDS") || conn.starts_with("DSI"))
                        && root.read_opt(format!("{DRM_CLASS}/{c}/status")).as_deref()
                            == Some("connected")
                })
        })
    });
    Some(connected)
}

/// Infers the graphics mode from the devices.
pub fn infer_mode(devices: &[GpuDevice]) -> GpuMode {
    let integrated = |d: &GpuDevice| d.vendor == GpuVendor::Intel || d.boot_vga == Some(true);
    match devices {
        [] => GpuMode::Unknown,
        [only] if integrated(only) => GpuMode::IntegratedOnly,
        [_] => GpuMode::Discrete,
        many => {
            let panel_on = many.iter().find(|d| d.drives_internal_panel == Some(true));
            match panel_on {
                Some(d) if integrated(d) => GpuMode::Hybrid,
                Some(_) => GpuMode::Discrete,
                None => GpuMode::Unknown,
            }
        }
    }
}

/// Enumerates display controllers from sysfs only (never wakes a GPU).
pub fn detect_devices(root: &SysRoot) -> Vec<GpuDevice> {
    let mut out = Vec::new();
    for addr in root.list(PCI_DEVICES) {
        let dir = format!("{PCI_DEVICES}/{addr}");
        let Some(class) = root.read_opt(format!("{dir}/class")) else {
            continue;
        };
        if !strip_hex(&class).starts_with("03") {
            continue;
        }
        let vendor_id = root
            .read_opt(format!("{dir}/vendor"))
            .map(|v| strip_hex(&v))
            .unwrap_or_default();
        let device_id = root
            .read_opt(format!("{dir}/device"))
            .map(|v| strip_hex(&v))
            .unwrap_or_default();
        let vendor = vendor_of(&vendor_id);
        let driver = sysfs::link_name(&root.path(format!("{dir}/driver")));

        let mut name = None;
        let mut driver_version = None;
        if vendor == GpuVendor::Nvidia && driver.as_deref() == Some("nvidia") {
            name = root
                .read_opt(format!("/proc/driver/nvidia/gpus/{addr}/information"))
                .and_then(|t| parse_nvidia_information(&t));
            driver_version = root
                .read_opt("/proc/driver/nvidia/version")
                .and_then(|t| parse_nvidia_version(&t));
        }
        if name.is_none() {
            name = pci_ids_name(root, &vendor_id, &device_id).map(|n| {
                let p = vendor_prefix(vendor);
                if p.is_empty() { n } else { format!("{p} {n}") }
            });
        }

        out.push(GpuDevice {
            name,
            driver,
            runtime_status: root.read_opt(format!("{dir}/power/runtime_status")),
            boot_vga: root.read_opt(format!("{dir}/boot_vga")).map(|v| v == "1"),
            drives_internal_panel: internal_panel(root, &addr),
            driver_version,
            nvml: if vendor == GpuVendor::Nvidia {
                NvmlStatus::Disabled
            } else {
                NvmlStatus::NotApplicable
            },
            metrics: None,
            pci_address: addr,
            vendor,
            vendor_id,
            device_id,
        });
    }
    out
}

/// Full GPU information, including NVML metrics when allowed.
pub fn read_gpu_info(root: &SysRoot, opts: GpuOptions) -> GpuInfo {
    let mut devices = detect_devices(root);
    if opts.use_nvml {
        nvml::fill(&mut devices, opts.wake_gpu);
    }
    let mode = infer_mode(&devices);
    GpuInfo { devices, mode }
}

#[cfg(feature = "nvml")]
mod nvml {
    use super::*;
    use legion_core::model::NvidiaMetrics;
    use nvml_wrapper::Nvml;
    use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
    use nvml_wrapper::error::NvmlError;

    fn parse_addr(addr: &str) -> Option<(u32, u32, u32)> {
        let mut it = addr.split([':', '.']);
        let dom = u32::from_str_radix(it.next()?, 16).ok()?;
        let bus = u32::from_str_radix(it.next()?, 16).ok()?;
        let dev = u32::from_str_radix(it.next()?, 16).ok()?;
        Some((dom, bus, dev))
    }

    /// NVML exposes P-states as `Zero`..`Fifteen`; render them as `P0`..`P15`.
    fn pstate_name(debug: &str) -> String {
        const WORDS: [&str; 16] = [
            "Zero", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
            "Eleven", "Twelve", "Thirteen", "Fourteen", "Fifteen",
        ];
        WORDS
            .iter()
            .position(|w| *w == debug)
            .map(|i| format!("P{i}"))
            .unwrap_or_else(|| debug.to_owned())
    }

    pub(super) fn fill(devices: &mut [GpuDevice], wake: bool) {
        let wanted: Vec<usize> = devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.vendor == GpuVendor::Nvidia)
            .map(|(i, _)| i)
            .collect();
        if wanted.is_empty() {
            return;
        }
        let mut to_query = Vec::new();
        for i in wanted {
            let d = &mut devices[i];
            if d.driver.as_deref() != Some("nvidia") {
                d.nvml = NvmlStatus::Error(format!(
                    "GPU bound to {:?}, not the NVIDIA proprietary/open driver",
                    d.driver
                ));
            } else if !wake && d.runtime_status.as_deref().is_some_and(|s| s != "active") {
                d.nvml = NvmlStatus::SkippedSuspended;
            } else {
                to_query.push(i);
            }
        }
        if to_query.is_empty() {
            return;
        }
        let nvml = match Nvml::init() {
            Ok(n) => n,
            Err(e) => {
                let status = match e {
                    NvmlError::LibloadingError(_) | NvmlError::LibraryNotFound => {
                        NvmlStatus::LibraryNotFound
                    }
                    other => NvmlStatus::Error(other.to_string()),
                };
                tracing::debug!(?status, "NVML unavailable");
                for i in to_query {
                    devices[i].nvml = status.clone();
                }
                return;
            }
        };
        let driver_version = nvml.sys_driver_version().ok();
        let count = nvml.device_count().unwrap_or(0);
        for i in to_query {
            let target = parse_addr(&devices[i].pci_address);
            let found = (0..count)
                .filter_map(|n| nvml.device_by_index(n).ok())
                .find(|dev| {
                    dev.pci_info()
                        .ok()
                        .is_some_and(|p| Some((p.domain, p.bus, p.device)) == target)
                });
            let d = &mut devices[i];
            let Some(dev) = found else {
                d.nvml = NvmlStatus::Error("device not found through NVML".into());
                continue;
            };
            let util = dev.utilization_rates().ok();
            let mem = dev.memory_info().ok();
            let m = NvidiaMetrics {
                name: dev.name().ok(),
                temperature_c: dev.temperature(TemperatureSensor::Gpu).ok(),
                utilization_percent: util.as_ref().map(|u| u.gpu),
                memory_utilization_percent: util.as_ref().map(|u| u.memory),
                memory_used_mib: mem.as_ref().map(|m| m.used / 1024 / 1024),
                memory_total_mib: mem.as_ref().map(|m| m.total / 1024 / 1024),
                graphics_clock_mhz: dev.clock_info(Clock::Graphics).ok(),
                memory_clock_mhz: dev.clock_info(Clock::Memory).ok(),
                power_draw_w: dev.power_usage().ok().map(|mw| mw as f64 / 1000.0),
                power_limit_w: dev.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0),
                fan_speed_percent: dev.fan_speed(0).ok(),
                performance_state: dev
                    .performance_state()
                    .ok()
                    .map(|p| pstate_name(&format!("{p:?}"))),
                driver_version: driver_version.clone(),
            };
            if d.name.is_none() {
                d.name = m.name.clone();
            }
            if d.driver_version.is_none() {
                d.driver_version = m.driver_version.clone();
            }
            d.metrics = Some(m);
            d.nvml = NvmlStatus::Available;
        }
    }
}

#[cfg(not(feature = "nvml"))]
mod nvml {
    use super::*;

    pub(super) fn fill(devices: &mut [GpuDevice], _wake: bool) {
        for d in devices.iter_mut().filter(|d| d.vendor == GpuVendor::Nvidia) {
            d.nvml = NvmlStatus::Error("built without NVML support".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    const PCI_IDS_SAMPLE: &str = "# comment\n8086  Intel Corporation\n\t7d67  Arrow Lake-S [Intel Graphics]\n\t\t1234 5678  Subsystem\n\
                                  10de  NVIDIA Corporation\n\t2d18  GB206M [GeForce RTX 5070 Max-Q / Mobile]\n1002  AMD\n";

    #[test]
    fn pci_ids_lookup() {
        assert_eq!(
            lookup_pci_ids(PCI_IDS_SAMPLE, "8086", "7d67").as_deref(),
            Some("Arrow Lake-S [Intel Graphics]")
        );
        assert_eq!(
            lookup_pci_ids(PCI_IDS_SAMPLE, "10DE", "2d18").as_deref(),
            Some("GB206M [GeForce RTX 5070 Max-Q / Mobile]")
        );
        assert_eq!(lookup_pci_ids(PCI_IDS_SAMPLE, "10de", "ffff"), None);
        assert_eq!(lookup_pci_ids(PCI_IDS_SAMPLE, "8086", "1234"), None);
    }

    #[test]
    fn nvidia_proc_parsing() {
        let v = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.71.09  Release Build  (x@y)  date\nGCC version: 1.2";
        assert_eq!(parse_nvidia_version(v).as_deref(), Some("615.71.09"));
        let info = "Model: \t\t NVIDIA GeForce RTX 5070 Laptop GPU\nIRQ:   16\n";
        assert_eq!(
            parse_nvidia_information(info).as_deref(),
            Some("NVIDIA GeForce RTX 5070 Laptop GPU")
        );
    }

    fn hybrid() -> FakeSysfs {
        let fs = FakeSysfs::new();
        let igpu = "/sys/devices/pci0000:00/0000:00:02.0";
        let dgpu = "/sys/devices/pci0000:00/0000:00:06.0/0000:02:00.0";
        fs.file(&format!("{igpu}/class"), "0x030000")
            .file(&format!("{igpu}/vendor"), "0x8086")
            .file(&format!("{igpu}/device"), "0x7d67")
            .file(&format!("{igpu}/boot_vga"), "1")
            .symlink(&format!("{igpu}/driver"), "/sys/bus/pci/drivers/xe")
            .file(&format!("{dgpu}/class"), "0x030000")
            .file(&format!("{dgpu}/vendor"), "0x10de")
            .file(&format!("{dgpu}/device"), "0x2d18")
            .file(&format!("{dgpu}/boot_vga"), "0")
            .file(&format!("{dgpu}/power/runtime_status"), "suspended")
            .symlink(&format!("{dgpu}/driver"), "/sys/bus/pci/drivers/nvidia")
            .symlink("/sys/bus/pci/devices/0000:00:02.0", igpu)
            .symlink("/sys/bus/pci/devices/0000:02:00.0", dgpu)
            .symlink("/sys/class/drm/card1/device", igpu)
            .symlink("/sys/class/drm/card0/device", dgpu)
            .file("/sys/class/drm/card1-eDP-1/status", "connected")
            .file("/sys/class/drm/card0-eDP-2/status", "disconnected")
            .file("/sys/class/drm/card0-HDMI-A-1/status", "connected")
            .file(
                "/proc/driver/nvidia/gpus/0000:02:00.0/information",
                "Model: \t\t NVIDIA GeForce RTX 5070 Laptop GPU\n",
            )
            .file("/proc/driver/nvidia/version", "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.71.09  Release Build\n")
            .file("/usr/share/hwdata/pci.ids", PCI_IDS_SAMPLE);
        fs
    }

    #[test]
    fn detects_hybrid_legion() {
        let fs = hybrid();
        let info = read_gpu_info(
            &fs.root(),
            GpuOptions {
                use_nvml: false,
                wake_gpu: false,
            },
        );
        assert_eq!(info.devices.len(), 2);
        let i = &info.devices[0];
        assert_eq!(i.vendor, GpuVendor::Intel);
        assert_eq!(
            i.name.as_deref(),
            Some("Intel Arrow Lake-S [Intel Graphics]")
        );
        assert_eq!(i.driver.as_deref(), Some("xe"));
        assert_eq!(i.drives_internal_panel, Some(true));
        let d = &info.devices[1];
        assert_eq!(
            d.name.as_deref(),
            Some("NVIDIA GeForce RTX 5070 Laptop GPU")
        );
        assert_eq!(d.driver_version.as_deref(), Some("615.71.09"));
        assert_eq!(d.drives_internal_panel, Some(false));
        assert_eq!(d.runtime_status.as_deref(), Some("suspended"));
        assert_eq!(d.nvml, NvmlStatus::Disabled);
        assert_eq!(info.mode, GpuMode::Hybrid);
    }

    #[test]
    fn suspended_gpu_is_not_woken() {
        let fs = hybrid();
        let info = read_gpu_info(
            &fs.root(),
            GpuOptions {
                use_nvml: true,
                wake_gpu: false,
            },
        );
        assert_eq!(info.devices[1].nvml, NvmlStatus::SkippedSuspended);
        assert!(info.devices[1].metrics.is_none());
    }

    #[test]
    fn discrete_mode_when_panel_on_dgpu() {
        let fs = hybrid();
        fs.file("/sys/class/drm/card1-eDP-1/status", "disconnected")
            .file("/sys/class/drm/card0-eDP-2/status", "connected");
        let info = read_gpu_info(
            &fs.root(),
            GpuOptions {
                use_nvml: false,
                wake_gpu: false,
            },
        );
        assert_eq!(info.mode, GpuMode::Discrete);
    }

    #[test]
    fn empty() {
        let fs = FakeSysfs::new();
        let info = read_gpu_info(&fs.root(), GpuOptions::default());
        assert!(info.devices.is_empty());
        assert_eq!(info.mode, GpuMode::Unknown);
    }
}
