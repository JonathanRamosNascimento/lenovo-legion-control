//! [`HardwareController`] implementation for Linux.

use std::sync::Arc;
use std::time::Duration;

use legion_core::capabilities::Capability;
use legion_core::keyboard::{KeyboardInfo, Rgb};
use legion_core::model::{
    BatteryInfo, CpuInfo, FanInfo, GpuInfo, GpuVendor, MemoryInfo, PowerLimits, SystemInfo,
    ThermalInfo,
};
use legion_core::profile::{PowerProfile, PowerProfileState};
use legion_core::{HardwareController, HwError, Result, SysRoot};
use legion_gpu::GpuOptions;
use legion_keyboard::{HidrawOpener, Opener, sim::SimOpener};
use legion_profiles::ProfileAction;

use crate::config::Config;

/// Options for [`LinuxController`].
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerOptions {
    /// Filesystem root (`/` in production, a fixture directory in tests).
    pub root: SysRoot,
    /// Use host services that are not part of the filesystem tree
    /// (D-Bus, NVML). Automatically disabled for non-host roots.
    pub host_services: bool,
    /// Query NVML even if the dGPU is runtime-suspended.
    pub wake_gpu: bool,
    /// Interval used to compute CPU usage and package power.
    pub sample_interval: Duration,
    /// Desktop environment string (from `XDG_CURRENT_DESKTOP`).
    pub desktop: Option<String>,
    /// System configuration (experimental feature gates).
    pub config: Config,
}

impl Default for ControllerOptions {
    fn default() -> Self {
        Self::host()
    }
}

impl ControllerOptions {
    /// Options for the real machine.
    pub fn host() -> Self {
        Self {
            root: SysRoot::host(),
            host_services: true,
            wake_gpu: false,
            sample_interval: Duration::from_millis(250),
            desktop: std::env::var("XDG_CURRENT_DESKTOP").ok(),
            config: Config::load(&SysRoot::host()),
        }
    }

    /// Options for a fixture tree (no D-Bus, no NVML, no desktop).
    pub fn fixture(root: SysRoot) -> Self {
        Self {
            config: Config::load(&root),
            root,
            host_services: false,
            wake_gpu: false,
            sample_interval: Duration::ZERO,
            desktop: None,
        }
    }
}

/// Reads hardware state from sysfs/procfs, D-Bus and NVML.
#[derive(Debug, Clone)]
pub struct LinuxController {
    opts: ControllerOptions,
    keyboard_opener: Arc<dyn Opener>,
}

impl LinuxController {
    /// Creates a controller.
    pub fn new(mut opts: ControllerOptions) -> Self {
        if !opts.root.is_host() {
            opts.host_services = false;
        }
        // fixture roots have no /dev/hidraw*: use the simulated controller
        let keyboard_opener: Arc<dyn Opener> = if opts.root.is_host() {
            Arc::new(HidrawOpener)
        } else {
            Arc::new(SimOpener::default())
        };
        Self {
            opts,
            keyboard_opener,
        }
    }

    /// Replaces how the keyboard controller is reached (tests).
    pub fn with_keyboard_opener(mut self, opener: Arc<dyn Opener>) -> Self {
        self.keyboard_opener = opener;
        self
    }

    pub(crate) fn keyboard_opener(&self) -> Arc<dyn Opener> {
        self.keyboard_opener.clone()
    }

    /// Controller for the real machine with default options.
    pub fn host() -> Self {
        Self::new(ControllerOptions::host())
    }

    /// Replaces the configuration (experimental feature gates).
    pub fn set_config(&mut self, config: Config) {
        self.opts.config = config;
    }

    /// Options in use.
    pub fn options(&self) -> &ControllerOptions {
        &self.opts
    }

    /// Filesystem root in use.
    pub fn root(&self) -> &SysRoot {
        &self.opts.root
    }

    fn gpu_options(&self) -> GpuOptions {
        GpuOptions {
            use_nvml: self.opts.host_services,
            wake_gpu: self.opts.wake_gpu,
        }
    }

    /// Thermal info reusing already-collected GPU info (avoids a second
    /// NVML session).
    pub fn temperatures_with(&self, gpu: &GpuInfo) -> ThermalInfo {
        let nv = gpu
            .devices
            .iter()
            .filter(|d| d.vendor == GpuVendor::Nvidia)
            .find_map(|d| {
                let t = d.metrics.as_ref()?.temperature_c?;
                Some((
                    d.name.clone().unwrap_or_else(|| "NVIDIA GPU".into()),
                    f64::from(t),
                ))
            });
        legion_thermal::read_thermal(&self.opts.root, nv)
    }
}

impl HardwareController for LinuxController {
    fn get_supported_features(&self) -> Result<Vec<Capability>> {
        Ok(crate::capabilities::detect(self))
    }

    fn get_system_info(&self) -> Result<SystemInfo> {
        let gpus = legion_gpu::detect_devices(&self.opts.root)
            .into_iter()
            .map(|d| {
                d.name
                    .unwrap_or_else(|| format!("{}:{}", d.vendor_id, d.device_id))
            })
            .collect();
        Ok(crate::system::detect(
            &self.opts.root,
            self.opts.desktop.as_deref(),
            gpus,
        ))
    }

    fn get_power_profile(&self) -> Result<PowerProfileState> {
        Ok(legion_profiles::detect(
            &self.opts.root,
            self.opts.host_services,
        ))
    }

    fn set_power_profile(&self, profile: PowerProfile) -> Result<()> {
        let state = self.get_power_profile()?;
        let action = legion_profiles::plan_change(&state, profile)?;
        tracing::info!(%profile, ?action, "changing power profile");
        match action {
            ProfileAction::Ppd(name) => match legion_profiles::ppd::set_active(&name) {
                Ok(()) => Ok(()),
                Err(e) => {
                    // fall back to the kernel interface (requires root)
                    let raw = state
                        .platform_profile
                        .as_ref()
                        .and_then(|pp| pp.mapping.iter().find(|m| m.logical == profile))
                        .map(|m| m.raw.clone())
                        .ok_or_else(|| HwError::Backend {
                            backend: "power-profiles-daemon",
                            message: e.clone(),
                        })?;
                    tracing::warn!(error = %e, "power-profiles-daemon failed; writing platform_profile");
                    legion_profiles::set_platform_profile(&self.opts.root, &raw)
                }
            },
            ProfileAction::PlatformProfile(raw) => {
                legion_profiles::set_platform_profile(&self.opts.root, &raw)
            }
        }
    }

    fn get_power_limits(&self) -> Result<PowerLimits> {
        let attributes: Vec<_> = legion_acpi::read_firmware_attributes(&self.opts.root)
            .into_iter()
            .flat_map(|s| s.attributes)
            .collect();
        let custom = legion_profiles::read_platform_profile(&self.opts.root)
            .and_then(|p| p.current_raw)
            .is_some_and(|r| r == "custom");
        Ok(PowerLimits {
            requires_custom_profile: !attributes.is_empty(),
            custom_profile_active: custom,
            attributes,
        })
    }

    fn set_firmware_attribute(&self, name: &str, value: i64) -> Result<()> {
        if !self.opts.config.enable_power_limits {
            return Err(HwError::NotSupported(format!(
                "power-limit changes are disabled; set \"enable_power_limits\": true in {}",
                crate::config::CONFIG_PATH
            )));
        }
        if !self.get_power_limits()?.custom_profile_active {
            return Err(HwError::InvalidValue(
                "switch to the custom profile first (legionctl profile custom): the firmware \
                 ignores power limits in other profiles"
                    .into(),
            ));
        }
        let limits = self.get_power_limits()?;
        let current = |n: &str| {
            limits
                .attributes
                .iter()
                .find(|a| a.name == n)
                .and_then(|a| a.current())
        };
        let (pl1, pl2) = match name {
            "ppt_pl1_spl" => (Some(value), current("ppt_pl2_sppt")),
            "ppt_pl2_sppt" => (current("ppt_pl1_spl"), Some(value)),
            _ => (None, None),
        };
        if let (Some(pl1), Some(pl2)) = (pl1, pl2) {
            if pl1 > pl2 {
                return Err(HwError::InvalidValue(format!(
                    "the long-term limit PL1 ({pl1} W) cannot exceed the short-term limit PL2 ({pl2} W)"
                )));
            }
        }
        legion_acpi::set_firmware_attribute(&self.opts.root, name, value)
    }

    fn set_battery_conservation(&self, enabled: bool) -> Result<()> {
        legion_battery::set_conservation(&self.opts.root, enabled)
    }

    fn set_charge_type(&self, charge_type: &str) -> Result<()> {
        legion_battery::set_charge_type(&self.opts.root, charge_type)
    }

    fn set_charge_threshold(&self, end: u8) -> Result<()> {
        legion_battery::set_charge_threshold(&self.opts.root, end)
    }

    fn set_fan_target(&self, fan: Option<usize>, rpm: u32) -> Result<()> {
        // returning to automatic control is always allowed
        if rpm != 0 && !self.opts.config.enable_fan_control {
            return Err(HwError::NotSupported(format!(
                "manual fan control is disabled; set \"enable_fan_control\": true in {}",
                crate::config::CONFIG_PATH
            )));
        }
        legion_fan::set_target(&self.opts.root, fan, rpm)
    }

    fn get_keyboard_info(&self) -> Result<KeyboardInfo> {
        self.keyboard_info_impl()
    }

    fn set_keyboard_color(&self, color: Rgb) -> Result<()> {
        self.set_keyboard_color_impl(color)
    }

    fn set_keyboard_brightness(&self, level: u8) -> Result<()> {
        self.set_keyboard_brightness_impl(level)
    }

    fn restore_keyboard(&self) -> Result<()> {
        self.restore_keyboard_impl()
    }

    fn set_cpu_epp(&self, epp: &str) -> Result<()> {
        legion_cpu::set_epp(&self.opts.root, epp)
    }

    fn set_cpu_governor(&self, governor: &str) -> Result<()> {
        legion_cpu::set_governor(&self.opts.root, governor)
    }

    fn set_turbo(&self, enabled: bool) -> Result<()> {
        legion_cpu::set_turbo(&self.opts.root, enabled)
    }

    fn get_cpu_info(&self) -> Result<CpuInfo> {
        Ok(legion_cpu::read_cpu_info(
            &self.opts.root,
            self.opts.sample_interval,
        ))
    }

    fn get_gpu_info(&self) -> Result<GpuInfo> {
        Ok(legion_gpu::read_gpu_info(
            &self.opts.root,
            self.gpu_options(),
        ))
    }

    fn get_memory_info(&self) -> Result<MemoryInfo> {
        crate::memory::read_memory(&self.opts.root)
            .ok_or_else(|| HwError::NotSupported("/proc/meminfo".into()))
    }

    fn get_temperatures(&self) -> Result<ThermalInfo> {
        Ok(self.temperatures_with(&self.get_gpu_info()?))
    }

    fn get_fan_info(&self) -> Result<FanInfo> {
        Ok(legion_fan::read_fans(&self.opts.root))
    }

    fn get_battery_info(&self) -> Result<BatteryInfo> {
        Ok(legion_battery::read_battery_info(&self.opts.root))
    }

    fn get_status(&self) -> Result<legion_core::model::Status> {
        let gpu = self.get_gpu_info()?;
        Ok(legion_core::model::Status {
            system: self.get_system_info()?,
            profile: self.get_power_profile()?,
            cpu: self.get_cpu_info()?,
            thermal: self.temperatures_with(&gpu),
            gpu,
            memory: self.get_memory_info().unwrap_or_default(),
            fans: self.get_fan_info()?,
            battery: self.get_battery_info()?,
        })
    }
}
