//! D-Bus contract of Legion Control.
//!
//! * Service: `com.legion.Control` (system bus)
//! * Object:  `/com/legion/Control`
//! * Interface: `com.legion.Control.Manager`
//!
//! Getter methods return JSON documents whose schema is the serde
//! representation of the types in `legion_core::model` (this keeps the
//! D-Bus signature stable while the model evolves). Setters take plain
//! D-Bus types; complex arguments (fan modes, custom profiles) are JSON.
//!
//! Every setter is authorised with polkit by the daemon (see
//! [`actions`]) and validated again against the hardware.

use std::time::Duration;

use serde::de::DeserializeOwned;

use legion_core::HwError;

/// Re-export for clients (proxy result types).
pub use zbus;

/// Well-known bus name.
pub const BUS_NAME: &str = "com.legion.Control";
/// Object path.
pub const OBJECT_PATH: &str = "/com/legion/Control";
/// Interface name.
pub const INTERFACE: &str = "com.legion.Control.Manager";

/// polkit action ids (see `polkit/com.legion.Control.policy`).
pub mod actions {
    /// Change the power profile.
    pub const PROFILE: &str = "com.legion.Control.set-profile";
    /// Battery conservation / charge type / threshold.
    pub const BATTERY: &str = "com.legion.Control.battery";
    /// CPU governor / EPP / turbo.
    pub const CPU: &str = "com.legion.Control.cpu";
    /// Manual fan control.
    pub const FAN: &str = "com.legion.Control.fan";
    /// Firmware power / thermal limits.
    pub const POWER_LIMITS: &str = "com.legion.Control.power-limits";
    /// Keyboard backlight colour / brightness (experimental).
    pub const KEYBOARD: &str = "com.legion.Control.keyboard";
    /// Change the daemon configuration (experimental feature gates).
    pub const CONFIGURE: &str = "com.legion.Control.configure";
}

/// Generated proxy (`ManagerProxy`, `ManagerProxyBlocking`, signal streams).
#[allow(missing_docs)]
mod proxy {
    /// Client-side proxy of `com.legion.Control.Manager`.
    #[zbus::proxy(
        interface = "com.legion.Control.Manager",
        default_service = "com.legion.Control",
        default_path = "/com/legion/Control"
    )]
    pub trait Manager {
        /// Daemon version and state (`DaemonInfo`).
        fn get_daemon_info(&self) -> zbus::Result<String>;
        /// `SystemInfo`.
        fn get_hardware_info(&self) -> zbus::Result<String>;
        /// `Vec<Capability>`.
        fn get_capabilities(&self) -> zbus::Result<String>;
        /// `PowerProfileState`.
        fn get_profile(&self) -> zbus::Result<String>;
        /// Switches the power profile (`quiet`, `balanced`, `performance`,
        /// `max-power`, `custom`).
        fn set_profile(&self, profile: &str) -> zbus::Result<()>;
        /// `CpuInfo`.
        fn get_cpu_info(&self) -> zbus::Result<String>;
        /// `GpuInfo`.
        fn get_gpu_info(&self) -> zbus::Result<String>;
        /// `BatteryInfo`.
        fn get_battery_info(&self) -> zbus::Result<String>;
        /// `ThermalInfo`.
        fn get_thermal_info(&self) -> zbus::Result<String>;
        /// `FanInfo`.
        fn get_fan_info(&self) -> zbus::Result<String>;
        /// `Status`.
        fn get_status(&self) -> zbus::Result<String>;
        /// `PowerLimits`.
        fn get_power_limits(&self) -> zbus::Result<String>;
        /// Battery conservation mode.
        fn set_battery_conservation(&self, enabled: bool) -> zbus::Result<()>;
        /// `charge_types` value.
        fn set_charge_type(&self, charge_type: &str) -> zbus::Result<()>;
        /// Numeric charge threshold.
        fn set_charge_threshold(&self, end: u8) -> zbus::Result<()>;
        /// Fan mode as JSON (`FanMode`).
        fn set_fan_mode(&self, mode_json: &str) -> zbus::Result<()>;
        /// Firmware attribute (power/thermal limit).
        fn set_firmware_attribute(&self, name: &str, value: i64) -> zbus::Result<()>;
        /// cpufreq EPP.
        fn set_cpu_epp(&self, epp: &str) -> zbus::Result<()>;
        /// cpufreq governor.
        fn set_cpu_governor(&self, governor: &str) -> zbus::Result<()>;
        /// Turbo boost.
        fn set_turbo(&self, enabled: bool) -> zbus::Result<()>;
        /// `KeyboardInfo` (the vendor interface is only queried when
        /// `enable_keyboard_rgb` is on).
        fn get_keyboard_info(&self) -> zbus::Result<String>;
        /// One steady colour on the whole keyboard (`#rrggbb` or a name).
        fn set_keyboard_color(&self, color: &str) -> zbus::Result<()>;
        /// Keyboard backlight brightness level.
        fn set_keyboard_brightness(&self, level: u8) -> zbus::Result<()>;
        /// Puts back the keyboard effect saved before the first change.
        fn restore_keyboard(&self) -> zbus::Result<()>;
        /// Applies a `CustomProfile` (JSON); returns the list of changes (JSON array).
        fn apply_profile(&self, profile_json: &str) -> zbus::Result<String>;
        /// Current settings as a `CustomProfile` (JSON).
        fn capture_profile(&self) -> zbus::Result<String>;
        /// Enables/disables an experimental feature (`enable_fan_control`,
        /// `enable_power_limits`, `enable_keyboard_rgb`) and saves `/etc/legion-control/daemon.json`.
        fn set_experimental(&self, feature: &str, enabled: bool) -> zbus::Result<()>;

        /// Logical profile changed.
        #[zbus(signal)]
        fn profile_changed(&self, profile: &str) -> zbus::Result<()>;
        /// CPU / GPU temperature changed by at least 1 °C (`{"cpu_c":..,"gpu_c":..}`).
        #[zbus(signal)]
        fn temperature_changed(&self, json: &str) -> zbus::Result<()>;
        /// Battery state changed (`BatteryInfo`).
        #[zbus(signal)]
        fn battery_changed(&self, json: &str) -> zbus::Result<()>;
        /// Something else changed (fan mode, settings, safety event).
        #[zbus(signal)]
        fn hardware_changed(&self) -> zbus::Result<()>;
    }
}

pub use proxy::*;

/// Converts a hardware error into a D-Bus error (daemon side).
pub fn to_fdo(e: HwError) -> zbus::fdo::Error {
    use zbus::fdo::Error as F;
    match e {
        HwError::NotSupported(m) => F::NotSupported(m),
        HwError::PermissionDenied(p) => F::AccessDenied(format!(
            "Administrator permission required: {}",
            p.display()
        )),
        HwError::InvalidValue(m) => F::InvalidArgs(m),
        other => F::Failed(other.to_string()),
    }
}

/// Converts a D-Bus error into a hardware error (client side), keeping the
/// daemon's message.
pub fn from_zbus(e: zbus::Error) -> HwError {
    if let zbus::Error::MethodError(name, msg, _) = &e {
        let msg = msg.clone().unwrap_or_default();
        return match name.as_str() {
            "org.freedesktop.DBus.Error.NotSupported" => HwError::NotSupported(msg),
            "org.freedesktop.DBus.Error.InvalidArgs" => HwError::InvalidValue(msg),
            "org.freedesktop.DBus.Error.AccessDenied" => HwError::Backend {
                backend: "legion-control-daemon",
                message: if msg.is_empty() {
                    "Administrator permission required.".into()
                } else {
                    msg
                },
            },
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => HwError::Backend {
                backend: "legion-control-daemon",
                message: "not running (sudo systemctl enable --now legion-control.service)".into(),
            },
            _ => HwError::Backend {
                backend: "legion-control-daemon",
                message: msg,
            },
        };
    }
    HwError::Backend {
        backend: "D-Bus",
        message: e.to_string(),
    }
}

/// Which bus to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    /// System bus (production).
    System,
    /// Session bus (development / tests).
    Session,
}

/// Blocking client with typed helpers.
pub struct Client {
    proxy: ManagerProxyBlocking<'static>,
}

impl Client {
    /// Connects. Method calls may wait for an interactive polkit
    /// authentication, hence the long timeout.
    pub fn connect(bus: Bus) -> legion_core::Result<Self> {
        let builder = match bus {
            Bus::System => zbus::blocking::connection::Builder::system(),
            Bus::Session => zbus::blocking::connection::Builder::session(),
        }
        .map_err(from_zbus)?;
        let conn = builder
            .method_timeout(Duration::from_secs(120))
            .build()
            .map_err(from_zbus)?;
        let proxy = ManagerProxyBlocking::new(&conn).map_err(from_zbus)?;
        Ok(Self { proxy })
    }

    /// Connects to an explicit bus address (tests).
    pub fn connect_address(address: &str) -> legion_core::Result<Self> {
        let conn = zbus::blocking::connection::Builder::address(address)
            .map_err(from_zbus)?
            .method_timeout(Duration::from_secs(120))
            .build()
            .map_err(from_zbus)?;
        let proxy = ManagerProxyBlocking::new(&conn).map_err(from_zbus)?;
        Ok(Self { proxy })
    }

    /// Returns `true` if the daemon currently owns its bus name.
    pub fn is_running(&self) -> bool {
        let conn = self.proxy.inner().connection();
        let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
            return false;
        };
        dbus.name_has_owner(BUS_NAME.try_into().expect("valid bus name"))
            .unwrap_or(false)
    }

    /// Raw proxy.
    pub fn proxy(&self) -> &ManagerProxyBlocking<'static> {
        &self.proxy
    }

    /// Calls a JSON getter and decodes it.
    pub fn get<T: DeserializeOwned>(
        &self,
        f: impl FnOnce(&ManagerProxyBlocking<'static>) -> zbus::Result<String>,
    ) -> legion_core::Result<T> {
        let json = f(&self.proxy).map_err(from_zbus)?;
        serde_json::from_str(&json).map_err(|e| HwError::Backend {
            backend: "legion-control-daemon",
            message: format!("unexpected reply: {e}"),
        })
    }

    /// Calls a setter.
    pub fn call(
        &self,
        f: impl FnOnce(&ManagerProxyBlocking<'static>) -> zbus::Result<()>,
    ) -> legion_core::Result<()> {
        f(&self.proxy).map_err(from_zbus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping_roundtrip() {
        let fdo = to_fdo(HwError::InvalidValue("bad".into()));
        assert!(matches!(fdo, zbus::fdo::Error::InvalidArgs(ref m) if m == "bad"));
        let fdo = to_fdo(HwError::NotSupported("x".into()));
        assert!(matches!(fdo, zbus::fdo::Error::NotSupported(_)));
    }
}
