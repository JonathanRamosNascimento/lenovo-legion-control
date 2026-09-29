//! Temperature discovery from `/sys/class/thermal` and hwmon.
//!
//! Sensors are only categorised when the source is unambiguous (e.g. hwmon
//! `coretemp` → CPU, `nvme` → storage). ACPI thermal zones such as `SEN1`
//! have no documented physical location, so they are reported as
//! [`SensorCategory::Acpi`] with their raw name instead of guessing
//! ("motherboard", "VRM", ...).

use legion_core::hwmon;
use legion_core::model::{SensorCategory, TemperatureReading, ThermalInfo};
use legion_core::sysfs::SysRoot;

const THERMAL_CLASS: &str = "/sys/class/thermal";

/// Plausible range for a temperature sensor; anything outside is treated as
/// a bogus reading (e.g. -273 °C or 255 °C placeholders).
const PLAUSIBLE_C: std::ops::RangeInclusive<f64> = -40.0..=150.0;

/// Category for an hwmon chip name.
pub fn category_for_chip(name: &str) -> SensorCategory {
    match name {
        "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" => SensorCategory::Cpu,
        "nvme" | "drivetemp" => SensorCategory::Storage,
        "amdgpu" | "nouveau" | "radeon" | "xe" | "i915" => SensorCategory::Gpu,
        n if n.starts_with("mt79") || n.starts_with("iwlwifi") || n.starts_with("ath") => {
            SensorCategory::Wireless
        }
        n if n.starts_with("BAT") => SensorCategory::Battery,
        "acpitz" => SensorCategory::Acpi,
        _ => SensorCategory::Other,
    }
}

/// Category for a thermal zone `type`.
pub fn category_for_zone(kind: &str) -> SensorCategory {
    match kind {
        "x86_pkg_temp" | "TCPU" | "TCPU_PCI" | "cpu-thermal" => SensorCategory::Cpu,
        "iwlwifi_1" => SensorCategory::Wireless,
        k if k.starts_with("SEN") || k.starts_with("TZ") || k == "acpitz" => SensorCategory::Acpi,
        _ => SensorCategory::Other,
    }
}

fn hwmon_readings(root: &SysRoot) -> Vec<TemperatureReading> {
    let mut out = Vec::new();
    for chip in hwmon::chips(root) {
        let category = category_for_chip(&chip.name);
        for n in chip.channels("temp") {
            let Some(c) = chip.temp_c(n) else { continue };
            if !PLAUSIBLE_C.contains(&c) {
                continue;
            }
            let label = match chip.label("temp", n) {
                Some(l) => format!("{} {}", chip.name, l),
                None => format!("{} temp{n}", chip.name),
            };
            out.push(TemperatureReading {
                label,
                category,
                celsius: c,
                critical_c: chip.temp_limit_c(n, "crit"),
                source: chip.source(&format!("temp{n}")),
            });
        }
    }
    out
}

fn zone_readings(root: &SysRoot) -> Vec<TemperatureReading> {
    root.list(THERMAL_CLASS)
        .into_iter()
        .filter(|z| z.starts_with("thermal_zone"))
        .filter_map(|z| {
            let dir = format!("{THERMAL_CLASS}/{z}");
            let kind = root.read_opt(format!("{dir}/type"))?;
            let c = root.read_parse_opt::<i64>(format!("{dir}/temp"))? as f64 / 1000.0;
            if !PLAUSIBLE_C.contains(&c) {
                return None;
            }
            let critical_c = critical_trip(root, &dir);
            Some(TemperatureReading {
                label: kind.clone(),
                category: category_for_zone(&kind),
                celsius: c,
                critical_c,
                source: z,
            })
        })
        .collect()
}

fn critical_trip(root: &SysRoot, dir: &str) -> Option<f64> {
    (0..16).find_map(|i| {
        let kind = root.read_opt(format!("{dir}/trip_point_{i}_type"))?;
        (kind == "critical")
            .then(|| root.read_parse_opt::<i64>(format!("{dir}/trip_point_{i}_temp")))
            .flatten()
            .map(|m| m as f64 / 1000.0)
    })
}

/// Picks the CPU package temperature from a sensor list.
pub fn cpu_package(sensors: &[TemperatureReading]) -> Option<f64> {
    let prefer = [
        "coretemp Package id 0",
        "k10temp Tctl",
        "zenpower Tctl",
        "x86_pkg_temp",
    ];
    prefer
        .iter()
        .find_map(|p| sensors.iter().find(|s| s.label == *p))
        .map(|s| s.celsius)
}

/// Reads every temperature sensor. `gpu_c` must be supplied by the caller
/// (NVIDIA GPUs expose temperature through NVML, not hwmon).
pub fn read_thermal(root: &SysRoot, gpu: Option<(String, f64)>) -> ThermalInfo {
    let mut sensors = hwmon_readings(root);
    sensors.extend(zone_readings(root));
    if let Some((name, c)) = &gpu {
        sensors.push(TemperatureReading {
            label: name.clone(),
            category: SensorCategory::Gpu,
            celsius: *c,
            critical_c: None,
            source: "nvml".into(),
        });
    }
    let storage_c = sensors
        .iter()
        .filter(|s| s.category == SensorCategory::Storage)
        .map(|s| s.celsius)
        .reduce(f64::max);
    let gpu_c = gpu.map(|g| g.1).or_else(|| {
        sensors
            .iter()
            .find(|s| s.category == SensorCategory::Gpu)
            .map(|s| s.celsius)
    });
    ThermalInfo {
        cpu_c: cpu_package(&sensors),
        gpu_c,
        storage_c,
        sensors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    fn fake() -> FakeSysfs {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/hwmon/hwmon4/name", "coretemp")
            .file("/sys/class/hwmon/hwmon4/temp1_input", "65000")
            .file("/sys/class/hwmon/hwmon4/temp1_label", "Package id 0")
            .file("/sys/class/hwmon/hwmon4/temp1_crit", "105000")
            .file("/sys/class/hwmon/hwmon2/name", "nvme")
            .file("/sys/class/hwmon/hwmon2/temp1_input", "42850")
            .file("/sys/class/hwmon/hwmon2/temp1_label", "Composite")
            .file("/sys/class/hwmon/hwmon2/temp2_input", "51850")
            .file("/sys/class/hwmon/hwmon2/temp2_label", "Sensor 1")
            .file("/sys/class/hwmon/hwmon6/name", "mt7925_phy0")
            .file("/sys/class/hwmon/hwmon6/temp1_input", "-273150")
            .file("/sys/class/thermal/thermal_zone1/type", "SEN1")
            .file("/sys/class/thermal/thermal_zone1/temp", "53050")
            .file(
                "/sys/class/thermal/thermal_zone1/trip_point_0_type",
                "critical",
            )
            .file(
                "/sys/class/thermal/thermal_zone1/trip_point_0_temp",
                "110000",
            )
            .file("/sys/class/thermal/thermal_zone10/type", "x86_pkg_temp")
            .file("/sys/class/thermal/thermal_zone10/temp", "68000")
            .dir("/sys/class/thermal/cooling_device0");
        fs
    }

    #[test]
    fn reads_and_categorises() {
        let fs = fake();
        let t = read_thermal(&fs.root(), Some(("NVIDIA GPU".into(), 61.0)));
        assert_eq!(t.cpu_c, Some(65.0));
        assert_eq!(t.gpu_c, Some(61.0));
        assert_eq!(t.storage_c, Some(51.85));
        // bogus wireless reading filtered out
        assert!(
            !t.sensors
                .iter()
                .any(|s| s.category == SensorCategory::Wireless)
        );
        let sen1 = t.sensors.iter().find(|s| s.label == "SEN1").unwrap();
        assert_eq!(sen1.category, SensorCategory::Acpi);
        assert_eq!(sen1.critical_c, Some(110.0));
        let pkg = t
            .sensors
            .iter()
            .find(|s| s.source == "hwmon:coretemp/temp1")
            .unwrap();
        assert_eq!(pkg.critical_c, Some(105.0));
    }

    #[test]
    fn falls_back_to_pkg_zone() {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/thermal/thermal_zone0/type", "x86_pkg_temp")
            .file("/sys/class/thermal/thermal_zone0/temp", "70000");
        let t = read_thermal(&fs.root(), None);
        assert_eq!(t.cpu_c, Some(70.0));
        assert_eq!(t.gpu_c, None);
    }

    #[test]
    fn categories() {
        assert_eq!(category_for_chip("coretemp"), SensorCategory::Cpu);
        assert_eq!(category_for_chip("mt7925_phy0"), SensorCategory::Wireless);
        assert_eq!(category_for_chip("lenovo_wmi_other"), SensorCategory::Other);
        assert_eq!(category_for_zone("SEN7"), SensorCategory::Acpi);
        assert_eq!(category_for_zone("INT3400 Thermal"), SensorCategory::Other);
    }
}
