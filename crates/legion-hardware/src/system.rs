//! System identification: DMI, BIOS, kernel, distribution, desktop.
//!
//! DMI is read from `/sys/class/dmi/id` (a symlink to
//! `/sys/devices/virtual/dmi/id`). Serial numbers, UUIDs and asset tags are
//! **never** read.

use legion_core::model::SystemInfo;
use legion_core::support;
use legion_core::sysfs::SysRoot;

const DMI_DIRS: [&str; 2] = ["/sys/class/dmi/id", "/sys/devices/virtual/dmi/id"];

/// DMI attributes that are read. Anything identifying the individual
/// machine or owner (serials, UUID, asset tag) is excluded on purpose.
pub const DMI_FIELDS: &[&str] = &[
    "sys_vendor",
    "product_name",
    "product_version",
    "product_family",
    "board_vendor",
    "board_name",
    "bios_vendor",
    "bios_version",
    "bios_date",
    "ec_firmware_release",
];

fn dmi(root: &SysRoot, field: &str) -> Option<String> {
    debug_assert!(DMI_FIELDS.contains(&field));
    DMI_DIRS
        .iter()
        .find_map(|d| root.read_opt(format!("{d}/{field}")))
        .filter(|v| !v.is_empty() && v != "Default string" && v != "To be filled by O.E.M.")
}

/// Parses `os-release` returning `(PRETTY_NAME or NAME, ID)`.
pub fn parse_os_release(text: &str) -> (Option<String>, Option<String>) {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_owned())
            .filter(|v| !v.is_empty())
    };
    (get("PRETTY_NAME").or_else(|| get("NAME")), get("ID"))
}

/// Normalises `XDG_CURRENT_DESKTOP` (`ubuntu:GNOME` → `GNOME`).
pub fn normalise_desktop(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split(':').filter(|p| !p.is_empty()).collect();
    parts
        .iter()
        .find(|p| {
            [
                "GNOME", "KDE", "XFCE", "Hyprland", "sway", "COSMIC", "Cinnamon", "MATE", "LXQt",
            ]
            .iter()
            .any(|k| p.eq_ignore_ascii_case(k))
        })
        .or(parts.last())
        .map(|s| (*s).to_owned())
}

/// Detects system information. `desktop` is passed in because it comes from
/// the environment, not the filesystem.
pub fn detect(root: &SysRoot, desktop: Option<&str>, gpus: Vec<String>) -> SystemInfo {
    let (os_name, os_id) = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|p| root.read_opt(p))
        .map(|t| parse_os_release(&t))
        .unwrap_or((None, None));
    let mut info = SystemInfo {
        manufacturer: dmi(root, "sys_vendor"),
        product_name: dmi(root, "product_name"),
        product_version: dmi(root, "product_version"),
        product_family: dmi(root, "product_family"),
        board_vendor: dmi(root, "board_vendor"),
        board_name: dmi(root, "board_name"),
        bios_vendor: dmi(root, "bios_vendor"),
        bios_version: dmi(root, "bios_version"),
        bios_date: dmi(root, "bios_date"),
        ec_firmware_release: dmi(root, "ec_firmware_release"),
        cpu_model: legion_cpu::cpu_model(root),
        gpus,
        memory_total_mib: crate::memory::read_memory(root).map(|m| m.total_mib),
        kernel: root.read_opt("/proc/sys/kernel/osrelease"),
        os_name,
        os_id,
        desktop: desktop.and_then(normalise_desktop),
        known_model: None,
    };
    info.known_model = support::lookup(&info).map(|m| m.name.to_owned());
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    #[test]
    fn os_release() {
        let (n, id) =
            parse_os_release("NAME=\"CachyOS Linux\"\nPRETTY_NAME=\"CachyOS\"\nID=cachyos\n");
        assert_eq!(n.as_deref(), Some("CachyOS"));
        assert_eq!(id.as_deref(), Some("cachyos"));
        let (n, _) = parse_os_release("NAME=Arch Linux\n");
        assert_eq!(n.as_deref(), Some("Arch Linux"));
    }

    #[test]
    fn desktop_names() {
        assert_eq!(normalise_desktop("GNOME").as_deref(), Some("GNOME"));
        assert_eq!(normalise_desktop("ubuntu:GNOME").as_deref(), Some("GNOME"));
        assert_eq!(normalise_desktop("Foo").as_deref(), Some("Foo"));
        assert_eq!(normalise_desktop(""), None);
    }

    #[test]
    fn dmi_detection() {
        let fs = FakeSysfs::new();
        let d = "/sys/class/dmi/id";
        fs.file(&format!("{d}/sys_vendor"), "LENOVO\n")
            .file(&format!("{d}/product_name"), "83F0\n")
            .file(&format!("{d}/product_version"), "Legion 5 15IAX10\n")
            .file(&format!("{d}/board_name"), "LNVNB161216\n")
            .file(&format!("{d}/bios_version"), "S2CN26WW\n")
            .file(&format!("{d}/board_vendor"), "Default string\n")
            .file(&format!("{d}/product_serial"), "SHOULD-NOT-BE-READ")
            .file("/proc/sys/kernel/osrelease", "7.2.8-1-cachyos\n")
            .file("/etc/os-release", "PRETTY_NAME=\"CachyOS\"\nID=cachyos\n")
            .file(
                "/proc/cpuinfo",
                "processor : 0\nmodel name : Intel(R) Core(TM) Ultra 7 255HX\n",
            )
            .file(
                "/proc/meminfo",
                "MemTotal: 16777216 kB\nMemAvailable: 8388608 kB\n",
            );
        let info = detect(&fs.root(), Some("GNOME"), vec!["GPU".into()]);
        assert_eq!(info.manufacturer.as_deref(), Some("LENOVO"));
        assert_eq!(info.display_product(), "Legion 5 15IAX10 (83F0)");
        assert_eq!(info.board_vendor, None);
        assert_eq!(info.cpu_model.as_deref(), Some("Intel Core Ultra 7 255HX"));
        assert_eq!(info.memory_total_mib, Some(16384));
        assert_eq!(info.kernel.as_deref(), Some("7.2.8-1-cachyos"));
        assert_eq!(info.known_model.as_deref(), Some("Legion 5 15IAX10"));
        assert!(!serde_json::to_string(&info).unwrap().contains("SHOULD-NOT"));
    }
}
