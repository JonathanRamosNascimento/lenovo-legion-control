//! Finding the built-in keyboard in sysfs.
//!
//! Everything here is read from `/sys/class/hidraw` (world-readable), so
//! detection works without privileges and against a fake tree in tests. A
//! device is only ever considered when **all** of these hold:
//!
//! 1. it is a USB HID interface of an ITE controller (vendor `048d`);
//! 2. its USB device is **non-removable** (`removable` = `fixed`, as declared
//!    by the firmware), so external keyboards are never touched;
//! 3. its report descriptor declares the vendor collection `0xff89/0x07` with
//!    the 960-byte feature report ([`spectrum::check_descriptor`]);
//! 4. its USB id is in [`VERIFIED_IDS`] — controllers whose answers were
//!    checked on real hardware. Others are reported but never written.
//!
//! Once the device is opened (root only), the controller must also declare
//! itself compatible.

use std::path::{Path, PathBuf};

use legion_core::SysRoot;

use crate::hid::Descriptor;
use crate::spectrum;

const HIDRAW: &str = "/sys/class/hidraw";
/// HID bus id of USB devices in `HID_ID`.
const BUS_USB: u32 = 0x0003;

/// A `hidrawN` node and what sysfs says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidNode {
    /// Node name (`hidraw6`).
    pub name: String,
    /// USB vendor id.
    pub vendor_id: u16,
    /// USB product id.
    pub product_id: u16,
    /// HID name (`HID_NAME`).
    pub hid_name: String,
    /// `removable` attribute of the USB device (`fixed`, `removable`, `unknown`).
    pub removable: Option<String>,
    /// Raw report descriptor.
    pub descriptor: Vec<u8>,
}

impl HidNode {
    /// Device node path.
    pub fn dev_path(&self) -> String {
        format!("/dev/{}", self.name)
    }

    /// `048d:c195`.
    pub fn usb_id(&self) -> String {
        format!("{:04x}:{:04x}", self.vendor_id, self.product_id)
    }

    /// Whether the USB device is declared non-removable.
    pub fn is_builtin(&self) -> bool {
        self.removable.as_deref() == Some("fixed")
    }
}

/// Parses `0003:0000048D:0000C195` into `(bus, vendor, product)`.
fn parse_hid_id(s: &str) -> Option<(u32, u16, u16)> {
    let mut it = s.split(':');
    let bus = u32::from_str_radix(it.next()?, 16).ok()?;
    let vid = u32::from_str_radix(it.next()?, 16).ok()?;
    let pid = u32::from_str_radix(it.next()?, 16).ok()?;
    Some((bus, u16::try_from(vid).ok()?, u16::try_from(pid).ok()?))
}

/// Walks up from a HID device directory to its USB device (the directory that
/// has `idVendor`).
fn usb_device_dir(hid_dir: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(hid_dir).ok()?;
    real.ancestors()
        .skip(1)
        .take(6)
        .find(|p| p.join("idVendor").is_file())
        .map(Path::to_path_buf)
}

/// Lists the USB `hidraw` nodes.
pub fn scan(root: &SysRoot) -> Vec<HidNode> {
    let mut out = Vec::new();
    for name in root.list(HIDRAW) {
        if !name.starts_with("hidraw") {
            continue;
        }
        let dir = root.path(format!("{HIDRAW}/{name}/device"));
        let Ok(uevent) = std::fs::read_to_string(dir.join("uevent")) else {
            continue;
        };
        let prop = |key: &str| {
            uevent
                .lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        };
        let Some((BUS_USB, vendor_id, product_id)) = prop("HID_ID").and_then(parse_hid_id) else {
            continue; // I²C touchpads, Bluetooth, …
        };
        let Ok(descriptor) = std::fs::read(dir.join("report_descriptor")) else {
            continue;
        };
        let removable = usb_device_dir(&dir).and_then(|usb| {
            std::fs::read_to_string(usb.join("removable"))
                .ok()
                .map(|s| s.trim().to_owned())
        });
        out.push(HidNode {
            name,
            vendor_id,
            product_id,
            hid_name: prop("HID_NAME").unwrap_or_default().to_owned(),
            removable,
            descriptor,
        });
    }
    out
}

/// What a hardware report records about an ITE USB HID interface (ids and
/// report layout only; nothing personal).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NodeSummary {
    /// `hidraw` node.
    pub node: String,
    /// USB id.
    pub usb_id: String,
    /// HID name.
    pub name: String,
    /// `removable` attribute of the USB device.
    pub removable: Option<String>,
    /// Top-level collections as `usage page/usage` (hex).
    pub collections: Vec<String>,
    /// Payload size of feature report 7, when declared.
    pub feature_report_7_bytes: Option<usize>,
}

/// Summarises every ITE USB HID interface (to extend the support matrix).
pub fn summarize(root: &SysRoot) -> Vec<NodeSummary> {
    scan(root)
        .into_iter()
        .filter(|n| n.vendor_id == VENDOR_ITE)
        .map(|n| {
            let desc = Descriptor::parse(&n.descriptor).unwrap_or_default();
            NodeSummary {
                usb_id: n.usb_id(),
                collections: desc
                    .applications()
                    .into_iter()
                    .map(|(p, u)| format!("{p:#06x}/{u:#04x}"))
                    .collect(),
                feature_report_7_bytes: match desc.report_bytes(crate::hid::ReportKind::Feature, 7)
                {
                    0 => None,
                    n => Some(n),
                },
                node: n.name,
                name: n.hid_name,
                removable: n.removable,
            }
        })
        .collect()
}

/// ITE Tech. Inc.
pub const VENDOR_ITE: u16 = 0x048d;

/// USB ids whose Spectrum answers were verified on real hardware.
pub const VERIFIED_IDS: &[(u16, u16, &str)] = &[(VENDOR_ITE, 0xc195, "Legion 5 15IAX10 (83F0)")];

/// Outcome of [`detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// A built-in controller with the expected report and a verified id.
    Found(Box<HidNode>),
    /// A built-in Spectrum-like controller whose id has not been verified.
    Unverified(Box<HidNode>),
    /// A candidate whose descriptor does not match (reason attached).
    Rejected(String),
    /// Nothing that looks like a Spectrum keyboard controller.
    Absent,
}

/// Looks for the built-in keyboard controller (sysfs only, no privileges).
pub fn detect(root: &SysRoot) -> Detection {
    let mut best = Detection::Absent;
    for node in scan(root) {
        if node.vendor_id != VENDOR_ITE || !node.is_builtin() {
            continue;
        }
        let Ok(desc) = Descriptor::parse(&node.descriptor) else {
            continue;
        };
        if !desc
            .applications()
            .contains(&(spectrum::USAGE_PAGE, spectrum::USAGE_APPLICATION))
        {
            continue;
        }
        if let Err(why) = spectrum::check_descriptor(&desc) {
            best = Detection::Rejected(format!("{} ({}): {why}", node.hid_name, node.usb_id()));
            continue;
        }
        if VERIFIED_IDS
            .iter()
            .any(|(v, p, _)| (*v, *p) == (node.vendor_id, node.product_id))
        {
            return Detection::Found(Box::new(node));
        }
        best = Detection::Unverified(Box::new(node));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptors::{ITE_8258_KEYBOARD_DESCRIPTOR, ITE_8258_LAMPARRAY_DESCRIPTOR};
    use crate::sim::{add_legion_5_15iax10_keyboard, add_node};
    use legion_core::testing::FakeSysfs;

    #[test]
    fn finds_the_vendor_interface_not_the_lamparray_one() {
        let fs = FakeSysfs::new();
        add_legion_5_15iax10_keyboard(&fs);
        let nodes = scan(&fs.root());
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].usb_id(), "048d:c195");
        assert_eq!(nodes[0].removable.as_deref(), Some("fixed"));
        assert_eq!(nodes[0].dev_path(), "/dev/hidraw5");
        match detect(&fs.root()) {
            Detection::Found(n) => assert_eq!(n.name, "hidraw5"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_non_removable_devices_are_considered() {
        for removable in ["removable", "unknown"] {
            let fs = FakeSysfs::new();
            add_node(
                &fs,
                5,
                0,
                removable,
                0x048d,
                0xc195,
                ITE_8258_KEYBOARD_DESCRIPTOR,
            );
            assert_eq!(detect(&fs.root()), Detection::Absent, "{removable}");
        }
    }

    #[test]
    fn other_vendors_are_ignored() {
        let fs = FakeSysfs::new();
        add_node(
            &fs,
            5,
            0,
            "fixed",
            0x046d,
            0xc339,
            ITE_8258_KEYBOARD_DESCRIPTOR,
        );
        assert_eq!(detect(&fs.root()), Detection::Absent);
    }

    #[test]
    fn unlisted_ite_ids_are_reported_but_not_usable() {
        let fs = FakeSysfs::new();
        add_node(
            &fs,
            5,
            0,
            "fixed",
            0x048d,
            0xc197,
            ITE_8258_KEYBOARD_DESCRIPTOR,
        );
        match detect(&fs.root()) {
            Detection::Unverified(n) => assert_eq!(n.usb_id(), "048d:c197"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lamparray_interface_alone_is_absent() {
        let fs = FakeSysfs::new();
        add_node(
            &fs,
            6,
            1,
            "fixed",
            0x048d,
            0xc195,
            ITE_8258_LAMPARRAY_DESCRIPTOR,
        );
        assert_eq!(detect(&fs.root()), Detection::Absent);
        assert_eq!(detect(&FakeSysfs::new().root()), Detection::Absent);
    }

    #[test]
    fn unexpected_report_sizes_are_rejected_with_a_reason() {
        let fs = FakeSysfs::new();
        let mut raw = ITE_8258_KEYBOARD_DESCRIPTOR.to_vec();
        // the 959-byte report 7 is declared as `96 bf 03` (report count 0x03bf)
        let at = raw
            .windows(3)
            .position(|w| w == [0x96, 0xbf, 0x03])
            .unwrap();
        raw[at + 1] = 0xbe;
        add_node(&fs, 5, 0, "fixed", 0x048d, 0xc195, &raw);
        assert!(matches!(detect(&fs.root()), Detection::Rejected(m) if m.contains("layout")));
    }

    #[test]
    fn summaries_list_ite_interfaces_only() {
        let fs = FakeSysfs::new();
        add_legion_5_15iax10_keyboard(&fs);
        let s = summarize(&fs.root());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].usb_id, "048d:c195");
        assert!(s[0].collections.contains(&"0xff89/0x07".to_string()));
        assert_eq!(s[0].feature_report_7_bytes, Some(959));
        assert_eq!(s[1].collections, ["0x0059/0x01"]);
        assert_eq!(s[1].feature_report_7_bytes, None);
        // other vendors are not listed
        let fs = FakeSysfs::new();
        add_node(
            &fs,
            5,
            0,
            "fixed",
            0x046d,
            0xc339,
            ITE_8258_KEYBOARD_DESCRIPTOR,
        );
        assert!(summarize(&fs.root()).is_empty());
    }

    #[test]
    fn hid_id_parsing() {
        assert_eq!(
            parse_hid_id("0003:0000048D:0000C195"),
            Some((3, 0x048d, 0xc195))
        );
        assert_eq!(
            parse_hid_id("0018:000027C6:000001E0"),
            Some((0x18, 0x27c6, 0x01e0))
        );
        assert_eq!(parse_hid_id("junk"), None);
        assert_eq!(parse_hid_id("0003:00010000:0001"), None);
    }

    #[test]
    fn non_usb_nodes_are_skipped() {
        let fs = FakeSysfs::new();
        let hid = "/sys/devices/pci/i2c-1/0018:27C6:01E0.000F";
        fs.file(
            &format!("{hid}/uevent"),
            "HID_ID=0018:000027C6:000001E0\nHID_NAME=GXTP5100:00 27C6:01E0\n",
        );
        std::fs::write(
            fs.root().path(format!("{hid}/report_descriptor")),
            ITE_8258_KEYBOARD_DESCRIPTOR,
        )
        .unwrap();
        fs.symlink(&format!("{HIDRAW}/hidraw14/device"), hid);
        assert!(scan(&fs.root()).is_empty());
    }
}
