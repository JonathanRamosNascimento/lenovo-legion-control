//! Generic, read-only hwmon enumeration (`/sys/class/hwmon`).
//!
//! See `Documentation/hwmon/sysfs-interface.rst` in the kernel tree for the
//! attribute naming rules used here.

use std::path::PathBuf;

use crate::sysfs::{self, SysRoot};

/// Kernel path of the hwmon class.
pub const HWMON_CLASS: &str = "/sys/class/hwmon";

/// One hwmon chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HwmonChip {
    /// Class device id (`hwmon3`).
    pub id: String,
    /// Chip name from the `name` attribute (`coretemp`, `nvme`, ...).
    pub name: String,
    /// Resolved directory of the chip.
    pub path: PathBuf,
    /// Kernel driver bound to the parent device, if any.
    pub driver: Option<String>,
}

impl HwmonChip {
    /// Path of an attribute of this chip.
    pub fn attr_path(&self, attr: &str) -> PathBuf {
        self.path.join(attr)
    }

    /// Reads an attribute (trimmed); `None` if absent/unreadable.
    pub fn read(&self, attr: &str) -> Option<String> {
        sysfs::read_opt(&self.attr_path(attr))
    }

    /// Reads and parses an attribute.
    pub fn read_parse<T: std::str::FromStr>(&self, attr: &str) -> Option<T> {
        sysfs::read_parse_opt(&self.attr_path(attr))
    }

    /// Indices `N` for which `{kind}N_input` exists (e.g. `kind = "temp"`).
    pub fn channels(&self, kind: &str) -> Vec<u32> {
        let mut out: Vec<u32> = sysfs::list_dir(&self.path)
            .into_iter()
            .filter_map(|f| {
                let rest = f.strip_prefix(kind)?;
                let idx = rest.strip_suffix("_input")?;
                idx.parse().ok()
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// Reads a temperature channel in °C (`tempN_input` is in millidegrees).
    pub fn temp_c(&self, n: u32) -> Option<f64> {
        self.read_parse::<i64>(&format!("temp{n}_input"))
            .map(|m| m as f64 / 1000.0)
    }

    /// Reads a temperature threshold (`crit`, `max`...) in °C.
    pub fn temp_limit_c(&self, n: u32, limit: &str) -> Option<f64> {
        self.read_parse::<i64>(&format!("temp{n}_{limit}"))
            .map(|m| m as f64 / 1000.0)
    }

    /// Label of a channel (`tempN_label`, `fanN_label`).
    pub fn label(&self, kind: &str, n: u32) -> Option<String> {
        self.read(&format!("{kind}{n}_label"))
            .filter(|s| !s.is_empty())
    }

    /// Report-friendly source name (`hwmon:coretemp/temp1`).
    pub fn source(&self, attr_prefix: &str) -> String {
        format!("hwmon:{}/{}", self.name, attr_prefix)
    }
}

/// Enumerates all hwmon chips, sorted by id.
pub fn chips(root: &SysRoot) -> Vec<HwmonChip> {
    root.list(HWMON_CLASS)
        .into_iter()
        .filter_map(|id| {
            let path = root.path(HWMON_CLASS).join(&id);
            let name = sysfs::read_opt(&path.join("name"))?;
            let driver = sysfs::link_name(&path.join("device/driver"));
            Some(HwmonChip {
                id,
                name,
                path,
                driver,
            })
        })
        .collect()
}

/// Finds the first chip with the given name.
pub fn find(root: &SysRoot, name: &str) -> Option<HwmonChip> {
    chips(root).into_iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeSysfs;

    #[test]
    fn enumerate_and_read() {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/hwmon/hwmon4/name", "coretemp\n")
            .file("/sys/class/hwmon/hwmon4/temp1_input", "65000")
            .file("/sys/class/hwmon/hwmon4/temp1_label", "Package id 0")
            .file("/sys/class/hwmon/hwmon4/temp1_crit", "105000")
            .file("/sys/class/hwmon/hwmon4/temp10_input", "60000")
            .file("/sys/class/hwmon/hwmon10/name", "nvme")
            .file("/sys/class/hwmon/hwmon10/temp1_input", "42850")
            .file("/sys/class/hwmon/hwmon2/nameless", "x");
        let root = fs.root();
        let all = chips(&root);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "hwmon4");
        assert_eq!(all[1].id, "hwmon10");
        let c = find(&root, "coretemp").unwrap();
        assert_eq!(c.channels("temp"), [1, 10]);
        assert_eq!(c.temp_c(1), Some(65.0));
        assert_eq!(c.temp_limit_c(1, "crit"), Some(105.0));
        assert_eq!(c.label("temp", 1).as_deref(), Some("Package id 0"));
        assert_eq!(c.label("temp", 10), None);
        assert_eq!(find(&root, "nvme").unwrap().temp_c(1), Some(42.85));
    }
}
