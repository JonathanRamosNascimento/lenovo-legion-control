//! `legionctl hardware-scan`: a JSON report of everything discovered.
//!
//! The report is meant to be attached to bug reports and used to extend the
//! support matrix. It contains no serial numbers, UUIDs, MAC addresses or
//! user data.

use std::collections::BTreeMap;

use serde::Serialize;

use legion_acpi::AcpiReport;
use legion_core::capabilities::{Capability, HardwareCapabilities};
use legion_core::doctor::DoctorReport;
use legion_core::hwmon;
use legion_core::model::{
    BatteryInfo, CpuInfo, FanInfo, GpuInfo, MemoryInfo, SystemInfo, ThermalInfo,
};
use legion_core::profile::PowerProfileState;
use legion_core::sysfs::SysRoot;

use crate::capabilities::{Inventory, from_inventory, gather};
use crate::controller::LinuxController;

/// Report format version.
pub const SCHEMA_VERSION: u32 = 1;

/// One hwmon chip with its sensor attributes.
#[derive(Debug, Clone, Serialize)]
pub struct HwmonDump {
    /// `hwmonN`.
    pub id: String,
    /// Chip name.
    pub name: String,
    /// Driver of the parent device.
    pub driver: Option<String>,
    /// Sensor attributes (`temp1_input` → `65000`).
    pub attributes: BTreeMap<String, String>,
}

/// The full report.
#[derive(Debug, Clone, Serialize)]
pub struct HardwareReport {
    /// Format version.
    pub schema_version: u32,
    /// Tool name and version.
    pub generated_by: String,
    /// Unix timestamp.
    pub generated_at_unix: u64,
    /// `sys_vendor`.
    pub manufacturer: Option<String>,
    /// Marketing product name.
    pub product: Option<String>,
    /// Machine type.
    pub machine_type: Option<String>,
    /// Board name.
    pub board: Option<String>,
    /// CPU model.
    pub cpu: Option<String>,
    /// Summary feature flags (interface detected).
    pub features: HardwareCapabilities,
    /// Detailed capabilities with evidence.
    pub capabilities: Vec<Capability>,
    /// DMI / OS details.
    pub system: SystemInfo,
    /// ACPI, WMI and Lenovo interfaces.
    pub acpi: AcpiReport,
    /// Power profiles.
    pub power_profiles: PowerProfileState,
    /// hwmon dump.
    pub hwmon: Vec<HwmonDump>,
    /// Temperatures.
    pub thermal: ThermalInfo,
    /// Fans.
    pub fans: FanInfo,
    /// Battery.
    pub battery: BatteryInfo,
    /// GPU.
    pub gpu: GpuInfo,
    /// CPU.
    pub cpu_info: CpuInfo,
    /// Memory.
    pub memory: Option<MemoryInfo>,
    /// Doctor results.
    pub doctor: DoctorReport,
}

/// hwmon attributes worth recording. Only standard sensor attributes.
fn is_sensor_attr(name: &str) -> bool {
    const KINDS: [&str; 7] = ["temp", "fan", "in", "curr", "power", "energy", "pwm"];
    const SUFFIXES: [&str; 11] = [
        "_input", "_label", "_min", "_max", "_crit", "_target", "_div", "_enable", "_mode",
        "_average", "",
    ];
    KINDS.iter().any(|k| {
        name.strip_prefix(k).is_some_and(|rest| {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            digits > 0 && SUFFIXES.contains(&&rest[digits..])
        })
    })
}

/// Dumps hwmon sensor attributes.
pub fn dump_hwmon(root: &SysRoot) -> Vec<HwmonDump> {
    hwmon::chips(root)
        .into_iter()
        .map(|c| {
            let attributes = legion_core::sysfs::list_dir(&c.path)
                .into_iter()
                .filter(|a| is_sensor_attr(a))
                .filter_map(|a| Some((a.clone(), c.read(&a)?)))
                .collect();
            HwmonDump {
                id: c.id,
                name: c.name,
                driver: c.driver,
                attributes,
            }
        })
        .collect()
}

/// Builds the report from an inventory.
pub fn build_with(ctrl: &LinuxController, inv: Inventory) -> HardwareReport {
    let capabilities = from_inventory(&inv);
    let doctor = crate::doctor::run_with(ctrl, &inv);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    HardwareReport {
        schema_version: SCHEMA_VERSION,
        generated_by: format!("legion-control {}", env!("CARGO_PKG_VERSION")),
        generated_at_unix: now,
        manufacturer: inv.system.manufacturer.clone(),
        product: inv.system.product_version.clone(),
        machine_type: inv.system.product_name.clone(),
        board: inv.system.board_name.clone(),
        cpu: inv.system.cpu_model.clone(),
        features: HardwareCapabilities::from_list(&capabilities),
        capabilities,
        hwmon: dump_hwmon(ctrl.root()),
        system: inv.system,
        acpi: inv.acpi,
        power_profiles: inv.profile,
        thermal: inv.thermal,
        fans: inv.fans,
        battery: inv.battery,
        gpu: inv.gpu,
        cpu_info: inv.cpu,
        memory: inv.memory,
        doctor,
    }
}

/// Gathers everything and builds the report.
pub fn build(ctrl: &LinuxController) -> HardwareReport {
    let inv = gather(ctrl);
    build_with(ctrl, inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_attr_filter() {
        for ok in [
            "temp1_input",
            "fan2_target",
            "in0_input",
            "pwm1",
            "pwm1_enable",
            "temp10_crit",
        ] {
            assert!(is_sensor_attr(ok), "{ok}");
        }
        for bad in [
            "name",
            "uevent",
            "temp_input",
            "device",
            "fanX_input",
            "temp1_crit_hyst_x",
        ] {
            assert!(!is_sensor_attr(bad), "{bad}");
        }
    }
}
