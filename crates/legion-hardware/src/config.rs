//! System configuration: `/etc/legion-control/daemon.json`.
//!
//! Owned by root and read by the daemon (and by `legionctl` when it runs as
//! root without a daemon). Experimental write features are **disabled by
//! default** and must be enabled explicitly by the administrator.

use serde::{Deserialize, Serialize};

use legion_core::SysRoot;

/// Path of the configuration file.
pub const CONFIG_PATH: &str = "/etc/legion-control/daemon.json";

/// Daemon / system configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Allow manual fan control (`fanN_target`) and fan curves.
    pub enable_fan_control: bool,
    /// Allow writing firmware power/thermal limits (`lenovo-wmi-other`).
    pub enable_power_limits: bool,
    /// CPU temperature (°C) at which manual fan control is abandoned and the
    /// firmware takes over again.
    pub fan_safety_cpu_c: f64,
    /// GPU temperature (°C) at which manual fan control is abandoned.
    pub fan_safety_gpu_c: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enable_fan_control: false,
            enable_power_limits: false,
            fan_safety_cpu_c: 95.0,
            fan_safety_gpu_c: 85.0,
        }
    }
}

impl Config {
    /// Loads the configuration; a missing file yields the defaults. An
    /// invalid file also yields the (safe) defaults and a warning.
    pub fn load(root: &SysRoot) -> Self {
        let Some(text) = root.read_opt(CONFIG_PATH) else {
            return Self::default();
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(c) => c.sanitised(),
            Err(e) => {
                tracing::warn!(error = %e, "invalid {CONFIG_PATH}; using safe defaults");
                Self::default()
            }
        }
    }

    /// Experimental feature flags that can be toggled at runtime.
    pub const FLAGS: [&'static str; 2] = ["enable_fan_control", "enable_power_limits"];

    /// Sets one of [`Config::FLAGS`].
    pub fn set_flag(&mut self, name: &str, value: bool) -> legion_core::Result<()> {
        match name {
            "enable_fan_control" => self.enable_fan_control = value,
            "enable_power_limits" => self.enable_power_limits = value,
            other => {
                return Err(legion_core::HwError::InvalidValue(format!(
                    "unknown setting {other:?} (allowed: {})",
                    Self::FLAGS.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// Writes the configuration atomically (root only; used by the daemon).
    pub fn save(&self, root: &SysRoot) -> legion_core::Result<()> {
        use std::io::Write;
        let path = root.path(CONFIG_PATH);
        let io = |e: std::io::Error| legion_core::HwError::from_io(&path, e);
        let dir = path.parent().expect("config path has a parent");
        std::fs::create_dir_all(dir).map_err(io)?;
        let tmp = dir.join(".daemon.json.tmp");
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| legion_core::HwError::InvalidValue(e.to_string()))?;
        let mut f = std::fs::File::create(&tmp).map_err(io)?;
        f.write_all(text.as_bytes())
            .and_then(|_| f.write_all(b"\n"))
            .and_then(|_| f.sync_all())
            .map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)?;
        tracing::info!(target: "legion_audit", path = CONFIG_PATH, config = %text.replace('\n', " "), "configuration saved");
        Ok(())
    }

    /// Clamps safety thresholds to sane values.
    pub fn sanitised(mut self) -> Self {
        self.fan_safety_cpu_c = self.fan_safety_cpu_c.clamp(60.0, 100.0);
        self.fan_safety_gpu_c = self.fan_safety_gpu_c.clamp(60.0, 92.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    #[test]
    fn defaults_are_safe() {
        let fs = FakeSysfs::new();
        let c = Config::load(&fs.root());
        assert!(!c.enable_fan_control && !c.enable_power_limits);
        fs.file(CONFIG_PATH, "{ not json");
        assert_eq!(Config::load(&fs.root()), Config::default());
        fs.file(
            CONFIG_PATH,
            r#"{"enable_fan_control": true, "fan_safety_cpu_c": 150}"#,
        );
        let c = Config::load(&fs.root());
        assert!(c.enable_fan_control);
        assert_eq!(c.fan_safety_cpu_c, 100.0);
        fs.file(CONFIG_PATH, r#"{"unknown": 1}"#);
        assert_eq!(Config::load(&fs.root()), Config::default());
    }

    #[test]
    fn set_flag_and_save() {
        let fs = FakeSysfs::new();
        let mut c = Config::default();
        c.set_flag("enable_power_limits", true).unwrap();
        assert!(c.set_flag("enable_ec_writes", true).is_err());
        c.save(&fs.root()).unwrap();
        let back = Config::load(&fs.root());
        assert!(back.enable_power_limits && !back.enable_fan_control);
    }
}
