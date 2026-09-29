//! Capability detection with evidence.
//!
//! Every capability is derived from something observed on the running
//! system:
//!
//! * `Supported` — Legion Control can change it (through the daemon, or
//!   `legionctl` as root), using a documented interface;
//! * `ReadOnly` — monitored only;
//! * `Detected` — an interface exists but is not used (not documented, or
//!   an experimental feature disabled in `/etc/legion-control/daemon.json`);
//! * `Unsupported` — no interface.

use legion_acpi::AcpiReport;
use legion_core::capabilities::{Capability, CapabilityState as S};
use legion_core::model::{
    BatteryInfo, CpuInfo, FanInfo, GpuInfo, GpuMode, GpuVendor, MemoryInfo, NvmlStatus, SystemInfo,
    ThermalInfo,
};
use legion_core::profile::PowerProfileState;
use legion_core::{HardwareController, SysRoot};

use crate::config::{CONFIG_PATH, Config};
use crate::controller::LinuxController;

/// Everything collected in one pass (used by capabilities, doctor and
/// hardware-scan so NVML/D-Bus are queried only once).
#[derive(Debug, Clone)]
pub struct Inventory {
    /// System identification.
    pub system: SystemInfo,
    /// Power profile.
    pub profile: PowerProfileState,
    /// CPU.
    pub cpu: CpuInfo,
    /// GPU.
    pub gpu: GpuInfo,
    /// Memory.
    pub memory: Option<MemoryInfo>,
    /// Temperatures.
    pub thermal: ThermalInfo,
    /// Fans.
    pub fans: FanInfo,
    /// Battery.
    pub battery: BatteryInfo,
    /// ACPI/WMI discovery.
    pub acpi: AcpiReport,
    /// Keyboard backlight LED class devices.
    pub keyboard_leds: Vec<String>,
    /// Fans that can be controlled (`chip/fanN`).
    pub controllable_fans: Vec<String>,
    /// Configuration in effect.
    pub config: Config,
}

fn keyboard_leds(root: &SysRoot) -> Vec<String> {
    root.list("/sys/class/leds")
        .into_iter()
        .filter(|l| l.contains("kbd_backlight") || l.contains("rgb:kbd"))
        .collect()
}

/// Collects the inventory.
pub fn gather(ctrl: &LinuxController) -> Inventory {
    let root = ctrl.root();
    let gpu = ctrl.get_gpu_info().unwrap_or(GpuInfo {
        devices: vec![],
        mode: GpuMode::Unknown,
    });
    let mut system = ctrl.get_system_info().unwrap_or_default();
    // Prefer names learned through NVML.
    system.gpus = gpu
        .devices
        .iter()
        .map(|d| {
            d.name
                .clone()
                .unwrap_or_else(|| format!("{}:{}", d.vendor_id, d.device_id))
        })
        .collect();
    Inventory {
        system,
        profile: legion_profiles::detect(root, ctrl.options().host_services),
        cpu: legion_cpu::read_cpu_info(root, ctrl.options().sample_interval),
        memory: ctrl.get_memory_info().ok(),
        thermal: ctrl.temperatures_with(&gpu),
        gpu,
        fans: legion_fan::read_fans(root),
        battery: legion_battery::read_battery_info(root),
        acpi: legion_acpi::scan(root),
        keyboard_leds: keyboard_leds(root),
        controllable_fans: legion_fan::controllable_fans(root)
            .iter()
            .map(|f| format!("hwmon:{}/fan{}_target", f.chip.name, f.channel))
            .collect(),
        config: ctrl.options().config.clone(),
    }
}

/// Convenience: gather + derive.
pub fn detect(ctrl: &LinuxController) -> Vec<Capability> {
    from_inventory(&gather(ctrl))
}

/// Adjustable firmware attributes among `names`.
fn adjustable(acpi: &AcpiReport, prefixes: &[&str]) -> (Vec<String>, Vec<String>) {
    let mut adj = Vec::new();
    let mut fixed = Vec::new();
    for a in acpi
        .lenovo
        .firmware_attributes
        .iter()
        .flat_map(|s| &s.attributes)
    {
        if !prefixes.iter().any(|p| a.name.starts_with(p)) {
            continue;
        }
        if a.range().is_some() {
            adj.push(a.name.clone());
        } else {
            fixed.push(a.name.clone());
        }
    }
    (adj, fixed)
}

fn gated(enabled: bool, flag: &str) -> (S, String) {
    if enabled {
        (S::Supported, "experimental; enabled".into())
    } else {
        (
            S::Detected,
            format!("experimental; disabled — set \"{flag}\": true in {CONFIG_PATH}"),
        )
    }
}

/// Derives the capability list from an inventory.
pub fn from_inventory(inv: &Inventory) -> Vec<Capability> {
    let mut out = Vec::new();

    // Power profiles
    out.push(match &inv.profile.platform_profile {
        Some(pp) if !pp.choices.is_empty() => Capability::new(
            "power_profiles",
            S::Supported,
            Some("/sys/firmware/acpi/platform_profile".into()),
            format!(
                "choices: {}; handler: {}{}",
                pp.choices.join(", "),
                pp.handlers
                    .iter()
                    .filter_map(|h| h.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                if inv.profile.ppd.is_some() {
                    "; quiet/balanced/performance via power-profiles-daemon"
                } else {
                    ""
                }
            ),
        ),
        _ if inv.profile.ppd.is_some() => Capability::new(
            "power_profiles",
            S::Supported,
            Some("power-profiles-daemon (D-Bus)".into()),
            "no kernel platform_profile; power-profiles-daemon only",
        ),
        _ => Capability::new(
            "power_profiles",
            S::Unsupported,
            None,
            "no platform_profile",
        ),
    });

    out.push(Capability::new(
        "cpu_monitoring",
        if inv.cpu.logical_cpus > 0 {
            S::ReadOnly
        } else {
            S::Unsupported
        },
        Some("/proc/stat, /sys/devices/system/cpu".into()),
        format!("{} logical CPUs", inv.cpu.logical_cpus),
    ));
    out.push(if inv.cpu.available_governors.is_empty() {
        Capability::new("cpu_tuning", S::Unsupported, None, "no cpufreq policies")
    } else {
        Capability::new(
            "cpu_tuning",
            S::Supported,
            Some("cpufreq scaling_governor / energy_performance_preference / no_turbo".into()),
            format!(
                "governors: {}; EPP: {}",
                inv.cpu.available_governors.join(", "),
                if inv.cpu.available_epp.is_empty() {
                    "n/a".into()
                } else {
                    inv.cpu.available_epp.join(", ")
                }
            ),
        )
    });

    // GPU monitoring
    let nv: Vec<_> = inv
        .gpu
        .devices
        .iter()
        .filter(|d| d.vendor == GpuVendor::Nvidia)
        .collect();
    out.push(if nv.is_empty() {
        Capability::new("gpu_monitoring", S::Unsupported, None, "no NVIDIA GPU")
    } else {
        match &nv[0].nvml {
            NvmlStatus::Available => Capability::new(
                "gpu_monitoring",
                S::ReadOnly,
                Some("NVML".into()),
                "NVML available",
            ),
            NvmlStatus::SkippedSuspended => Capability::new(
                "gpu_monitoring",
                S::ReadOnly,
                Some("NVML".into()),
                "dGPU runtime-suspended; NVML not queried to avoid waking it",
            ),
            NvmlStatus::Disabled => Capability::new(
                "gpu_monitoring",
                S::ReadOnly,
                Some("NVML".into()),
                "NVML not queried (fixture)",
            ),
            NvmlStatus::LibraryNotFound => Capability::new(
                "gpu_monitoring",
                S::Unsupported,
                None,
                "libnvidia-ml.so not found (install nvidia-utils)",
            ),
            other => Capability::new(
                "gpu_monitoring",
                S::Unsupported,
                Some("NVML".into()),
                format!("{other:?}"),
            ),
        }
    });

    out.push(Capability::new(
        "gpu_mode",
        if inv.gpu.mode == GpuMode::Unknown {
            S::Unsupported
        } else {
            S::ReadOnly
        },
        Some("/sys/class/drm/*/status".into()),
        format!(
            "{:?} (inferred from which GPU drives the internal panel); switching is not supported",
            inv.gpu.mode
        ),
    ));

    // Fans
    out.push(Capability::new(
        "fan_monitoring",
        if inv.fans.fans.is_empty() {
            S::Unsupported
        } else {
            S::ReadOnly
        },
        inv.fans.fans.first().map(|f| format!("hwmon:{}", f.chip)),
        format!("{} fan(s)", inv.fans.fans.len()),
    ));
    if inv.controllable_fans.is_empty() {
        let unvalidated: Vec<_> = inv
            .fans
            .fans
            .iter()
            .filter(|f| f.control_interface_present)
            .map(|f| f.source.clone())
            .collect();
        out.push(if unvalidated.is_empty() {
            Capability::new(
                "fan_control",
                S::Unsupported,
                None,
                legion_fan::FAN_CONTROL_UNSUPPORTED,
            )
        } else {
            Capability::new(
                "fan_control",
                S::Detected,
                Some(unvalidated.join(", ")),
                "writable interface from an undocumented driver; not used",
            )
        });
        out.push(Capability::new(
            "fan_curve",
            S::Unsupported,
            None,
            "requires a controllable fan",
        ));
    } else {
        let (state, note) = gated(inv.config.enable_fan_control, "enable_fan_control");
        out.push(Capability::new(
            "fan_control",
            state,
            Some(inv.controllable_fans.join(", ")),
            format!("{note}; target RPM, 0 = firmware auto (lenovo-wmi-other docs)"),
        ));
        out.push(Capability::new(
            "fan_curve",
            state,
            Some("software curve in legion-control-daemon".into()),
            format!(
                "{note}; reverts to firmware control above {:.0} °C CPU / {:.0} °C GPU",
                inv.config.fan_safety_cpu_c, inv.config.fan_safety_gpu_c
            ),
        ));
    }

    // Battery
    let bat = inv.battery.batteries.first();
    out.push(Capability::new(
        "battery_monitoring",
        if bat.is_some() {
            S::ReadOnly
        } else {
            S::Unsupported
        },
        bat.map(|b| format!("/sys/class/power_supply/{}", b.name)),
        "",
    ));
    out.push(match bat.and_then(|b| b.charge_end_threshold.map(|_| b)) {
        Some(b) => Capability::new(
            "battery_threshold",
            S::Supported,
            Some(format!(
                "/sys/class/power_supply/{}/charge_control_end_threshold",
                b.name
            )),
            "numeric charge threshold",
        ),
        None => Capability::new(
            "battery_threshold",
            S::Unsupported,
            None,
            "no charge_control_end_threshold; see battery_conservation",
        ),
    });
    let long_life = bat.and_then(|b| {
        b.charge_types
            .as_ref()
            .filter(|c| c.available.iter().any(|a| a == "Long_Life"))
            .map(|_| {
                format!(
                    "/sys/class/power_supply/{}/charge_types (Long_Life)",
                    b.name
                )
            })
    });
    let ideapad = inv
        .battery
        .conservation_mode
        .map(|_| format!("{}/conservation_mode", legion_battery::IDEAPAD_DEVICE));
    let cons_ifaces: Vec<String> = ideapad.into_iter().chain(long_life).collect();
    out.push(if cons_ifaces.is_empty() {
        Capability::new("battery_conservation", S::Unsupported, None, "")
    } else {
        Capability::new(
            "battery_conservation",
            S::Supported,
            Some(cons_ifaces.join(", ")),
            format!(
                "currently {}; fixed firmware charge limit",
                match inv.battery.conservation_mode {
                    Some(true) => "enabled",
                    Some(false) => "disabled",
                    None => "unknown",
                }
            ),
        )
    });
    out.push(match bat.and_then(|b| b.charge_types.as_ref()) {
        Some(ct) => Capability::new(
            "charge_type",
            S::Supported,
            Some("/sys/class/power_supply/*/charge_types".into()),
            format!("available: {}", ct.available.join(", ")),
        ),
        None => Capability::new("charge_type", S::Unsupported, None, "no charge_types"),
    });

    // Power limits (firmware attributes from lenovo-wmi-other)
    for (id, prefixes) in [
        ("cpu_power_limit", &["ppt_", "cpu_"][..]),
        ("gpu_power_limit", &["gpu_"][..]),
    ] {
        let (adj, fixed) = adjustable(&inv.acpi, prefixes);
        out.push(if adj.is_empty() && fixed.is_empty() {
            Capability::new(id, S::Unsupported, None, "no Lenovo firmware attributes")
        } else if adj.is_empty() {
            Capability::new(
                id,
                S::ReadOnly,
                Some(format!("firmware-attributes: {}", fixed.join(", "))),
                "attributes report no adjustable range",
            )
        } else {
            let (state, note) = gated(inv.config.enable_power_limits, "enable_power_limits");
            Capability::new(
                id,
                state,
                Some(format!("firmware-attributes: {}", adj.join(", "))),
                format!(
                    "{note}; only effective in the custom profile{}",
                    if fixed.is_empty() {
                        String::new()
                    } else {
                        format!("; fixed: {}", fixed.join(", "))
                    }
                ),
            )
        });
    }

    out.push(if inv.keyboard_leds.is_empty() {
        Capability::new(
            "rgb_control",
            S::Unsupported,
            None,
            "no keyboard LED class device (USB HID keyboard controllers are not probed)",
        )
    } else {
        Capability::new(
            "rgb_control",
            S::Detected,
            Some(inv.keyboard_leds.join(", ")),
            "LED class device present; not used",
        )
    });

    out.push(Capability::new(
        "thermal_monitoring",
        if inv.thermal.sensors.is_empty() {
            S::Unsupported
        } else {
            S::ReadOnly
        },
        Some("/sys/class/thermal, hwmon".into()),
        format!("{} sensor(s)", inv.thermal.sensors.len()),
    ));
    out
}
