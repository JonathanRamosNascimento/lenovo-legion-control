//! Human-readable rendering.

use std::fmt::Write;
use std::io::IsTerminal;

use legion_core::capabilities::{Capability, CapabilityState};
use legion_core::doctor::{CheckStatus, DoctorReport};
use legion_core::model::*;
use legion_core::profile::{PowerProfileState, ProfileBackend};
use legion_hardware::report::HardwareReport;
use legion_i18n::tr;

const NOT_SUPPORTED: &str = "Not supported on this hardware.";
const NEEDS_ROOT: &str = "Administrator permission required.";

fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

fn paint(s: &str, code: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_owned()
    }
}

fn opt<T: std::fmt::Display>(v: &Option<T>, unit: &str) -> String {
    match v {
        Some(v) => format!("{v}{unit}"),
        None => "n/a".into(),
    }
}

fn metric(m: &Metric<f64>, unit: &str, prec: usize) -> String {
    match m {
        Metric::Ok(v) => format!("{v:.prec$}{unit}"),
        Metric::NotSupported => tr(NOT_SUPPORTED),
        Metric::PermissionRequired => tr(NEEDS_ROOT),
        Metric::Unavailable(r) => tr(&format!("unavailable ({r})")),
    }
}

/// Writes `  Label:   value`, translating both.
fn row(out: &mut String, label: &str, value: impl AsRef<str>) {
    let label = tr(label);
    let _ = writeln!(out, "  {:<26} {}", format!("{label}:"), tr(value.as_ref()));
}

/// Section header (`System:`).
fn head(label: &str) -> String {
    format!("{}:\n", tr(label))
}

fn list(v: &[String]) -> String {
    if v.is_empty() {
        "n/a".into()
    } else {
        v.join(", ")
    }
}

/// `legionctl hardware`.
pub fn system(s: &SystemInfo) -> String {
    let mut o = head("System");
    row(&mut o, "Manufacturer", opt(&s.manufacturer, ""));
    row(&mut o, "Product", opt(&s.product_version, ""));
    row(&mut o, "Machine type", opt(&s.product_name, ""));
    row(&mut o, "Board", opt(&s.board_name, ""));
    row(
        &mut o,
        "BIOS",
        format!(
            "{} {}",
            opt(&s.bios_version, ""),
            s.bios_date
                .as_deref()
                .map(|d| format!("({d})"))
                .unwrap_or_default()
        ),
    );
    if s.ec_firmware_release.is_some() {
        row(&mut o, "EC firmware", opt(&s.ec_firmware_release, ""));
    }
    row(&mut o, "CPU", opt(&s.cpu_model, ""));
    row(&mut o, "GPU", list(&s.gpus));
    row(&mut o, "Memory", opt(&s.memory_total_mib, " MiB"));
    row(&mut o, "OS", opt(&s.os_name, ""));
    row(&mut o, "Kernel", opt(&s.kernel, ""));
    row(&mut o, "Desktop", opt(&s.desktop, ""));
    row(
        &mut o,
        "Support",
        match &s.known_model {
            Some(m) => format!("known model ({m})"),
            None => "model not in the support matrix — generic interfaces only".into(),
        },
    );
    o
}

/// `legionctl profile`.
pub fn profile(p: &PowerProfileState) -> String {
    let mut o = head("Power profile");
    row(&mut o, "Current", p.current.to_string());
    row(
        &mut o,
        "Backend",
        match p.backend {
            ProfileBackend::PowerProfilesDaemon => "power-profiles-daemon",
            ProfileBackend::PlatformProfile => "platform_profile",
            ProfileBackend::None => NOT_SUPPORTED,
        },
    );
    if let Some(pp) = &p.platform_profile {
        row(&mut o, "platform_profile", opt(&pp.current_raw, ""));
        row(&mut o, "Choices", pp.choices.join(" "));
        for h in &pp.handlers {
            row(
                &mut o,
                "Handler",
                format!("{} ({})", h.name.as_deref().unwrap_or("?"), h.id),
            );
        }
        let _ = writeln!(o, "  {}:", tr("Mapping"));
        for m in &pp.mapping {
            let _ = writeln!(o, "    {:<12} -> {}", m.logical.to_string(), m.raw);
        }
    }
    if let Some(d) = &p.ppd {
        row(&mut o, "ppd active", &d.active);
        row(
            &mut o,
            "ppd profiles",
            d.profiles
                .iter()
                .map(|x| match &x.driver {
                    Some(dr) => format!("{} ({dr})", x.profile),
                    None => x.profile.clone(),
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    for n in &p.notes {
        row(&mut o, "Note", n);
    }
    o
}

/// `legionctl cpu status`.
pub fn cpu(c: &CpuInfo) -> String {
    let mut o = head("CPU");
    row(&mut o, "Model", opt(&c.model, ""));
    let topo = c
        .topology
        .map(|t| format!(" ({} P + {} E)", t.performance, t.efficiency))
        .unwrap_or_default();
    row(&mut o, "Threads", format!("{}{topo}", c.logical_cpus));
    row(&mut o, "Usage", metric(&c.usage_percent, "%", 0));
    row(&mut o, "Frequency", opt(&c.frequency_mhz, " MHz"));
    row(
        &mut o,
        "Range",
        format!(
            "{} - {}",
            opt(&c.min_frequency_mhz, " MHz"),
            opt(&c.max_frequency_mhz, " MHz")
        ),
    );
    row(
        &mut o,
        "Temperature",
        c.temperature_c
            .map(|t| format!("{t:.0} °C"))
            .unwrap_or("n/a".into()),
    );
    row(&mut o, "Package power", metric(&c.package_power_w, " W", 1));
    row(
        &mut o,
        "Driver",
        format!(
            "{}{}",
            opt(&c.scaling_driver, ""),
            c.pstate_status
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        ),
    );
    row(&mut o, "Governor", list(&c.governors));
    row(&mut o, "EPP", list(&c.epp));
    row(
        &mut o,
        "Turbo",
        match c.turbo {
            TurboState::Enabled => "enabled",
            TurboState::Disabled => "disabled",
            TurboState::Unknown => "unknown",
        },
    );
    for r in &c.rapl_constraints {
        row(
            &mut o,
            &format!("RAPL {}", r.name),
            format!(
                "{} W{}",
                r.limit_w.map(|w| format!("{w:.0}")).unwrap_or("?".into()),
                r.time_window_s
                    .map(|t| format!(" / {t:.3} s"))
                    .unwrap_or_default()
            ),
        );
    }
    o
}

/// `legionctl gpu status`.
pub fn gpu(g: &GpuInfo) -> String {
    let mut o = String::new();
    if g.devices.is_empty() {
        return format!("GPU:\n  {}\n", tr(NOT_SUPPORTED));
    }
    let _ = writeln!(o, "{}", tr(&format!("Graphics mode: {:?}", g.mode)));
    for d in &g.devices {
        let _ = writeln!(o, "\nGPU {}:", d.pci_address);
        row(&mut o, "Model", opt(&d.name, ""));
        row(
            &mut o,
            "Driver",
            format!(
                "{} {}",
                opt(&d.driver, ""),
                d.driver_version.clone().unwrap_or_default()
            )
            .trim_end(),
        );
        row(&mut o, "Power state", opt(&d.runtime_status, ""));
        row(
            &mut o,
            "Internal panel",
            match d.drives_internal_panel {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            },
        );
        match (&d.nvml, &d.metrics) {
            (NvmlStatus::Available, Some(m)) => {
                row(&mut o, "Temperature", opt(&m.temperature_c, " °C"));
                row(&mut o, "Utilization", opt(&m.utilization_percent, "%"));
                row(
                    &mut o,
                    "VRAM",
                    format!(
                        "{} / {} MiB",
                        opt(&m.memory_used_mib, ""),
                        opt(&m.memory_total_mib, "")
                    ),
                );
                row(&mut o, "Clock", opt(&m.graphics_clock_mhz, " MHz"));
                row(
                    &mut o,
                    "Power",
                    m.power_draw_w
                        .map(|w| format!("{w:.1} W"))
                        .unwrap_or("n/a".into()),
                );
                row(
                    &mut o,
                    "Power limit",
                    m.power_limit_w
                        .map(|w| format!("{w:.0} W"))
                        .unwrap_or("n/a".into()),
                );
                row(&mut o, "Fan", opt(&m.fan_speed_percent, "%"));
                row(&mut o, "P-state", opt(&m.performance_state, ""));
            }
            (NvmlStatus::SkippedSuspended, _) => row(
                &mut o,
                "Metrics",
                "GPU is suspended (saving power); use --wake-gpu to query NVML",
            ),
            (NvmlStatus::LibraryNotFound, _) => {
                row(&mut o, "Metrics", "NVML not found (install nvidia-utils)")
            }
            (NvmlStatus::NotApplicable, _) => {}
            (other, _) => row(&mut o, "Metrics", format!("{other:?}")),
        }
    }
    o
}

/// Short description of a fan mode.
pub fn fan_mode(m: &FanMode) -> String {
    legion_hardware::custom::fan_mode_name(m)
}

/// `legionctl fan status`.
pub fn fans(f: &FanInfo, daemon: Option<&DaemonInfo>) -> String {
    let mut o = head("Fans");
    match daemon {
        Some(d) => {
            row(&mut o, "Mode", fan_mode(&d.fan_mode));
            if let Some(ev) = &d.last_safety_event {
                row(&mut o, "Safety", format!("reverted to auto: {ev}"));
            }
        }
        None => row(
            &mut o,
            "Mode",
            "unknown (daemon not running; firmware default is auto)",
        ),
    }
    if f.fans.is_empty() {
        let _ = writeln!(o, "  {}", tr("No fan sensors found."));
    }
    for fan in &f.fans {
        let range = match (fan.min_rpm, fan.max_rpm) {
            (Some(a), Some(b)) => format!("  ({} {a}-{b})", tr("range")),
            _ => String::new(),
        };
        row(
            &mut o,
            &fan.label,
            format!("{} RPM{range}  [{}]", opt(&fan.rpm, ""), fan.chip),
        );
    }
    let _ = writeln!(o, "\n{}", tr(&f.note));
    o
}

/// `legionctl battery`.
pub fn battery(b: &BatteryInfo) -> String {
    let mut o = String::new();
    if b.batteries.is_empty() {
        o.push_str(&head("Battery"));
        let _ = writeln!(o, "  {}", tr("No system battery found."));
    }
    for bat in &b.batteries {
        let _ = writeln!(o, "{}", tr(&format!("Battery ({}):", bat.name)));
        row(&mut o, "Status", opt(&bat.status, ""));
        row(&mut o, "Capacity", opt(&bat.capacity_percent, "%"));
        row(
            &mut o,
            "Health",
            format!(
                "{:?}{}",
                bat.health,
                bat.health_percent
                    .map(|h| format!(" ({h:.1}% of design)"))
                    .unwrap_or_default()
            ),
        );
        row(&mut o, "Cycle count", opt(&bat.cycle_count, ""));
        row(
            &mut o,
            "Power",
            bat.power_w
                .map(|w| format!("{w:.1} W"))
                .unwrap_or("n/a".into()),
        );
        row(
            &mut o,
            "Energy",
            format!(
                "{} / {} Wh (design {})",
                bat.energy_now_wh
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or("?".into()),
                bat.energy_full_wh
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or("?".into()),
                bat.energy_full_design_wh
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or("?".into())
            ),
        );
        row(
            &mut o,
            "Model",
            format!(
                "{} {} {}",
                opt(&bat.manufacturer, ""),
                opt(&bat.model, ""),
                opt(&bat.technology, "")
            ),
        );
        if let Some(ct) = &bat.charge_types {
            row(
                &mut o,
                "Charge type",
                format!(
                    "{} (available: {})",
                    opt(&ct.active, ""),
                    ct.available.join(", ")
                ),
            );
        }
        row(
            &mut o,
            "Threshold",
            match bat.charge_end_threshold {
                Some(t) => format!("{t}%"),
                None => NOT_SUPPORTED.into(),
            },
        );
    }
    row(
        &mut o,
        "AC adapter",
        match b.ac_online {
            Some(true) => "online",
            Some(false) => "offline",
            None => "unknown",
        },
    );
    row(
        &mut o,
        "Conservation",
        match b.conservation_mode {
            Some(true) => "enabled (firmware charge limit)",
            Some(false) => "disabled",
            None => NOT_SUPPORTED,
        },
    );
    o
}

/// `legionctl thermal`.
pub fn thermal(t: &ThermalInfo) -> String {
    let mut o = head("Temperatures");
    let c = |v: Option<f64>| v.map(|v| format!("{v:.0} °C")).unwrap_or("n/a".into());
    row(&mut o, "CPU", c(t.cpu_c));
    row(
        &mut o,
        "GPU",
        t.gpu_c
            .map(|v| format!("{v:.0} °C"))
            .unwrap_or("n/a (dGPU suspended or no sensor)".into()),
    );
    row(&mut o, "SSD", c(t.storage_c));
    let _ = writeln!(o, "\n{}:", tr("All sensors"));
    let mut sensors = t.sensors.clone();
    sensors.sort_by(|a, b| a.category.cmp(&b.category).then(a.label.cmp(&b.label)));
    for s in &sensors {
        let _ = writeln!(
            o,
            "  {:<14} {:<28} {:>6.1} °C{}   [{}]",
            tr(&format!("{:?}", s.category)),
            s.label,
            s.celsius,
            s.critical_c
                .map(|c| format!(" (crit {c:.0})"))
                .unwrap_or_default(),
            s.source
        );
    }
    o
}

/// `legionctl status`.
pub fn status(s: &Status) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "{} — {}\n",
        s.system.display_product(),
        opt(&s.system.cpu_model, "")
    );
    row(&mut o, "Profile", s.profile.current.to_string());
    row(
        &mut o,
        "CPU",
        format!(
            "{}  {}  {}  {}",
            metric(&s.cpu.usage_percent, "%", 0),
            opt(&s.cpu.frequency_mhz, " MHz"),
            s.cpu
                .temperature_c
                .map(|t| format!("{t:.0} °C"))
                .unwrap_or("n/a".into()),
            match &s.cpu.package_power_w {
                Metric::Ok(w) => format!("{w:.1} W"),
                _ => String::new(),
            }
        ),
    );
    for d in s
        .gpu
        .devices
        .iter()
        .filter(|d| d.vendor == GpuVendor::Nvidia)
    {
        let v = match &d.metrics {
            Some(m) => format!(
                "{}  {}  {}  {}",
                opt(&m.utilization_percent, "%"),
                opt(&m.graphics_clock_mhz, " MHz"),
                opt(&m.temperature_c, " °C"),
                m.power_draw_w
                    .map(|w| format!("{w:.1} W"))
                    .unwrap_or_default()
            ),
            None => format!("{} (no metrics)", opt(&d.runtime_status, "")),
        };
        row(&mut o, "GPU", v);
    }
    row(&mut o, "Graphics mode", format!("{:?}", s.gpu.mode));
    row(
        &mut o,
        "Memory",
        format!("{} / {} MiB", s.memory.used_mib, s.memory.total_mib),
    );
    row(
        &mut o,
        "SSD",
        s.thermal
            .storage_c
            .map(|v| format!("{v:.0} °C"))
            .unwrap_or("n/a".into()),
    );
    let fans: Vec<String> = s
        .fans
        .fans
        .iter()
        .map(|f| format!("{} {}", f.label, opt(&f.rpm, " RPM")))
        .collect();
    row(
        &mut o,
        "Fans",
        if fans.is_empty() {
            NOT_SUPPORTED.into()
        } else {
            fans.join(", ")
        },
    );
    for b in &s.battery.batteries {
        let pct = b.capacity_percent.unwrap_or(0).min(100) as usize;
        let bar = format!("{}{}", "█".repeat(pct / 5), "░".repeat(20 - pct / 5));
        row(
            &mut o,
            "Battery",
            format!("{}% {bar} {}", pct, tr(b.status.as_deref().unwrap_or(""))),
        );
    }
    o
}

/// `legionctl capabilities`.
pub fn capabilities(caps: &[Capability]) -> String {
    let mut o = head("Capabilities");
    for c in caps {
        let st = match c.state {
            CapabilityState::Supported => paint(&pad(&tr("SUPPORTED"), 14), "32"),
            CapabilityState::ReadOnly => paint(&pad(&tr("READ-ONLY"), 14), "36"),
            CapabilityState::Detected => paint(&pad(&tr("DETECTED"), 14), "33"),
            CapabilityState::Unsupported => paint(&pad(&tr("UNSUPPORTED"), 14), "2"),
        };
        let _ = writeln!(o, "  {:<22} {} {}", c.id, st, tr(&c.note));
        if let Some(i) = &c.interface {
            let _ = writeln!(o, "  {:<22} {:<14} via {i}", "", "");
        }
    }
    let _ = writeln!(
        o,
        "\n{}\n{}",
        tr("SUPPORTED: can be changed.  READ-ONLY: monitored.  DETECTED: available but disabled"),
        tr("(experimental, see /etc/legion-control/daemon.json) or not documented.")
    );
    o
}

/// Left-pads by characters (colour codes must be added after padding).
fn pad(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

fn status_word(s: CheckStatus) -> String {
    let w = pad(&tr(&s.to_string()), 13);
    match s {
        CheckStatus::Pass => paint(&w, "32"),
        CheckStatus::Warn => paint(&w, "33"),
        CheckStatus::Fail => paint(&w, "31"),
        CheckStatus::Unsupported => paint(&w, "2"),
    }
}

/// `legionctl doctor`.
pub fn doctor(r: &DoctorReport) -> String {
    let mut o = format!("{}\n\n", tr("Legion Control doctor (read-only)"));
    for c in &r.checks {
        let _ = writeln!(
            o,
            "{:<30} {}  {}",
            tr(&c.name),
            status_word(c.status),
            tr(&c.detail)
        );
    }
    let _ = writeln!(
        o,
        "\n{}: {}",
        tr("Overall"),
        status_word(r.overall()).trim_end()
    );
    o
}

/// `legionctl daemon status`.
pub fn daemon(r: &crate::DaemonReport) -> String {
    let mut o = head("Daemon");
    row(&mut o, "Bus name", legion_dbus::BUS_NAME);
    match &r.info {
        Some(i) => {
            row(&mut o, "Running", format!("yes (version {})", i.version));
            row(
                &mut o,
                "Authorization",
                if i.polkit {
                    "polkit"
                } else {
                    "disabled (development)"
                },
            );
            row(&mut o, "Fan mode", fan_mode(&i.fan_mode));
            row(
                &mut o,
                "Fan control",
                if i.fan_control_enabled {
                    "enabled"
                } else {
                    "disabled (daemon.json)"
                },
            );
            row(
                &mut o,
                "Power limits",
                if i.power_limits_enabled {
                    "enabled"
                } else {
                    "disabled (daemon.json)"
                },
            );
            if let Some(ev) = &i.last_safety_event {
                row(&mut o, "Safety event", ev);
            }
        }
        None => {
            row(&mut o, "Running", "no");
            let _ = writeln!(
                o,
                "\n{}\n  sudo systemctl enable --now legion-control.service",
                tr("Reading works without the daemon. To change settings as a normal user:")
            );
        }
    }
    o
}

/// `legionctl power-limit`.
pub fn power_limits(p: &PowerLimits) -> String {
    let mut o = head("Firmware power / thermal limits");
    if p.attributes.is_empty() {
        return format!("{o}  {}\n", tr(NOT_SUPPORTED));
    }
    for a in &p.attributes {
        let range = match a.range() {
            Some((min, max, step)) => format!(
                "[{min}..{max}{}]",
                if step > 1 {
                    format!(" step {step}")
                } else {
                    String::new()
                }
            ),
            None => tr("(fixed)"),
        };
        let _ = writeln!(
            o,
            "  {:<18} {:>5}  {:<14} {}",
            a.name,
            a.current_value.as_deref().unwrap_or("?"),
            range,
            a.display_name.as_deref().unwrap_or("")
        );
    }
    let _ = writeln!(
        o,
        "\n{}",
        tr(&format!(
            "Custom profile active: {}. The firmware only applies these values in the custom \
             profile, and resets them when another profile is selected.",
            if p.custom_profile_active { "yes" } else { "no" }
        ))
    );
    o
}

/// `legionctl config`.
pub fn config(c: &legion_hardware::Config) -> String {
    let mut o = head("Configuration");
    let onoff = |b: bool| if b { "enabled" } else { "disabled" };
    row(&mut o, "enable_fan_control", onoff(c.enable_fan_control));
    row(&mut o, "enable_power_limits", onoff(c.enable_power_limits));
    row(
        &mut o,
        "fan_safety_cpu_c",
        format!("{:.0} °C", c.fan_safety_cpu_c),
    );
    row(
        &mut o,
        "fan_safety_gpu_c",
        format!("{:.0} °C", c.fan_safety_gpu_c),
    );
    let _ = writeln!(
        o,
        "\n{}",
        tr("Change with: legionctl config set <name> on|off")
    );
    o
}

/// `legionctl custom list`.
pub fn custom_list(dir: &std::path::Path, names: &[String]) -> String {
    let mut o = format!("{}\n", tr(&format!("Custom profiles ({}):", dir.display())));
    if names.is_empty() {
        let _ = writeln!(
            o,
            "  {}",
            tr("none — save one with `legionctl custom save <name>`")
        );
    }
    for n in names {
        let _ = writeln!(o, "  {n}");
    }
    o
}

/// Summary printed by `legionctl hardware-scan`.
pub fn scan_summary(r: &HardwareReport) -> String {
    let mut o = system(&r.system);
    let l = &r.acpi.lenovo;
    let _ = writeln!(
        o,
        "\n{}",
        tr(&format!(
            "ACPI: {} tables, {} devices; WMI: {} blocks ({} unbound)",
            r.acpi.tables.len(),
            r.acpi.devices.len(),
            r.acpi.wmi_devices.len(),
            l.unbound_wmi_guids.len()
        ))
    );
    let _ = writeln!(o, "{}:", tr("Lenovo drivers"));
    for d in &l.drivers {
        let _ = writeln!(o, "  {:<28} {}", d.driver, d.description);
    }
    for set in &l.firmware_attributes {
        let _ = writeln!(o, "{}", tr(&format!("Firmware attributes ({}):", set.name)));
        for a in &set.attributes {
            let _ = writeln!(
                o,
                "  {:<18} {:>5}  [{}..{}]  {}",
                a.name,
                a.current_value.as_deref().unwrap_or("?"),
                a.min_value.as_deref().unwrap_or("?"),
                a.max_value.as_deref().unwrap_or("?"),
                a.display_name.as_deref().unwrap_or("")
            );
        }
    }
    if !l.ideapad.is_empty() {
        let _ = writeln!(o, "ideapad-laptop (VPC2004):");
        for a in &l.ideapad {
            let _ = writeln!(o, "  {:<18} {}", a.name, a.value.as_deref().unwrap_or("?"));
        }
    }
    o.push('\n');
    o.push_str(&capabilities(&r.capabilities));
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::HardwareController;
    use legion_core::testing::MockController;

    /// Tests share the process-wide language; serialise them.
    static LANG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_lang(l: legion_i18n::Lang) -> std::sync::MutexGuard<'static, ()> {
        let g = LANG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        legion_i18n::set_lang(l);
        g
    }

    fn en() -> std::sync::MutexGuard<'static, ()> {
        with_lang(legion_i18n::Lang::En)
    }

    #[test]
    fn renders_unsupported_and_permission_messages() {
        let _g = en();
        let m = MockController::default();
        let mut cpu = m.get_cpu_info().unwrap();
        cpu.package_power_w = Metric::PermissionRequired;
        let t = self::cpu(&cpu);
        assert!(t.contains(NEEDS_ROOT));
        let b = battery(&BatteryInfo::default());
        assert!(b.contains("No system battery found."));
        assert!(b.contains(NOT_SUPPORTED));
        let f = fans(
            &FanInfo {
                note: "Manual fan control is not supported on this hardware.".into(),
                ..Default::default()
            },
            None,
        );
        assert!(f.contains("Manual fan control is not supported on this hardware."));
    }

    #[test]
    fn renders_doctor() {
        let _g = en();
        let r = DoctorReport {
            checks: vec![legion_core::doctor::CheckResult::new(
                "ACPI",
                CheckStatus::Pass,
                "ok",
            )],
        };
        let t = doctor(&r);
        assert!(t.contains("ACPI"));
        assert!(t.contains("PASS"));
    }

    #[test]
    fn renders_portuguese() {
        let _g = with_lang(legion_i18n::Lang::PtBr);
        let b = battery(&BatteryInfo::default());
        assert!(b.contains("Nenhuma bateria do sistema encontrada."), "{b}");
        assert!(b.contains("Carregador:"), "{b}");
        let r = DoctorReport {
            checks: vec![legion_core::doctor::CheckResult::new(
                "Fan control",
                CheckStatus::Unsupported,
                "Manual fan control is not supported on this hardware.",
            )],
        };
        let d = doctor(&r);
        assert!(d.contains("Controle das ventoinhas"), "{d}");
        assert!(d.contains("NÃO SUPORTADO"), "{d}");
    }

    #[test]
    fn renders_status_from_mock() {
        let _g = en();
        let m = MockController::default();
        let s = status(&m.get_status().unwrap());
        assert!(s.contains("Profile"));
        assert!(s.contains("8192 / 16384 MiB"));
    }
}
