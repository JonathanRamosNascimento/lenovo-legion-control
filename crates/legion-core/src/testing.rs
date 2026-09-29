//! Test helpers: a fake sysfs/procfs tree and a mock controller.
//!
//! Only compiled for tests or with the `testing` feature. The fake tree lives
//! in a temporary directory that is removed when dropped.

use std::path::Path;

use crate::capabilities::Capability;
use crate::controller::HardwareController;
use crate::error::{HwError, Result};
use crate::model::*;
use crate::profile::{PowerProfile, PowerProfileState, ProfileBackend};
use crate::sysfs::SysRoot;

/// Builder for a fake filesystem tree.
pub struct FakeSysfs {
    dir: tempfile::TempDir,
}

impl Default for FakeSysfs {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSysfs {
    /// Creates an empty tree.
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("create tempdir"),
        }
    }

    /// A [`SysRoot`] pointing at this tree.
    pub fn root(&self) -> SysRoot {
        SysRoot::new(self.dir.path())
    }

    /// Keeps the tree on disk (not deleted on drop) and returns its path.
    pub fn keep(self) -> std::path::PathBuf {
        self.dir.keep()
    }

    /// Filesystem path of the tree.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Writes a file (creating parents) at a kernel path.
    pub fn file(&self, abs: &str, content: &str) -> &Self {
        let p = self.root().path(abs);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        self
    }

    /// Creates a directory.
    pub fn dir(&self, abs: &str) -> &Self {
        std::fs::create_dir_all(self.root().path(abs)).unwrap();
        self
    }

    /// Creates a symlink at `abs` pointing to `target` (a kernel path inside
    /// the tree; the target directory is created if missing).
    pub fn symlink(&self, abs: &str, target: &str) -> &Self {
        let p = self.root().path(abs);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let t = self.root().path(target);
        std::fs::create_dir_all(&t).unwrap();
        std::os::unix::fs::symlink(t, p).unwrap();
        self
    }

    /// Sets permission bits of a file.
    pub fn mode(&self, abs: &str, mode: u32) -> &Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(self.root().path(abs), std::fs::Permissions::from_mode(mode))
            .unwrap();
        self
    }
}

/// A controller returning canned data; used to test consumers of the trait.
#[derive(Debug, Clone, Default)]
pub struct MockController {
    /// Returned by `get_system_info`.
    pub system: SystemInfo,
    /// Returned by `get_battery_info`.
    pub battery: BatteryInfo,
    /// Returned by `get_temperatures`.
    pub thermal: ThermalInfo,
    /// Returned by `get_fan_info`.
    pub fans: FanInfo,
    /// Returned by `get_supported_features`.
    pub features: Vec<Capability>,
    /// Current profile.
    pub profile: Option<PowerProfile>,
}

impl HardwareController for MockController {
    fn get_supported_features(&self) -> Result<Vec<Capability>> {
        Ok(self.features.clone())
    }

    fn get_system_info(&self) -> Result<SystemInfo> {
        Ok(self.system.clone())
    }

    fn get_power_profile(&self) -> Result<PowerProfileState> {
        Ok(PowerProfileState {
            current: self.profile.unwrap_or(PowerProfile::Unknown),
            backend: if self.profile.is_some() {
                ProfileBackend::PlatformProfile
            } else {
                ProfileBackend::None
            },
            platform_profile: None,
            ppd: None,
            notes: vec![],
        })
    }

    fn set_power_profile(&self, _profile: PowerProfile) -> Result<()> {
        Err(HwError::ReadOnly("changing the power profile"))
    }

    fn get_cpu_info(&self) -> Result<CpuInfo> {
        Ok(CpuInfo {
            model: Some("Mock CPU".into()),
            vendor: None,
            logical_cpus: 4,
            topology: None,
            usage_percent: Metric::Ok(10.0),
            frequency_mhz: Some(3000),
            max_frequency_mhz: None,
            min_frequency_mhz: None,
            temperature_c: Some(50.0),
            scaling_driver: None,
            pstate_status: None,
            governors: vec!["powersave".into()],
            available_governors: vec![],
            epp: vec![],
            available_epp: vec![],
            turbo: TurboState::Unknown,
            package_power_w: Metric::NotSupported,
            rapl_constraints: vec![],
        })
    }

    fn get_gpu_info(&self) -> Result<GpuInfo> {
        Ok(GpuInfo {
            devices: vec![],
            mode: GpuMode::Unknown,
        })
    }

    fn get_memory_info(&self) -> Result<MemoryInfo> {
        Ok(MemoryInfo {
            total_mib: 16384,
            available_mib: 8192,
            used_mib: 8192,
            ..Default::default()
        })
    }

    fn get_temperatures(&self) -> Result<ThermalInfo> {
        Ok(self.thermal.clone())
    }

    fn get_fan_info(&self) -> Result<FanInfo> {
        Ok(self.fans.clone())
    }

    fn get_battery_info(&self) -> Result<BatteryInfo> {
        Ok(self.battery.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::CapabilityState;

    #[test]
    fn mock_controller_defaults() {
        let m = MockController {
            profile: Some(PowerProfile::Balanced),
            features: vec![Capability::new(
                "power_profiles",
                CapabilityState::ReadOnly,
                None,
                "",
            )],
            ..Default::default()
        };
        assert!(m.get_capabilities().unwrap().power_profiles);
        assert!(!m.get_capabilities().unwrap().fan_control);
        assert_eq!(
            m.get_power_profile().unwrap().current,
            PowerProfile::Balanced
        );
        assert!(matches!(
            m.set_power_profile(PowerProfile::Quiet),
            Err(HwError::ReadOnly(_))
        ));
        let st = m.get_status().unwrap();
        assert_eq!(st.memory.total_mib, 16384);
    }
}
