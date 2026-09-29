//! Read-only discovery of ACPI, WMI and Lenovo firmware interfaces.
//!
//! This crate **only enumerates** what the kernel already exposes:
//!
//! * `/sys/firmware/acpi` — ACPI presence and table *names* (table contents
//!   are root-only and never read);
//! * `/sys/bus/acpi/devices` — ACPI device HIDs and namespace paths;
//! * `/sys/bus/wmi/devices` — WMI GUIDs and the driver bound to each;
//! * `/sys/class/firmware-attributes` — firmware tunables published by
//!   drivers such as `lenovo-wmi-other`
//!   (`Documentation/ABI/testing/sysfs-class-firmware-attributes`);
//! * the `ideapad-laptop` platform device (`VPC2004`), reading only a fixed
//!   allow-list of attributes.
//!
//! It never calls ACPI methods directly, never evaluates WMI methods and never
//! touches the Embedded Controller. Unbound WMI GUIDs are listed as-is: their
//! purpose is **not** guessed.
//!
//! The only write is [`set_firmware_attribute`], limited to the attributes
//! documented in `Documentation/wmi/devices/lenovo-wmi-other.rst`, validated
//! against the driver-reported `min_value`/`max_value`/`scalar_increment`.

use serde::{Deserialize, Serialize};

use legion_core::sysfs::{self, SysRoot};

const ACPI_DEVICES: &str = "/sys/bus/acpi/devices";
const WMI_DEVICES: &str = "/sys/bus/wmi/devices";
const FW_ATTR_CLASS: &str = "/sys/class/firmware-attributes";
const IDEAPAD_DEVICE: &str = "/sys/bus/platform/devices/VPC2004:00";

/// ideapad-laptop attributes that are safe to read (documented in
/// `Documentation/ABI/testing/sysfs-platform-ideapad-laptop`).
pub const IDEAPAD_READABLE: &[&str] = &[
    "conservation_mode",
    "fan_mode",
    "fn_lock",
    "usb_charging",
    "camera_power",
    "touchpad",
];

/// Known in-tree (and one well-known out-of-tree) drivers that provide
/// Lenovo laptop features. The descriptions summarise the kernel
/// documentation referenced in `documentation`.
pub const KNOWN_LENOVO_DRIVERS: &[(&str, &str, &str)] = &[
    (
        "lenovo_wmi_gamezone",
        "Gaming-series thermal modes exposed as platform_profile",
        "Documentation/wmi/devices/lenovo-wmi-gamezone.rst",
    ),
    (
        "lenovo_wmi_other",
        "Firmware tunables (CPU/GPU power & thermal limits) via firmware-attributes; fan hwmon",
        "Documentation/wmi/devices/lenovo-wmi-other.rst",
    ),
    (
        "lenovo_wmi_capdata",
        "Capability data (limits/defaults) consumed by lenovo-wmi-other",
        "Linux kernel source, drivers/platform/x86/",
    ),
    (
        "lenovo_wmi_events",
        "WMI event notifications (e.g. thermal-mode hotkey) for gamezone",
        "Linux kernel source, drivers/platform/x86/",
    ),
    (
        "lenovo_wmi_hotkey_utilities",
        "Hotkey / LED utilities",
        "Linux kernel source, drivers/platform/x86/",
    ),
    (
        "ideapad_acpi",
        "ideapad-laptop: conservation mode, Fn lock, USB charging, fan mode (VPC2004)",
        "Documentation/ABI/testing/sysfs-platform-ideapad-laptop",
    ),
    (
        "ideapad_wmi",
        "ideapad-laptop WMI event handler",
        "Linux kernel source, drivers/platform/x86/",
    ),
    (
        "legion_laptop",
        "Out-of-tree LenovoLegionLinux module (not used by Legion Control)",
        "https://github.com/johnfanv2/LenovoLegionLinux",
    ),
];

/// One ACPI device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpiDevice {
    /// sysfs name (`PNP0C14:01`).
    pub id: String,
    /// Hardware id.
    pub hid: Option<String>,
    /// ACPI namespace path (`\_SB_.GZFD`).
    pub path: Option<String>,
    /// `_STA` value, if exposed.
    pub status: Option<String>,
    /// Driver bound to the physical node, if any.
    pub driver: Option<String>,
}

/// One WMI block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WmiDevice {
    /// sysfs name (`GUID-N`).
    pub id: String,
    /// GUID.
    pub guid: String,
    /// Bound driver, if any.
    pub driver: Option<String>,
    /// Number of instances.
    pub instance_count: Option<u32>,
    /// `expensive` flag.
    pub expensive: Option<bool>,
    /// Object id (method/data block) if exposed.
    pub object_id: Option<String>,
    /// Notify id (event block) if exposed.
    pub notify_id: Option<String>,
}

pub use legion_core::model::FirmwareAttribute;

/// A firmware-attributes class device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareAttributeSet {
    /// Class device name (`lenovo-wmi-other-0`).
    pub name: String,
    /// Attributes.
    pub attributes: Vec<FirmwareAttribute>,
}

/// A read-only view of one platform-device attribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAttribute {
    /// Attribute name.
    pub name: String,
    /// Raw value.
    pub value: Option<String>,
    /// Whether the attribute has a write bit (informational only).
    pub writable_by_root: bool,
}

/// A detected Lenovo driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LenovoDriver {
    /// Kernel driver name.
    pub driver: String,
    /// Loaded / bound.
    pub loaded: bool,
    /// Devices bound to it.
    pub bound_devices: Vec<String>,
    /// Summary from the kernel docs.
    pub description: String,
    /// Where the behaviour is documented.
    pub documentation: String,
}

/// Lenovo-specific interfaces found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LenovoInterfaces {
    /// Known Lenovo drivers that are loaded.
    pub drivers: Vec<LenovoDriver>,
    /// Firmware-attribute sets published by Lenovo drivers.
    pub firmware_attributes: Vec<FirmwareAttributeSet>,
    /// ideapad-laptop platform device attributes.
    pub ideapad: Vec<DeviceAttribute>,
    /// WMI GUIDs not bound to any driver (purpose unknown).
    pub unbound_wmi_guids: Vec<String>,
    /// Lenovo-specific ACPI HIDs found (`VPC2004`, `LEN*`).
    pub acpi_hids: Vec<String>,
}

/// Full ACPI / WMI discovery report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpiReport {
    /// `/sys/firmware/acpi` exists.
    pub acpi_present: bool,
    /// ACPI table names (contents are not read).
    pub tables: Vec<String>,
    /// ACPI devices with a HID.
    pub devices: Vec<AcpiDevice>,
    /// WMI bus present.
    pub wmi_present: bool,
    /// WMI blocks.
    pub wmi_devices: Vec<WmiDevice>,
    /// Lenovo-specific findings.
    pub lenovo: LenovoInterfaces,
}

fn acpi_devices(root: &SysRoot) -> Vec<AcpiDevice> {
    root.list(ACPI_DEVICES)
        .into_iter()
        .filter_map(|id| {
            let dir = format!("{ACPI_DEVICES}/{id}");
            let hid = root.read_opt(format!("{dir}/hid"))?;
            Some(AcpiDevice {
                hid: Some(hid),
                path: root.read_opt(format!("{dir}/path")),
                status: root.read_opt(format!("{dir}/status")),
                driver: sysfs::link_name(&root.path(format!("{dir}/physical_node/driver"))),
                id,
            })
        })
        .collect()
}

fn wmi_devices(root: &SysRoot) -> Vec<WmiDevice> {
    root.list(WMI_DEVICES)
        .into_iter()
        .map(|id| {
            let dir = format!("{WMI_DEVICES}/{id}");
            let guid = root.read_opt(format!("{dir}/guid")).unwrap_or_else(|| {
                id.rsplit_once('-')
                    .map(|(g, _)| g.to_owned())
                    .unwrap_or_default()
            });
            WmiDevice {
                guid,
                driver: sysfs::link_name(&root.path(format!("{dir}/driver"))),
                instance_count: root.read_parse_opt(format!("{dir}/instance_count")),
                expensive: root.read_opt(format!("{dir}/expensive")).map(|v| v == "1"),
                object_id: root.read_opt(format!("{dir}/object_id")),
                notify_id: root.read_opt(format!("{dir}/notify_id")),
                id,
            }
        })
        .collect()
}

fn firmware_attributes(root: &SysRoot) -> Vec<FirmwareAttributeSet> {
    root.list(FW_ATTR_CLASS)
        .into_iter()
        .filter(|n| n.starts_with("lenovo"))
        .map(|name| {
            let base = format!("{FW_ATTR_CLASS}/{name}/attributes");
            let attributes = root
                .list(&base)
                .into_iter()
                .filter(|a| root.path(format!("{base}/{a}")).is_dir())
                .map(|a| {
                    let dir = format!("{base}/{a}");
                    let r = |f: &str| root.read_opt(format!("{dir}/{f}"));
                    FirmwareAttribute {
                        display_name: r("display_name"),
                        kind: r("type"),
                        current_value: r("current_value"),
                        default_value: r("default_value"),
                        min_value: r("min_value"),
                        max_value: r("max_value"),
                        scalar_increment: r("scalar_increment"),
                        possible_values: r("possible_values"),
                        writable_by_root: sysfs::is_writable_mode(
                            &root.path(format!("{dir}/current_value")),
                        ),
                        name: a,
                    }
                })
                .collect();
            FirmwareAttributeSet { name, attributes }
        })
        .collect()
}

fn ideapad_attributes(root: &SysRoot) -> Vec<DeviceAttribute> {
    if !root.exists(IDEAPAD_DEVICE) {
        return vec![];
    }
    IDEAPAD_READABLE
        .iter()
        .filter(|a| root.exists(format!("{IDEAPAD_DEVICE}/{a}")))
        .map(|a| DeviceAttribute {
            name: (*a).to_owned(),
            value: root.read_opt(format!("{IDEAPAD_DEVICE}/{a}")),
            writable_by_root: sysfs::is_writable_mode(&root.path(format!("{IDEAPAD_DEVICE}/{a}"))),
        })
        .collect()
}

fn lenovo_drivers(root: &SysRoot, wmi: &[WmiDevice]) -> Vec<LenovoDriver> {
    KNOWN_LENOVO_DRIVERS
        .iter()
        .filter_map(|(drv, desc, doc)| {
            let mut bound: Vec<String> = wmi
                .iter()
                .filter(|w| w.driver.as_deref() == Some(*drv))
                .map(|w| w.guid.clone())
                .collect();
            if bound.is_empty() {
                // platform drivers (ideapad_acpi) are bound outside the WMI bus
                bound = root
                    .list(format!("/sys/bus/platform/drivers/{drv}"))
                    .into_iter()
                    .filter(|e| e.contains(':'))
                    .collect();
            }
            let module = drv.replace('-', "_");
            let loaded = !bound.is_empty() || root.exists(format!("/sys/module/{module}"));
            loaded.then(|| LenovoDriver {
                driver: (*drv).to_owned(),
                loaded,
                bound_devices: bound,
                description: (*desc).to_owned(),
                documentation: (*doc).to_owned(),
            })
        })
        .collect()
}

/// Runs the full read-only discovery.
pub fn scan(root: &SysRoot) -> AcpiReport {
    let acpi_present = root.exists("/sys/firmware/acpi");
    let devices = acpi_devices(root);
    let wmi_present = root.exists(WMI_DEVICES);
    let wmi = wmi_devices(root);
    let mut acpi_hids: Vec<String> = devices
        .iter()
        .filter_map(|d| d.hid.clone())
        .filter(|h| h == "VPC2004" || h.starts_with("LEN") || h.starts_with("IDEA"))
        .collect();
    acpi_hids.sort();
    acpi_hids.dedup();
    let lenovo = LenovoInterfaces {
        drivers: lenovo_drivers(root, &wmi),
        firmware_attributes: firmware_attributes(root),
        ideapad: ideapad_attributes(root),
        unbound_wmi_guids: wmi
            .iter()
            .filter(|w| w.driver.is_none())
            .map(|w| w.guid.clone())
            .collect(),
        acpi_hids,
    };
    AcpiReport {
        acpi_present,
        tables: root
            .list("/sys/firmware/acpi/tables")
            .into_iter()
            .filter(|t| !root.path(format!("/sys/firmware/acpi/tables/{t}")).is_dir())
            .collect(),
        devices,
        wmi_present,
        wmi_devices: wmi,
        lenovo,
    }
}

/// Attributes documented for `lenovo-wmi-other`
/// (`Documentation/wmi/devices/lenovo-wmi-other.rst`). Nothing else is ever
/// written.
pub const DOCUMENTED_LENOVO_ATTRIBUTES: &[&str] = &[
    "cpu_temp",
    "gpu_temp",
    "gpu_nv_ac_offset",
    "gpu_nv_bpl",
    "gpu_nv_cpu_boost",
    "gpu_nv_ctgp",
    "gpu_nv_ppab",
    "ppt_cpu_cl",
    "ppt_pl1_apu_spl",
    "ppt_pl1_spl",
    "ppt_pl1_spl_cl",
    "ppt_pl1_tau",
    "ppt_pl2_sppt",
    "ppt_pl2_sppt_cl",
    "ppt_pl3_fppt",
    "ppt_pl3_fppt_cl",
    "ppt_pl4_ipl",
    "ppt_pl4_ipl_cl",
];

/// Reads only the Lenovo firmware attributes (cheaper than a full [`scan`]).
pub fn read_firmware_attributes(root: &SysRoot) -> Vec<FirmwareAttributeSet> {
    firmware_attributes(root)
}

/// Validates `value` for an attribute.
pub fn validate_firmware_value(attr: &FirmwareAttribute, value: i64) -> legion_core::Result<()> {
    use legion_core::HwError;
    if !DOCUMENTED_LENOVO_ATTRIBUTES.contains(&attr.name.as_str()) {
        return Err(HwError::NotSupported(format!(
            "{} is not a documented lenovo-wmi-other attribute",
            attr.name
        )));
    }
    let Some((min, max, step)) = attr.range() else {
        return Err(HwError::NotSupported(format!(
            "{} is not adjustable on this machine (driver reports no range)",
            attr.name
        )));
    };
    if !(min..=max).contains(&value) {
        return Err(HwError::InvalidValue(format!(
            "{} must be between {min} and {max}, got {value}",
            attr.name
        )));
    }
    if (value - min) % step != 0 {
        return Err(HwError::InvalidValue(format!(
            "{} must be a multiple of {step} from {min}",
            attr.name
        )));
    }
    Ok(())
}

/// Writes a Lenovo firmware attribute after validation. Callers must make
/// sure the `custom` platform profile is active, otherwise the firmware
/// ignores the value.
pub fn set_firmware_attribute(root: &SysRoot, name: &str, value: i64) -> legion_core::Result<()> {
    let sets = firmware_attributes(root);
    let (set, attr) = sets
        .iter()
        .find_map(|s| s.attributes.iter().find(|a| a.name == name).map(|a| (s, a)))
        .ok_or_else(|| legion_core::HwError::NotSupported(format!("firmware attribute {name}")))?;
    validate_firmware_value(attr, value)?;
    sysfs::write_attr(
        root,
        &format!(
            "{FW_ATTR_CLASS}/{}/attributes/{name}/current_value",
            set.name
        ),
        &value.to_string(),
    )?;
    Ok(())
}

/// Finds a firmware attribute by name in any Lenovo set.
pub fn find_firmware_attribute<'a>(
    report: &'a AcpiReport,
    name: &str,
) -> Option<&'a FirmwareAttribute> {
    report
        .lenovo
        .firmware_attributes
        .iter()
        .flat_map(|s| s.attributes.iter())
        .find(|a| a.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    fn legion() -> FakeSysfs {
        let fs = FakeSysfs::new();
        let gz = "/sys/bus/wmi/devices/887B54E3-DDDC-4B2C-8B88-68A26A8835D0-2";
        let unk = "/sys/bus/wmi/devices/0F7B30EC-AC90-4B34-B62F-87F85115505D-26";
        let fw = "/sys/class/firmware-attributes/lenovo-wmi-other-0/attributes/ppt_pl1_spl";
        fs.dir("/sys/firmware/acpi")
            .file("/sys/firmware/acpi/tables/DSDT", "")
            .dir("/sys/firmware/acpi/tables/dynamic")
            .file("/sys/bus/acpi/devices/VPC2004:00/hid", "VPC2004")
            .file(
                "/sys/bus/acpi/devices/VPC2004:00/path",
                "\\_SB_.PC00.LPCB.EC0_.VPC0",
            )
            .file("/sys/bus/acpi/devices/PNP0C14:01/hid", "PNP0C14")
            .dir("/sys/bus/acpi/devices/LNXCPU:00")
            .file(
                &format!("{gz}/guid"),
                "887B54E3-DDDC-4B2C-8B88-68A26A8835D0",
            )
            .file(&format!("{gz}/instance_count"), "1")
            .file(&format!("{gz}/expensive"), "0")
            .file(&format!("{gz}/object_id"), "AA")
            .symlink(
                &format!("{gz}/driver"),
                "/sys/bus/wmi/drivers/lenovo_wmi_gamezone",
            )
            .file(
                &format!("{unk}/guid"),
                "0F7B30EC-AC90-4B34-B62F-87F85115505D",
            )
            .file(&format!("{fw}/current_value"), "105")
            .mode(&format!("{fw}/current_value"), 0o644)
            .file(&format!("{fw}/min_value"), "50")
            .file(&format!("{fw}/max_value"), "105")
            .file(&format!("{fw}/type"), "integer")
            .file(
                &format!("{fw}/display_name"),
                "Set the CPU sustained power limit",
            )
            .file(
                "/sys/bus/platform/devices/VPC2004:00/conservation_mode",
                "1",
            )
            .file("/sys/bus/platform/devices/VPC2004:00/fan_mode", "0")
            .file("/sys/bus/platform/devices/VPC2004:00/secret_thing", "x")
            .dir("/sys/bus/platform/drivers/ideapad_acpi/VPC2004:00");
        fs
    }

    #[test]
    fn scans_legion_interfaces() {
        let fs = legion();
        let r = scan(&fs.root());
        assert!(r.acpi_present);
        assert_eq!(r.tables, ["DSDT"]);
        assert_eq!(r.devices.len(), 2);
        assert!(r.wmi_present);
        assert_eq!(r.wmi_devices.len(), 2);
        let gz = r
            .wmi_devices
            .iter()
            .find(|w| w.guid.starts_with("887B54E3"))
            .unwrap();
        assert_eq!(gz.driver.as_deref(), Some("lenovo_wmi_gamezone"));
        assert_eq!(gz.expensive, Some(false));
        assert_eq!(
            r.lenovo.unbound_wmi_guids,
            ["0F7B30EC-AC90-4B34-B62F-87F85115505D"]
        );
        let drivers: Vec<_> = r.lenovo.drivers.iter().map(|d| d.driver.as_str()).collect();
        assert_eq!(drivers, ["lenovo_wmi_gamezone", "ideapad_acpi"]);
        assert_eq!(r.lenovo.acpi_hids, ["VPC2004"]);
        let pl1 = find_firmware_attribute(&r, "ppt_pl1_spl").unwrap();
        assert_eq!(pl1.current_value.as_deref(), Some("105"));
        assert_eq!(pl1.max_value.as_deref(), Some("105"));
        assert!(pl1.writable_by_root);
        // only allow-listed ideapad attributes are read
        let names: Vec<_> = r.lenovo.ideapad.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["conservation_mode", "fan_mode"]);
    }

    #[test]
    fn firmware_attribute_writes() {
        let fs = legion();
        let root = fs.root();
        let fw = "/sys/class/firmware-attributes/lenovo-wmi-other-0/attributes/ppt_pl1_spl";
        set_firmware_attribute(&root, "ppt_pl1_spl", 90).unwrap();
        assert_eq!(root.read(format!("{fw}/current_value")).unwrap(), "90");
        assert!(set_firmware_attribute(&root, "ppt_pl1_spl", 200).is_err());
        assert!(set_firmware_attribute(&root, "ppt_pl1_spl", 49).is_err());
        assert!(set_firmware_attribute(&root, "nonexistent", 1).is_err());
        assert_eq!(root.read(format!("{fw}/current_value")).unwrap(), "90");
    }

    #[test]
    fn non_adjustable_attribute_is_refused() {
        let fs = legion();
        let d = "/sys/class/firmware-attributes/lenovo-wmi-other-0/attributes/gpu_nv_ctgp";
        fs.file(&format!("{d}/current_value"), "100")
            .mode(&format!("{d}/current_value"), 0o644)
            .file(&format!("{d}/min_value"), "0")
            .file(&format!("{d}/max_value"), "0")
            .file(&format!("{d}/type"), "integer");
        let e = set_firmware_attribute(&fs.root(), "gpu_nv_ctgp", 90).unwrap_err();
        assert!(e.to_string().contains("not adjustable"));
    }

    #[test]
    fn non_acpi_machine() {
        let fs = FakeSysfs::new();
        let r = scan(&fs.root());
        assert!(!r.acpi_present);
        assert!(!r.wmi_present);
        assert!(r.lenovo.drivers.is_empty());
    }
}
