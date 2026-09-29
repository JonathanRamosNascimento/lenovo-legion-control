//! Read-only battery monitoring (`/sys/class/power_supply`,
//! `Documentation/ABI/testing/sysfs-class-power`).
//!
//! Charge limiting on Lenovo laptops can appear in three different ways:
//!
//! * `charge_control_end_threshold` — numeric threshold (generic ABI);
//! * `charge_types` containing `Long_Life` — fixed conservation mode
//!   (generic ABI, used by `ideapad-laptop` on recent kernels);
//! * `conservation_mode` on the `VPC2004` platform device (`ideapad-laptop`).
//!
//! Writes (Phase 4) use only these documented attributes, after validating
//! the value against what the kernel advertises.

use legion_core::model::{Battery, BatteryHealth, BatteryInfo, ChargeTypes};
use legion_core::sysfs::{self, SysRoot};

const PSU: &str = "/sys/class/power_supply";

/// ideapad-laptop platform device holding `conservation_mode`.
pub const IDEAPAD_DEVICE: &str = "/sys/bus/platform/devices/VPC2004:00";

/// Classifies health from a full/design percentage.
pub fn classify_health(pct: Option<f64>) -> BatteryHealth {
    match pct {
        Some(p) if p >= 80.0 => BatteryHealth::Good,
        Some(p) if p >= 60.0 => BatteryHealth::Fair,
        Some(_) => BatteryHealth::Poor,
        None => BatteryHealth::Unknown,
    }
}

fn micro(root: &SysRoot, dir: &str, attr: &str) -> Option<f64> {
    root.read_parse_opt::<i64>(format!("{dir}/{attr}"))
        .map(|v| v as f64 / 1e6)
}

fn read_battery(root: &SysRoot, name: &str) -> Battery {
    let dir = format!("{PSU}/{name}");
    let voltage_v = micro(root, &dir, "voltage_now");
    let design_v = micro(root, &dir, "voltage_min_design").or(voltage_v);

    // Prefer energy_* (µWh). Fall back to charge_* (µAh) × voltage.
    let energy = |attr: &str| {
        micro(root, &dir, &format!("energy_{attr}"))
            .or_else(|| Some(micro(root, &dir, &format!("charge_{attr}"))? * design_v?))
    };
    let (now, full, design) = (energy("now"), energy("full"), energy("full_design"));
    let health_percent = match (full, design) {
        (Some(f), Some(d)) if d > 0.0 => Some((f / d * 1000.0).round() / 10.0),
        _ => None,
    };

    let power_w = micro(root, &dir, "power_now")
        .or_else(|| Some(micro(root, &dir, "current_now")? * voltage_v?))
        .map(f64::abs);

    let charge_types = root.read_opt(format!("{dir}/charge_types")).map(|raw| {
        let (available, active) = sysfs::parse_bracket_choices(&raw);
        ChargeTypes { available, active }
    });

    Battery {
        name: name.to_owned(),
        status: root.read_opt(format!("{dir}/status")),
        capacity_percent: root.read_parse_opt(format!("{dir}/capacity")),
        health_percent,
        health: classify_health(health_percent),
        cycle_count: root.read_parse_opt(format!("{dir}/cycle_count")),
        power_w,
        voltage_v,
        energy_now_wh: now,
        energy_full_wh: full,
        energy_full_design_wh: design,
        technology: root.read_opt(format!("{dir}/technology")),
        manufacturer: root.read_opt(format!("{dir}/manufacturer")),
        model: root.read_opt(format!("{dir}/model_name")),
        charge_types,
        charge_start_threshold: root
            .read_parse_opt(format!("{dir}/charge_control_start_threshold")),
        charge_end_threshold: root.read_parse_opt(format!("{dir}/charge_control_end_threshold")),
    }
}

/// Reads batteries, AC state and ideapad conservation mode.
///
/// `serial_number` is intentionally never read.
pub fn read_battery_info(root: &SysRoot) -> BatteryInfo {
    let mut ac_online = None;
    let mut batteries = Vec::new();
    for name in root.list(PSU) {
        match root.read_opt(format!("{PSU}/{name}/type")).as_deref() {
            Some("Battery") => {
                // Skip peripheral batteries (mice, headsets): scope=Device.
                if root.read_opt(format!("{PSU}/{name}/scope")).as_deref() == Some("Device") {
                    continue;
                }
                batteries.push(read_battery(root, &name));
            }
            Some("Mains") => {
                let online = root.read_opt(format!("{PSU}/{name}/online")).as_deref() == Some("1");
                ac_online = Some(ac_online.unwrap_or(false) || online);
            }
            _ => {}
        }
    }
    let conservation_mode = root
        .read_opt(format!("{IDEAPAD_DEVICE}/conservation_mode"))
        .and_then(|v| match v.as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        });
    BatteryInfo {
        ac_online,
        batteries,
        conservation_mode,
    }
}

fn first_battery(root: &SysRoot) -> legion_core::Result<String> {
    root.list(PSU)
        .into_iter()
        .find(|n| {
            root.read_opt(format!("{PSU}/{n}/type")).as_deref() == Some("Battery")
                && root.read_opt(format!("{PSU}/{n}/scope")).as_deref() != Some("Device")
        })
        .ok_or_else(|| legion_core::HwError::NotSupported("system battery".into()))
}

/// Enables or disables the firmware conservation mode.
///
/// Prefers ideapad-laptop `conservation_mode` (explicit on/off); falls back
/// to `charge_types` (`Long_Life` / `Standard`).
pub fn set_conservation(root: &SysRoot, enabled: bool) -> legion_core::Result<()> {
    let ideapad = format!("{IDEAPAD_DEVICE}/conservation_mode");
    if root.exists(&ideapad) {
        sysfs::write_attr(root, &ideapad, if enabled { "1" } else { "0" })?;
        return Ok(());
    }
    let bat = first_battery(root)?;
    let raw = root
        .read_opt(format!("{PSU}/{bat}/charge_types"))
        .ok_or_else(|| legion_core::HwError::NotSupported("battery conservation mode".into()))?;
    let (available, _) = sysfs::parse_bracket_choices(&raw);
    let target = if enabled { "Long_Life" } else { "Standard" };
    if !available.iter().any(|a| a == target) {
        return Err(legion_core::HwError::NotSupported(format!(
            "charge type {target}"
        )));
    }
    sysfs::write_attr(root, &format!("{PSU}/{bat}/charge_types"), target)?;
    Ok(())
}

/// Selects one of the advertised `charge_types` values.
pub fn set_charge_type(root: &SysRoot, value: &str) -> legion_core::Result<()> {
    let bat = first_battery(root)?;
    let path = format!("{PSU}/{bat}/charge_types");
    let raw = root
        .read_opt(&path)
        .ok_or_else(|| legion_core::HwError::NotSupported("battery charge types".into()))?;
    let (available, _) = sysfs::parse_bracket_choices(&raw);
    if !available.iter().any(|a| a == value) {
        return Err(legion_core::HwError::InvalidValue(format!(
            "charge type {value:?} is not one of: {}",
            available.join(", ")
        )));
    }
    sysfs::write_attr(root, &path, value)?;
    Ok(())
}

/// Sets `charge_control_end_threshold` (and never below the start threshold).
pub fn set_charge_threshold(root: &SysRoot, end: u8) -> legion_core::Result<()> {
    let bat = first_battery(root)?;
    let path = format!("{PSU}/{bat}/charge_control_end_threshold");
    if !root.exists(&path) {
        return Err(legion_core::HwError::NotSupported(
            "Battery charge threshold is not supported on this hardware.".into(),
        ));
    }
    let start: u8 = root
        .read_parse_opt(format!("{PSU}/{bat}/charge_control_start_threshold"))
        .unwrap_or(0);
    validate_threshold(end, start.saturating_add(1).max(1))?;
    sysfs::write_attr(root, &path, &end.to_string())?;
    Ok(())
}

/// Validates a charge threshold.
///
/// Accepts only integers within `[min, 100]`.
pub fn validate_threshold(value: u8, min: u8) -> legion_core::Result<u8> {
    if (min..=100).contains(&value) {
        Ok(value)
    } else {
        Err(legion_core::HwError::InvalidValue(format!(
            "charge threshold must be between {min} and 100, got {value}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    fn legion() -> FakeSysfs {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/power_supply/ADP0/type", "Mains")
            .file("/sys/class/power_supply/ADP0/online", "1")
            .file("/sys/class/power_supply/BAT0/type", "Battery")
            .file("/sys/class/power_supply/BAT0/status", "Not charging")
            .file("/sys/class/power_supply/BAT0/capacity", "75")
            .file("/sys/class/power_supply/BAT0/cycle_count", "55")
            .file("/sys/class/power_supply/BAT0/power_now", "0")
            .file("/sys/class/power_supply/BAT0/voltage_now", "16352000")
            .file("/sys/class/power_supply/BAT0/energy_now", "61500000")
            .file("/sys/class/power_supply/BAT0/energy_full", "81990000")
            .file(
                "/sys/class/power_supply/BAT0/energy_full_design",
                "80000000",
            )
            .file("/sys/class/power_supply/BAT0/technology", "Li-poly")
            .file("/sys/class/power_supply/BAT0/manufacturer", "BYD")
            .file("/sys/class/power_supply/BAT0/model_name", "L24B4PC0")
            .file("/sys/class/power_supply/BAT0/serial_number", "SECRET")
            .file(
                "/sys/class/power_supply/BAT0/charge_types",
                "Fast Standard [Long_Life]",
            )
            .file(
                "/sys/class/power_supply/ucsi-source-psy-USBC000:001/type",
                "USB",
            )
            .file(
                "/sys/bus/platform/devices/VPC2004:00/conservation_mode",
                "1",
            );
        fs
    }

    #[test]
    fn reads_legion_battery() {
        let fs = legion();
        let info = read_battery_info(&fs.root());
        assert_eq!(info.ac_online, Some(true));
        assert_eq!(info.conservation_mode, Some(true));
        assert_eq!(info.batteries.len(), 1);
        let b = &info.batteries[0];
        assert_eq!(b.capacity_percent, Some(75));
        assert_eq!(b.cycle_count, Some(55));
        assert_eq!(b.health_percent, Some(102.5));
        assert_eq!(b.health, BatteryHealth::Good);
        assert_eq!(b.energy_now_wh, Some(61.5));
        assert_eq!(b.power_w, Some(0.0));
        let ct = b.charge_types.as_ref().unwrap();
        assert_eq!(ct.active.as_deref(), Some("Long_Life"));
        assert_eq!(b.charge_end_threshold, None);
        // The serial number must never be collected.
        assert!(!format!("{info:?}").contains("SECRET"));
    }

    #[test]
    fn charge_based_battery() {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/power_supply/BAT1/type", "Battery")
            .file(
                "/sys/class/power_supply/BAT1/voltage_min_design",
                "15000000",
            )
            .file("/sys/class/power_supply/BAT1/voltage_now", "16000000")
            .file("/sys/class/power_supply/BAT1/charge_full", "3000000")
            .file("/sys/class/power_supply/BAT1/charge_full_design", "5000000")
            .file("/sys/class/power_supply/BAT1/current_now", "-1000000")
            .file(
                "/sys/class/power_supply/BAT1/charge_control_end_threshold",
                "80",
            )
            .file("/sys/class/power_supply/hidpp_battery_0/type", "Battery")
            .file("/sys/class/power_supply/hidpp_battery_0/scope", "Device");
        let info = read_battery_info(&fs.root());
        assert_eq!(info.batteries.len(), 1);
        let b = &info.batteries[0];
        assert_eq!(b.energy_full_wh, Some(45.0));
        assert_eq!(b.health_percent, Some(60.0));
        assert_eq!(b.health, BatteryHealth::Fair);
        assert_eq!(b.power_w, Some(16.0));
        assert_eq!(b.charge_end_threshold, Some(80));
        assert_eq!(info.ac_online, None);
        assert_eq!(info.conservation_mode, None);
    }

    #[test]
    fn conservation_and_charge_type_writes() {
        let fs = legion();
        let root = fs.root();
        set_conservation(&root, false).unwrap();
        assert_eq!(read_battery_info(&root).conservation_mode, Some(false));
        set_charge_type(&root, "Fast").unwrap();
        assert!(set_charge_type(&root, "Turbo").is_err());
        assert!(set_charge_threshold(&root, 80).is_err());
    }

    #[test]
    fn conservation_via_charge_types() {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/power_supply/BAT0/type", "Battery")
            .file(
                "/sys/class/power_supply/BAT0/charge_types",
                "Fast [Standard] Long_Life",
            );
        set_conservation(&fs.root(), true).unwrap();
        assert_eq!(
            fs.root()
                .read("/sys/class/power_supply/BAT0/charge_types")
                .unwrap(),
            "Long_Life"
        );
    }

    #[test]
    fn threshold_write() {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/power_supply/BAT0/type", "Battery")
            .file(
                "/sys/class/power_supply/BAT0/charge_control_start_threshold",
                "40",
            )
            .file(
                "/sys/class/power_supply/BAT0/charge_control_end_threshold",
                "100",
            );
        let root = fs.root();
        set_charge_threshold(&root, 80).unwrap();
        assert_eq!(
            read_battery_info(&root).batteries[0].charge_end_threshold,
            Some(80)
        );
        assert!(set_charge_threshold(&root, 30).is_err());
        assert!(set_charge_threshold(&root, 101).is_err());
    }

    #[test]
    fn health_classes() {
        assert_eq!(classify_health(Some(95.0)), BatteryHealth::Good);
        assert_eq!(classify_health(Some(59.9)), BatteryHealth::Poor);
        assert_eq!(classify_health(None), BatteryHealth::Unknown);
    }

    #[test]
    fn threshold_validation() {
        assert_eq!(validate_threshold(80, 50).unwrap(), 80);
        assert!(validate_threshold(101, 50).is_err());
        assert!(validate_threshold(20, 50).is_err());
        assert!(validate_threshold(100, 0).is_ok());
    }
}
