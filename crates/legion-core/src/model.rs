//! Data model shared between the hardware crates, the CLI and (later) the
//! daemon / D-Bus interface. Every type is serialisable to JSON.

use serde::{Deserialize, Serialize};

use crate::profile::PowerProfileState;

/// A value that may be unavailable for a well-defined reason.
///
/// Used for metrics where the UX must distinguish "not supported" from
/// "needs root" (see the UX rules in the project plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Metric<T> {
    /// The value was read successfully.
    Ok(T),
    /// No interface exists for this value.
    NotSupported,
    /// The interface exists but requires administrator permission.
    PermissionRequired,
    /// The value is temporarily unavailable (reason included).
    Unavailable(String),
}

impl<T> Metric<T> {
    /// Returns the value if present.
    pub fn value(&self) -> Option<&T> {
        match self {
            Metric::Ok(v) => Some(v),
            _ => None,
        }
    }

    /// Returns `true` if a value is present.
    pub fn is_ok(&self) -> bool {
        matches!(self, Metric::Ok(_))
    }

    /// Maps the contained value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Metric<U> {
        match self {
            Metric::Ok(v) => Metric::Ok(f(v)),
            Metric::NotSupported => Metric::NotSupported,
            Metric::PermissionRequired => Metric::PermissionRequired,
            Metric::Unavailable(r) => Metric::Unavailable(r),
        }
    }
}

impl<T> From<Option<T>> for Metric<T> {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(v) => Metric::Ok(v),
            None => Metric::NotSupported,
        }
    }
}

/// General system identification (DMI, OS, kernel, desktop).
///
/// Serial numbers and UUIDs are deliberately **not** collected.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemInfo {
    /// `sys_vendor` (e.g. `LENOVO`).
    pub manufacturer: Option<String>,
    /// `product_name` — on Lenovo this is the machine type (e.g. `83F0`).
    pub product_name: Option<String>,
    /// `product_version` — on Lenovo this is the marketing name
    /// (e.g. `Legion 5 15IAX10`).
    pub product_version: Option<String>,
    /// `product_family`.
    pub product_family: Option<String>,
    /// `board_vendor`.
    pub board_vendor: Option<String>,
    /// `board_name` (e.g. `LNVNB161216`).
    pub board_name: Option<String>,
    /// `bios_vendor`.
    pub bios_vendor: Option<String>,
    /// `bios_version` (e.g. `S2CN26WW`).
    pub bios_version: Option<String>,
    /// `bios_date`.
    pub bios_date: Option<String>,
    /// `ec_firmware_release`, when exposed.
    pub ec_firmware_release: Option<String>,
    /// CPU model string.
    pub cpu_model: Option<String>,
    /// Names of detected GPUs.
    pub gpus: Vec<String>,
    /// Total RAM in MiB.
    pub memory_total_mib: Option<u64>,
    /// Kernel release (`uname -r` equivalent).
    pub kernel: Option<String>,
    /// OS pretty name from `os-release`.
    pub os_name: Option<String>,
    /// OS id from `os-release` (e.g. `cachyos`).
    pub os_id: Option<String>,
    /// Desktop environment (from `XDG_CURRENT_DESKTOP`).
    pub desktop: Option<String>,
    /// Display name of the model in the support matrix, when it is known.
    pub known_model: Option<String>,
}

impl SystemInfo {
    /// Human friendly product name, e.g. `Legion 5 15IAX10 (83F0)`.
    pub fn display_product(&self) -> String {
        match (&self.product_version, &self.product_name) {
            (Some(v), Some(n)) if v != n => format!("{v} ({n})"),
            (Some(v), _) => v.clone(),
            (None, Some(n)) => n.clone(),
            (None, None) => "unknown".into(),
        }
    }
}

/// Turbo/boost state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurboState {
    /// Turbo boost enabled.
    Enabled,
    /// Turbo boost disabled.
    Disabled,
    /// No interface found.
    Unknown,
}

/// Hybrid core topology (Intel P-cores/E-cores), when the kernel exposes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreTopology {
    /// Logical CPUs of type "core" (performance).
    pub performance: usize,
    /// Logical CPUs of type "atom" (efficiency).
    pub efficiency: usize,
}

/// RAPL power limits (read-only view).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RaplConstraint {
    /// Constraint name (`long_term`, `short_term`, ...).
    pub name: String,
    /// Power limit in watts.
    pub limit_w: Option<f64>,
    /// Time window in seconds.
    pub time_window_s: Option<f64>,
}

/// CPU status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CpuInfo {
    /// Model name.
    pub model: Option<String>,
    /// Vendor id (`GenuineIntel`, `AuthenticAMD`).
    pub vendor: Option<String>,
    /// Number of logical CPUs.
    pub logical_cpus: usize,
    /// P-core / E-core split, if hybrid.
    pub topology: Option<CoreTopology>,
    /// Total utilisation in percent over the sampling interval.
    pub usage_percent: Metric<f64>,
    /// Average current frequency across online CPUs, MHz.
    pub frequency_mhz: Option<u32>,
    /// Highest `cpuinfo_max_freq`, MHz.
    pub max_frequency_mhz: Option<u32>,
    /// Lowest `cpuinfo_min_freq`, MHz.
    pub min_frequency_mhz: Option<u32>,
    /// Package temperature, °C.
    pub temperature_c: Option<f64>,
    /// cpufreq scaling driver (e.g. `intel_pstate`).
    pub scaling_driver: Option<String>,
    /// `intel_pstate` mode (`active`, `passive`), if any.
    pub pstate_status: Option<String>,
    /// Current governor(s). More than one entry means policies differ.
    pub governors: Vec<String>,
    /// Available governors.
    pub available_governors: Vec<String>,
    /// Current energy performance preference(s).
    pub epp: Vec<String>,
    /// Available EPP values.
    pub available_epp: Vec<String>,
    /// Turbo state.
    pub turbo: TurboState,
    /// Package power (RAPL) in watts over the sampling interval.
    pub package_power_w: Metric<f64>,
    /// RAPL package constraints (power limits), read-only.
    pub rapl_constraints: Vec<RaplConstraint>,
}

/// GPU vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    /// NVIDIA.
    Nvidia,
    /// Intel.
    Intel,
    /// AMD.
    Amd,
    /// Anything else.
    Other,
}

/// Metrics read via NVML.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NvidiaMetrics {
    /// Name reported by NVML.
    pub name: Option<String>,
    /// Core temperature, °C.
    pub temperature_c: Option<u32>,
    /// GPU utilisation, %.
    pub utilization_percent: Option<u32>,
    /// Memory controller utilisation, %.
    pub memory_utilization_percent: Option<u32>,
    /// VRAM used, MiB.
    pub memory_used_mib: Option<u64>,
    /// VRAM total, MiB.
    pub memory_total_mib: Option<u64>,
    /// Graphics clock, MHz.
    pub graphics_clock_mhz: Option<u32>,
    /// Memory clock, MHz.
    pub memory_clock_mhz: Option<u32>,
    /// Power draw, W.
    pub power_draw_w: Option<f64>,
    /// Enforced power limit, W.
    pub power_limit_w: Option<f64>,
    /// Fan speed, % (laptops usually do not report this through NVML).
    pub fan_speed_percent: Option<u32>,
    /// Performance state (P0..P12).
    pub performance_state: Option<String>,
    /// Driver version reported by NVML.
    pub driver_version: Option<String>,
}

/// Why NVML metrics are or are not present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum NvmlStatus {
    /// Metrics were collected.
    Available,
    /// NVML was not queried because the GPU is runtime-suspended and waking
    /// it would cost battery. Use `--wake-gpu` to force.
    SkippedSuspended,
    /// NVML was not queried (disabled by configuration, e.g. fixtures).
    Disabled,
    /// `libnvidia-ml.so` could not be loaded.
    LibraryNotFound,
    /// NVML returned an error.
    Error(String),
    /// Not an NVIDIA GPU.
    NotApplicable,
}

/// One display controller on the PCI bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuDevice {
    /// PCI address (`0000:02:00.0`).
    pub pci_address: String,
    /// Vendor.
    pub vendor: GpuVendor,
    /// PCI vendor id (hex, e.g. `10de`).
    pub vendor_id: String,
    /// PCI device id (hex).
    pub device_id: String,
    /// Marketing name when it could be determined.
    pub name: Option<String>,
    /// Bound kernel driver.
    pub driver: Option<String>,
    /// Runtime PM status (`active`, `suspended`, ...).
    pub runtime_status: Option<String>,
    /// `true` if this is the boot VGA device (usually the iGPU on hybrids).
    pub boot_vga: Option<bool>,
    /// Whether this GPU's DRM card has a connected internal panel (eDP/LVDS).
    pub drives_internal_panel: Option<bool>,
    /// Driver version (NVIDIA: from `/proc/driver/nvidia/version`).
    pub driver_version: Option<String>,
    /// NVML status.
    pub nvml: NvmlStatus,
    /// NVML metrics, when available.
    pub metrics: Option<NvidiaMetrics>,
}

/// Graphics mode, inferred from which GPU drives the internal panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuMode {
    /// Internal panel on the iGPU, dGPU available for offload.
    Hybrid,
    /// Internal panel driven by the dGPU (MUX switched / dGPU-only).
    Discrete,
    /// Only an integrated GPU is present.
    IntegratedOnly,
    /// Could not be determined.
    Unknown,
}

/// GPU status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// All display controllers.
    pub devices: Vec<GpuDevice>,
    /// Inferred graphics mode.
    pub mode: GpuMode,
}

/// Memory status (from `/proc/meminfo`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryInfo {
    /// Total RAM, MiB.
    pub total_mib: u64,
    /// Available RAM, MiB.
    pub available_mib: u64,
    /// Used RAM (total - available), MiB.
    pub used_mib: u64,
    /// Total swap, MiB.
    pub swap_total_mib: u64,
    /// Free swap, MiB.
    pub swap_free_mib: u64,
}

/// Sensor category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorCategory {
    /// CPU package / cores.
    Cpu,
    /// GPU.
    Gpu,
    /// NVMe / SATA storage.
    Storage,
    /// Wireless card.
    Wireless,
    /// Battery.
    Battery,
    /// ACPI thermal zone whose physical location is not documented.
    Acpi,
    /// Anything else.
    Other,
}

/// One temperature reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemperatureReading {
    /// Human label.
    pub label: String,
    /// Category.
    pub category: SensorCategory,
    /// Temperature, °C.
    pub celsius: f64,
    /// Critical threshold, °C.
    pub critical_c: Option<f64>,
    /// Where it was read from (e.g. `hwmon:coretemp/temp1`, `thermal_zone8`).
    pub source: String,
}

/// Temperature summary + full list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThermalInfo {
    /// CPU package temperature.
    pub cpu_c: Option<f64>,
    /// GPU temperature.
    pub gpu_c: Option<f64>,
    /// Hottest storage sensor.
    pub storage_c: Option<f64>,
    /// Every sensor found.
    pub sensors: Vec<TemperatureReading>,
}

/// One fan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fan {
    /// Label (from `fanN_label`, or `Fan N`).
    pub label: String,
    /// hwmon chip name providing it.
    pub chip: String,
    /// Current speed, RPM.
    pub rpm: Option<u32>,
    /// Minimum RPM reported by the driver.
    pub min_rpm: Option<u32>,
    /// Maximum RPM reported by the driver.
    pub max_rpm: Option<u32>,
    /// Target RPM reported by the driver (0 usually means automatic).
    pub target_rpm: Option<u32>,
    /// Whether the kernel exposes a writable target/pwm attribute.
    /// Informational only: Legion Control does not write to it.
    pub control_interface_present: bool,
    /// Source path.
    pub source: String,
}

/// Fan status.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FanInfo {
    /// Fans.
    pub fans: Vec<Fan>,
    /// Whether Legion Control supports manual fan control on this machine.
    pub fan_control_supported: bool,
    /// Explanation.
    pub note: String,
}

/// Battery health classification (from full vs design capacity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryHealth {
    /// ≥ 80 % of design capacity.
    Good,
    /// 60–80 %.
    Fair,
    /// < 60 %.
    Poor,
    /// Unknown.
    Unknown,
}

/// Charging-type selection as exposed by the `charge_types` power_supply
/// attribute (e.g. `Fast Standard [Long_Life]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChargeTypes {
    /// Values offered by the kernel.
    pub available: Vec<String>,
    /// Active value.
    pub active: Option<String>,
}

/// One battery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Battery {
    /// power_supply name (`BAT0`).
    pub name: String,
    /// Status (`Charging`, `Discharging`, `Not charging`, `Full`).
    pub status: Option<String>,
    /// State of charge, %.
    pub capacity_percent: Option<u8>,
    /// Full / design capacity, %.
    pub health_percent: Option<f64>,
    /// Health classification.
    pub health: BatteryHealth,
    /// Charge cycles.
    pub cycle_count: Option<u32>,
    /// Instant power, W (absolute value).
    pub power_w: Option<f64>,
    /// Voltage, V.
    pub voltage_v: Option<f64>,
    /// Energy now, Wh.
    pub energy_now_wh: Option<f64>,
    /// Energy when full, Wh.
    pub energy_full_wh: Option<f64>,
    /// Design energy, Wh.
    pub energy_full_design_wh: Option<f64>,
    /// Chemistry.
    pub technology: Option<String>,
    /// Manufacturer.
    pub manufacturer: Option<String>,
    /// Model.
    pub model: Option<String>,
    /// `charge_types`, if present.
    pub charge_types: Option<ChargeTypes>,
    /// `charge_control_start_threshold`, if present.
    pub charge_start_threshold: Option<u8>,
    /// `charge_control_end_threshold`, if present.
    pub charge_end_threshold: Option<u8>,
}

/// Battery / AC status.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatteryInfo {
    /// Whether a mains adapter is online.
    pub ac_online: Option<bool>,
    /// Batteries.
    pub batteries: Vec<Battery>,
    /// ideapad-laptop `conservation_mode`, if exposed.
    pub conservation_mode: Option<bool>,
}

/// Aggregated status used by `legionctl status` and (later) `GetStatus()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// System identification.
    pub system: SystemInfo,
    /// Power profile.
    pub profile: PowerProfileState,
    /// CPU.
    pub cpu: CpuInfo,
    /// GPU.
    pub gpu: GpuInfo,
    /// Memory.
    pub memory: MemoryInfo,
    /// Temperatures.
    pub thermal: ThermalInfo,
    /// Fans.
    pub fans: FanInfo,
    /// Battery.
    pub battery: BatteryInfo,
}

/// One firmware attribute (`/sys/class/firmware-attributes/*/attributes/*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareAttribute {
    /// Attribute name.
    pub name: String,
    /// `display_name`.
    pub display_name: Option<String>,
    /// `type` (`integer`, `enumeration`, ...).
    pub kind: Option<String>,
    /// `current_value`.
    pub current_value: Option<String>,
    /// `default_value`.
    pub default_value: Option<String>,
    /// `min_value`.
    pub min_value: Option<String>,
    /// `max_value`.
    pub max_value: Option<String>,
    /// `scalar_increment`.
    pub scalar_increment: Option<String>,
    /// `possible_values` (enumerations).
    pub possible_values: Option<String>,
    /// Whether `current_value` has a write bit.
    pub writable_by_root: bool,
}

impl FirmwareAttribute {
    fn num(v: &Option<String>) -> Option<i64> {
        v.as_deref().and_then(|s| s.trim().parse().ok())
    }

    /// Current value as integer.
    pub fn current(&self) -> Option<i64> {
        Self::num(&self.current_value)
    }

    /// `(min, max, step)` when the attribute is an adjustable integer.
    ///
    /// Attributes reporting `min == max` (e.g. `0..0`) are not adjustable.
    pub fn range(&self) -> Option<(i64, i64, i64)> {
        if self.kind.as_deref() != Some("integer") || !self.writable_by_root {
            return None;
        }
        let (min, max) = (Self::num(&self.min_value)?, Self::num(&self.max_value)?);
        let step = Self::num(&self.scalar_increment)
            .filter(|s| *s > 0)
            .unwrap_or(1);
        (max > min).then_some((min, max, step))
    }
}

/// Firmware power/thermal limits (Lenovo `lenovo-wmi-other`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerLimits {
    /// Attributes found.
    pub attributes: Vec<FirmwareAttribute>,
    /// Changes only take effect in the firmware `custom` profile
    /// (`Documentation/wmi/devices/lenovo-wmi-gamezone.rst`).
    pub requires_custom_profile: bool,
    /// Whether the `custom` profile is currently active.
    pub custom_profile_active: bool,
}

/// One point of a fan curve: at `temp_c` and above, run at `rpm`
/// (`0` = firmware automatic control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurvePoint {
    /// Temperature threshold, °C.
    pub temp_c: u8,
    /// Target RPM (`0` = automatic).
    pub rpm: u32,
}

/// Step fan curve driven by the hottest of CPU/GPU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanCurve {
    /// Points in ascending temperature order.
    pub points: Vec<CurvePoint>,
    /// Degrees the temperature must drop below a step before stepping down.
    #[serde(default = "default_hysteresis")]
    pub hysteresis_c: u8,
}

fn default_hysteresis() -> u8 {
    3
}

/// Fan control mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum FanMode {
    /// Firmware automatic control (target 0).
    #[default]
    Auto,
    /// Fixed target RPM on all fans.
    Fixed {
        /// Target RPM.
        rpm: u32,
    },
    /// Software curve evaluated by the daemon every second.
    Curve(FanCurve),
}

/// State of the daemon itself (returned by `GetDaemonInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// Daemon version.
    pub version: String,
    /// Active fan mode.
    pub fan_mode: FanMode,
    /// Manual fan control enabled in `/etc/legion-control/daemon.json`.
    pub fan_control_enabled: bool,
    /// Power-limit writes enabled in `/etc/legion-control/daemon.json`.
    pub power_limits_enabled: bool,
    /// Whether polkit authorization is enforced.
    pub polkit: bool,
    /// Last safety event, if any (e.g. fan control reverted to auto).
    pub last_safety_event: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_json_shape() {
        let m: Metric<f64> = Metric::Ok(12.5);
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"status":"ok","value":12.5}"#
        );
        let m: Metric<f64> = Metric::PermissionRequired;
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"status":"permission_required"}"#
        );
        let back: Metric<f64> =
            serde_json::from_str(r#"{"status":"unavailable","value":"x"}"#).unwrap();
        assert_eq!(back, Metric::Unavailable("x".into()));
    }

    #[test]
    fn metric_helpers() {
        let m: Metric<u32> = Some(3).into();
        assert_eq!(m.value(), Some(&3));
        assert_eq!(m.map(|v| v * 2), Metric::Ok(6));
        let n: Metric<u32> = None.into();
        assert!(!n.is_ok());
    }

    #[test]
    fn firmware_attribute_range() {
        let mut a = FirmwareAttribute {
            name: "ppt_pl1_spl".into(),
            display_name: None,
            kind: Some("integer".into()),
            current_value: Some("105".into()),
            default_value: None,
            min_value: Some("50".into()),
            max_value: Some("105".into()),
            scalar_increment: Some("1".into()),
            possible_values: None,
            writable_by_root: true,
        };
        assert_eq!(a.range(), Some((50, 105, 1)));
        assert_eq!(a.current(), Some(105));
        a.max_value = Some("0".into());
        a.min_value = Some("0".into());
        assert_eq!(a.range(), None);
    }

    #[test]
    fn fan_mode_json() {
        let m = FanMode::Curve(FanCurve {
            points: vec![CurvePoint {
                temp_c: 60,
                rpm: 2400,
            }],
            hysteresis_c: 3,
        });
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"mode\":\"curve\""));
        assert_eq!(serde_json::from_str::<FanMode>(&s).unwrap(), m);
        let f: FanMode = serde_json::from_str(r#"{"mode":"fixed","rpm":3000}"#).unwrap();
        assert_eq!(f, FanMode::Fixed { rpm: 3000 });
        assert_eq!(
            serde_json::from_str::<FanMode>(r#"{"mode":"auto"}"#).unwrap(),
            FanMode::Auto
        );
    }

    #[test]
    fn product_display() {
        let s = SystemInfo {
            product_name: Some("83F0".into()),
            product_version: Some("Legion 5 15IAX10".into()),
            ..Default::default()
        };
        assert_eq!(s.display_product(), "Legion 5 15IAX10 (83F0)");
        assert_eq!(SystemInfo::default().display_product(), "unknown");
    }
}
