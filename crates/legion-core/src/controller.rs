//! The hardware abstraction trait.

use crate::capabilities::{Capability, HardwareCapabilities};
use crate::error::{HwError, Result};
use crate::model::{
    BatteryInfo, CpuInfo, FanInfo, GpuInfo, MemoryInfo, PowerLimits, Status, SystemInfo,
    ThermalInfo,
};
use crate::profile::{PowerProfile, PowerProfileState};

/// Abstraction over a machine's hardware interfaces.
///
/// Implementations must never assume a feature exists: every getter probes
/// the real interface and reports absence explicitly.
pub trait HardwareController {
    /// Summary capability flags.
    fn get_capabilities(&self) -> Result<HardwareCapabilities> {
        Ok(HardwareCapabilities::from_list(
            &self.get_supported_features()?,
        ))
    }

    /// Detailed feature list with evidence.
    fn get_supported_features(&self) -> Result<Vec<Capability>>;

    /// System identification.
    fn get_system_info(&self) -> Result<SystemInfo>;

    /// Current power profile.
    fn get_power_profile(&self) -> Result<PowerProfileState>;

    /// Changes the power profile. Implementations validate the profile
    /// against the choices offered by the hardware.
    fn set_power_profile(&self, profile: PowerProfile) -> Result<()>;

    /// CPU status.
    fn get_cpu_info(&self) -> Result<CpuInfo>;

    /// GPU status.
    fn get_gpu_info(&self) -> Result<GpuInfo>;

    /// Memory status.
    fn get_memory_info(&self) -> Result<MemoryInfo>;

    /// Temperatures.
    fn get_temperatures(&self) -> Result<ThermalInfo>;

    /// Fans.
    fn get_fan_info(&self) -> Result<FanInfo>;

    /// Battery.
    fn get_battery_info(&self) -> Result<BatteryInfo>;

    /// Firmware power / thermal limits.
    fn get_power_limits(&self) -> Result<PowerLimits> {
        Ok(PowerLimits::default())
    }

    /// Enables/disables the firmware battery conservation mode.
    fn set_battery_conservation(&self, _enabled: bool) -> Result<()> {
        Err(HwError::NotSupported("battery conservation mode".into()))
    }

    /// Selects a `charge_types` value (e.g. `Fast`, `Standard`, `Long_Life`).
    fn set_charge_type(&self, _charge_type: &str) -> Result<()> {
        Err(HwError::NotSupported("battery charge type".into()))
    }

    /// Sets the numeric charge end threshold (%).
    fn set_charge_threshold(&self, _end: u8) -> Result<()> {
        Err(HwError::NotSupported("battery charge threshold".into()))
    }

    /// Sets the target RPM of one fan (`Some(index)`, 1-based) or all fans
    /// (`None`). `0` returns control to the firmware.
    fn set_fan_target(&self, _fan: Option<usize>, _rpm: u32) -> Result<()> {
        Err(HwError::NotSupported(
            "Manual fan control is not supported on this hardware.".into(),
        ))
    }

    /// Writes a firmware attribute (power / thermal limit).
    fn set_firmware_attribute(&self, _name: &str, _value: i64) -> Result<()> {
        Err(HwError::NotSupported("firmware attributes".into()))
    }

    /// Sets the cpufreq energy performance preference on all policies.
    fn set_cpu_epp(&self, _epp: &str) -> Result<()> {
        Err(HwError::NotSupported("CPU EPP".into()))
    }

    /// Sets the cpufreq governor on all policies.
    fn set_cpu_governor(&self, _governor: &str) -> Result<()> {
        Err(HwError::NotSupported("CPU governor".into()))
    }

    /// Enables/disables turbo boost.
    fn set_turbo(&self, _enabled: bool) -> Result<()> {
        Err(HwError::NotSupported("turbo control".into()))
    }

    /// Everything at once.
    fn get_status(&self) -> Result<Status> {
        Ok(Status {
            system: self.get_system_info()?,
            profile: self.get_power_profile()?,
            cpu: self.get_cpu_info()?,
            gpu: self.get_gpu_info()?,
            memory: self.get_memory_info()?,
            thermal: self.get_temperatures()?,
            fans: self.get_fan_info()?,
            battery: self.get_battery_info()?,
        })
    }
}
