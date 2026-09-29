//! `legionctl doctor`: a complete, read-only diagnostic.

use legion_core::capabilities::CapabilityState;
use legion_core::doctor::{CheckResult, CheckStatus as St, DoctorReport};
use legion_core::model::{GpuVendor, Metric, NvmlStatus};
use legion_core::support;

use crate::capabilities::{Inventory, from_inventory};
use crate::controller::LinuxController;

/// D-Bus name the daemon will own (Phase 3).
pub const DAEMON_BUS_NAME: &str = "com.legion.Control";

const UNIT_PATHS: [&str; 3] = [
    "/etc/systemd/system/legion-control.service",
    "/usr/lib/systemd/system/legion-control.service",
    "/lib/systemd/system/legion-control.service",
];

fn check(name: &str, status: St, detail: impl Into<String>) -> CheckResult {
    CheckResult::new(name, status, detail)
}

/// Maps a capability to a doctor line.
fn cap_check(name: &str, cp: &legion_core::capabilities::Capability) -> CheckResult {
    let iface = cp
        .interface
        .clone()
        .map(|i| format!(" [{i}]"))
        .unwrap_or_default();
    match cp.state {
        CapabilityState::Unsupported => check(name, St::Unsupported, cp.note.clone()),
        CapabilityState::Supported => check(name, St::Pass, format!("{}{iface}", cp.note)),
        CapabilityState::ReadOnly => {
            check(name, St::Pass, format!("read-only: {}{iface}", cp.note))
        }
        CapabilityState::Detected => {
            check(name, St::Pass, format!("available, {}{iface}", cp.note))
        }
    }
}

/// Effective uid of this process, from `/proc/self/status`.
fn effective_uid(ctrl: &LinuxController) -> Option<u32> {
    ctrl.root()
        .read_opt("/proc/self/status")?
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|v| v.split_whitespace().nth(1)?.parse().ok())
}

/// Runs every check against an already-gathered inventory.
pub fn run_with(ctrl: &LinuxController, inv: &Inventory) -> DoctorReport {
    let mut c = Vec::new();
    let sys = &inv.system;
    let caps = from_inventory(inv);
    let cap = |id: &str| caps.iter().find(|c| c.id == id);

    // Hardware detection
    c.push(match (&sys.manufacturer, &sys.product_name) {
        (Some(m), Some(_)) => check(
            "Hardware detection",
            St::Pass,
            format!(
                "{m} {} / board {}",
                sys.display_product(),
                sys.board_name.as_deref().unwrap_or("?")
            ),
        ),
        _ => check(
            "Hardware detection",
            St::Fail,
            "DMI information not readable",
        ),
    });
    c.push(match support::lookup(sys) {
        Some(m) => check(
            "Support matrix",
            St::Pass,
            format!("known model: {}", m.name),
        ),
        None if support::is_legion(sys) => check(
            "Support matrix",
            St::Warn,
            "Legion model not validated yet; only generic Linux interfaces are used",
        ),
        None => check("Support matrix", St::Unsupported, "not a Lenovo Legion"),
    });
    c.push(match &sys.kernel {
        Some(k) => check("Kernel", St::Pass, k.clone()),
        None => check("Kernel", St::Warn, "unknown"),
    });
    c.push(match &sys.bios_version {
        Some(v) => check(
            "BIOS",
            St::Pass,
            format!(
                "{v}{}",
                sys.bios_date
                    .as_ref()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            ),
        ),
        None => check("BIOS", St::Warn, "version not readable"),
    });
    c.push(check(
        "CPU",
        if sys.cpu_model.is_some() {
            St::Pass
        } else {
            St::Fail
        },
        sys.cpu_model
            .clone()
            .unwrap_or_else(|| "not detected".into()),
    ));
    c.push(check(
        "GPU",
        if inv.gpu.devices.is_empty() {
            St::Warn
        } else {
            St::Pass
        },
        if inv.gpu.devices.is_empty() {
            "no display controller found".into()
        } else {
            format!("{} — mode: {:?}", sys.gpus.join(" + "), inv.gpu.mode)
        },
    ));
    c.push(match &inv.memory {
        Some(m) => check("Memory", St::Pass, format!("{} MiB total", m.total_mib)),
        None => check("Memory", St::Fail, "/proc/meminfo not readable"),
    });

    // ACPI / WMI / Lenovo
    let a = &inv.acpi;
    c.push(if a.acpi_present {
        check(
            "ACPI",
            St::Pass,
            format!("{} tables, {} devices", a.tables.len(), a.devices.len()),
        )
    } else {
        check("ACPI", St::Fail, "/sys/firmware/acpi missing")
    });
    let bound = a.wmi_devices.iter().filter(|w| w.driver.is_some()).count();
    c.push(if a.wmi_present {
        check(
            "WMI",
            St::Pass,
            format!("{} blocks, {bound} bound to drivers", a.wmi_devices.len()),
        )
    } else {
        check("WMI", St::Unsupported, "no WMI bus")
    });
    c.push(if a.lenovo.drivers.is_empty() {
        check(
            "Lenovo interfaces",
            St::Warn,
            "no Lenovo platform drivers loaded",
        )
    } else {
        check(
            "Lenovo interfaces",
            St::Pass,
            a.lenovo
                .drivers
                .iter()
                .map(|d| d.driver.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    });

    // Power profile
    let pp = &inv.profile;
    c.push(match &pp.platform_profile {
        Some(p) => check(
            "Power profile",
            St::Pass,
            format!(
                "{}{} — choices: {}",
                p.current,
                match p.current_raw.as_deref() {
                    Some(raw) if raw != p.current.as_str() => format!(" (raw: {raw})"),
                    Some(_) => String::new(),
                    None => " (raw: ?)".into(),
                },
                p.choices.join(" ")
            ),
        ),
        None => check("Power profile", St::Unsupported, "no platform_profile"),
    });
    c.push(if !ctrl.options().host_services {
        check(
            "power-profiles-daemon",
            St::Unsupported,
            "skipped (fixture root)",
        )
    } else {
        match &pp.ppd {
            Some(d) => check(
                "power-profiles-daemon",
                St::Pass,
                format!(
                    "active: {}{}",
                    d.active,
                    d.performance_degraded
                        .as_ref()
                        .map(|r| format!(" (performance degraded: {r})"))
                        .unwrap_or_default()
                ),
            ),
            None => check(
                "power-profiles-daemon",
                St::Warn,
                pp.notes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "not running".into()),
            ),
        }
    });

    // CPU monitoring
    let cpu = &inv.cpu;
    c.push(check(
        "CPU monitoring",
        if cpu.frequency_mhz.is_some() {
            St::Pass
        } else {
            St::Warn
        },
        format!(
            "driver {} / governor {} / EPP {}",
            cpu.scaling_driver.as_deref().unwrap_or("?"),
            if cpu.governors.is_empty() {
                "?".into()
            } else {
                cpu.governors.join(",")
            },
            if cpu.epp.is_empty() {
                "n/a".into()
            } else {
                cpu.epp.join(",")
            }
        ),
    ));
    c.push(match &cpu.package_power_w {
        Metric::Ok(w) => check("CPU package power", St::Pass, format!("{w:.1} W (RAPL)")),
        Metric::PermissionRequired => check(
            "CPU package power",
            St::Warn,
            "RAPL present — Administrator permission required (energy_uj is root-only)",
        ),
        Metric::NotSupported => check("CPU package power", St::Unsupported, "no RAPL package zone"),
        Metric::Unavailable(r) => check("CPU package power", St::Warn, r.clone()),
    });

    // GPU monitoring
    let nv = inv
        .gpu
        .devices
        .iter()
        .find(|d| d.vendor == GpuVendor::Nvidia);
    c.push(match nv.map(|d| &d.nvml) {
        None => check("GPU monitoring", St::Unsupported, "no NVIDIA GPU"),
        Some(NvmlStatus::Available) => check(
            "GPU monitoring",
            St::Pass,
            format!("NVML, driver {}", nv.and_then(|d| d.driver_version.clone()).unwrap_or_default()),
        ),
        Some(NvmlStatus::SkippedSuspended) => check(
            "GPU monitoring",
            St::Pass,
            "dGPU suspended (power saving); NVML not queried — use `legionctl gpu status --wake-gpu`",
        ),
        Some(NvmlStatus::Disabled) => check("GPU monitoring", St::Unsupported, "skipped (fixture root)"),
        Some(NvmlStatus::LibraryNotFound) => {
            check("GPU monitoring", St::Warn, "libnvidia-ml.so not found (install nvidia-utils)")
        }
        Some(other) => check("GPU monitoring", St::Warn, format!("{other:?}")),
    });

    // Sensors
    let chips = legion_core::hwmon::chips(ctrl.root());
    c.push(if chips.is_empty() {
        check("hwmon", St::Warn, "no hwmon chips")
    } else {
        check(
            "hwmon",
            St::Pass,
            chips
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    });
    let t = &inv.thermal;
    c.push(match t.cpu_c {
        Some(cpu_c) => check(
            "Thermal",
            St::Pass,
            format!(
                "{} sensors; CPU {cpu_c:.0} °C{}",
                t.sensors.len(),
                t.gpu_c
                    .map(|g| format!(", GPU {g:.0} °C"))
                    .unwrap_or_default()
            ),
        ),
        None if !t.sensors.is_empty() => check(
            "Thermal",
            St::Warn,
            format!("{} sensors, CPU sensor not identified", t.sensors.len()),
        ),
        None => check("Thermal", St::Fail, "no temperature sensors"),
    });
    c.push(if inv.fans.fans.is_empty() {
        check("Fan monitoring", St::Unsupported, "no fan RPM sensors")
    } else {
        check(
            "Fan monitoring",
            St::Pass,
            inv.fans
                .fans
                .iter()
                .map(|f| {
                    format!(
                        "{}: {} RPM",
                        f.label,
                        f.rpm.map(|r| r.to_string()).unwrap_or("?".into())
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        )
    });
    if let Some(cp) = cap("fan_control") {
        c.push(cap_check("Fan control", cp));
    }
    if let Some(cp) = cap("fan_curve") {
        c.push(cap_check("Fan curve", cp));
    }

    // Battery
    c.push(match inv.battery.batteries.first() {
        Some(b) => check(
            "Battery",
            St::Pass,
            format!(
                "{} {}% ({}), health {}, {} cycles",
                b.name,
                b.capacity_percent
                    .map(|v| v.to_string())
                    .unwrap_or("?".into()),
                b.status.as_deref().unwrap_or("?"),
                b.health_percent
                    .map(|h| format!("{h:.0}%"))
                    .unwrap_or("?".into()),
                b.cycle_count.map(|v| v.to_string()).unwrap_or("?".into())
            ),
        ),
        None => check("Battery", St::Unsupported, "no system battery"),
    });
    for (id, name) in [
        ("battery_threshold", "Battery threshold"),
        ("battery_conservation", "Battery conservation"),
        ("charge_type", "Charge type"),
        ("cpu_tuning", "CPU tuning"),
        ("cpu_power_limit", "CPU power limits"),
        ("gpu_power_limit", "GPU power limits"),
    ] {
        if let Some(cp) = cap(id) {
            c.push(cap_check(name, cp));
        }
    }
    if let Some(cp) = cap("rgb_control") {
        c.push(check(
            "Keyboard RGB",
            if cp.state == CapabilityState::Unsupported {
                St::Unsupported
            } else {
                St::Pass
            },
            cp.note.clone(),
        ));
    }

    // Environment
    c.push(match effective_uid(ctrl) {
        Some(0) => check(
            "Permissions",
            St::Pass,
            "running as root (not required for diagnostics)",
        ),
        Some(uid) => check(
            "Permissions",
            St::Pass,
            if inv.cpu.package_power_w == Metric::PermissionRequired {
                format!("uid {uid}; unprivileged — RAPL energy requires administrator permission")
            } else {
                format!("uid {uid}; unprivileged read-only mode, all monitored values readable")
            },
        ),
        None => check("Permissions", St::Warn, "could not determine uid"),
    });
    if ctrl.options().host_services {
        match legion_profiles::ppd::name_has_owner(DAEMON_BUS_NAME) {
            Ok(owned) => {
                c.push(check("D-Bus", St::Pass, "system bus reachable"));
                c.push(check(
                    "Legion Control daemon",
                    if owned { St::Pass } else { St::Warn },
                    if owned {
                        format!("{DAEMON_BUS_NAME} is running")
                    } else {
                        "not running — changes need `sudo systemctl enable --now \
                         legion-control.service` (reading works without it)"
                            .into()
                    },
                ));
            }
            Err(e) => c.push(check("D-Bus", St::Fail, e)),
        }
    } else {
        c.push(check("D-Bus", St::Unsupported, "skipped (fixture root)"));
    }
    let root = ctrl.root();
    c.push(if root.exists("/run/systemd/system") {
        let unit = UNIT_PATHS.iter().find(|p| root.exists(p));
        check(
            "systemd",
            St::Pass,
            match unit {
                Some(p) => format!("running; unit installed at {p}"),
                None => "running; legion-control.service not installed".into(),
            },
        )
    } else {
        check("systemd", St::Warn, "systemd is not the init system")
    });

    DoctorReport { checks: c }
}

/// Gathers the inventory and runs the checks.
pub fn run(ctrl: &LinuxController) -> DoctorReport {
    let inv = crate::capabilities::gather(ctrl);
    run_with(ctrl, &inv)
}
