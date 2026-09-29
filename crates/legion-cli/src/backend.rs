//! Where changes are sent.
//!
//! * **Daemon** (preferred): `legion-control-daemon` over D-Bus — works for
//!   normal users, authorised by polkit.
//! * **Direct**: this process writes sysfs itself. Used when the daemon is
//!   not running (needs root, except profile changes that
//!   power-profiles-daemon accepts), and with `--sysroot` fixtures in tests.
//!
//! Reads always happen directly: they need no privileges.

use anyhow::{Result, anyhow};

use legion_core::model::{DaemonInfo, FanMode, PowerLimits};
use legion_core::{HardwareController, HwError, PowerProfile, SysRoot};
use legion_dbus::{Bus, Client};
use legion_hardware::{ControllerOptions, LinuxController, custom};
use legion_profiles::CustomProfile;

use crate::cli::Cli;

/// Selected backend.
pub enum Backend {
    /// Through the daemon.
    Daemon(Box<Client>),
    /// Direct sysfs access.
    Direct(LinuxController),
}

fn hint(e: HwError) -> anyhow::Error {
    match e {
        HwError::PermissionDenied(p) => anyhow!(
            "Administrator permission required ({}). Start the daemon with \
             `sudo systemctl enable --now legion-control.service`, or run with sudo.",
            p.display()
        ),
        other => anyhow!(other),
    }
}

impl Backend {
    /// Picks the backend for this invocation.
    pub fn select(cli: &Cli) -> Self {
        if let Some(dir) = &cli.sysroot {
            return Backend::Direct(LinuxController::new(ControllerOptions::fixture(
                SysRoot::new(dir),
            )));
        }
        let bus = if cli.session {
            Bus::Session
        } else {
            Bus::System
        };
        match Client::connect(bus) {
            Ok(c) if c.is_running() => Backend::Daemon(Box::new(c)),
            _ => Backend::Direct(LinuxController::host()),
        }
    }

    /// Human description.
    pub fn describe(&self) -> &'static str {
        match self {
            Backend::Daemon(_) => "legion-control-daemon",
            Backend::Direct(_) => "direct",
        }
    }

    fn run(
        &self,
        daemon: impl FnOnce(&Client) -> legion_core::Result<()>,
        direct: impl FnOnce(&LinuxController) -> legion_core::Result<()>,
    ) -> Result<()> {
        match self {
            Backend::Daemon(c) => daemon(c).map_err(hint),
            Backend::Direct(ctrl) => direct(ctrl).map_err(hint),
        }
    }

    /// Changes the power profile.
    pub fn set_profile(&self, p: PowerProfile) -> Result<()> {
        self.run(
            |c| c.call(|x| x.set_profile(p.as_str())),
            |d| d.set_power_profile(p),
        )
    }

    /// Conservation mode.
    pub fn set_conservation(&self, on: bool) -> Result<()> {
        self.run(
            |c| c.call(|x| x.set_battery_conservation(on)),
            |d| d.set_battery_conservation(on),
        )
    }

    /// Charge type.
    pub fn set_charge_type(&self, t: &str) -> Result<()> {
        self.run(
            |c| c.call(|x| x.set_charge_type(t)),
            |d| d.set_charge_type(t),
        )
    }

    /// Charge threshold.
    pub fn set_threshold(&self, v: u8) -> Result<()> {
        self.run(
            |c| c.call(|x| x.set_charge_threshold(v)),
            |d| d.set_charge_threshold(v),
        )
    }

    /// Fan mode.
    pub fn set_fan_mode(&self, m: &FanMode) -> Result<()> {
        let json = serde_json::to_string(m)?;
        self.run(
            |c| c.call(|x| x.set_fan_mode(&json)),
            |d| {
                legion_fan::validate_mode(d.root(), m)?;
                custom::direct_fan_handler(d)(m)
            },
        )
    }

    /// Firmware attribute.
    pub fn set_firmware_attribute(&self, name: &str, value: i64) -> Result<()> {
        self.run(
            |c| c.call(|x| x.set_firmware_attribute(name, value)),
            |d| d.set_firmware_attribute(name, value),
        )
    }

    /// EPP.
    pub fn set_epp(&self, v: &str) -> Result<()> {
        self.run(|c| c.call(|x| x.set_cpu_epp(v)), |d| d.set_cpu_epp(v))
    }

    /// Governor.
    pub fn set_governor(&self, v: &str) -> Result<()> {
        self.run(
            |c| c.call(|x| x.set_cpu_governor(v)),
            |d| d.set_cpu_governor(v),
        )
    }

    /// Turbo.
    pub fn set_turbo(&self, on: bool) -> Result<()> {
        self.run(|c| c.call(|x| x.set_turbo(on)), |d| d.set_turbo(on))
    }

    /// Applies a custom profile; returns the changes made.
    pub fn apply_profile(&self, p: &CustomProfile) -> Result<Vec<String>> {
        match self {
            Backend::Daemon(c) => {
                let json = serde_json::to_string(p)?;
                c.get(|x| x.apply_profile(&json)).map_err(hint)
            }
            Backend::Direct(d) => custom::apply(d, p, custom::direct_fan_handler(d)).map_err(hint),
        }
    }

    /// Current settings as a profile.
    pub fn capture(&self) -> Result<CustomProfile> {
        match self {
            Backend::Daemon(c) => c.get(|x| x.capture_profile()).map_err(hint),
            Backend::Direct(d) => Ok(custom::capture(d, None)),
        }
    }

    /// Firmware limits.
    pub fn power_limits(&self) -> Result<PowerLimits> {
        match self {
            Backend::Daemon(c) => c.get(|x| x.get_power_limits()).map_err(hint),
            Backend::Direct(d) => d.get_power_limits().map_err(hint),
        }
    }

    /// Toggles an experimental feature.
    pub fn set_experimental(&self, name: &str, on: bool) -> Result<()> {
        match self {
            Backend::Daemon(c) => c.call(|x| x.set_experimental(name, on)).map_err(hint),
            Backend::Direct(d) => {
                let mut cfg = legion_hardware::Config::load(d.root());
                cfg.set_flag(name, on).map_err(hint)?;
                cfg.save(d.root()).map_err(hint)
            }
        }
    }

    /// Daemon state, if a daemon is used.
    pub fn daemon_info(&self) -> Option<DaemonInfo> {
        match self {
            Backend::Daemon(c) => c.get(|x| x.get_daemon_info()).ok(),
            Backend::Direct(_) => None,
        }
    }
}
