//! Per-model support matrix.
//!
//! Every entry documents what was observed on real hardware. Two Legion
//! models are **never** assumed to share an EC or firmware interface, so an
//! unknown model gets no model-specific behaviour at all.

use serde::Serialize;

use crate::model::SystemInfo;

/// A model that has been examined on real hardware.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnownModel {
    /// Marketing name (DMI `product_version`).
    pub name: &'static str,
    /// Machine types (DMI `product_name`).
    pub machine_types: &'static [&'static str],
    /// Board names (DMI `board_name`).
    pub boards: &'static [&'static str],
    /// CPU observed on the reference machine.
    pub reference_cpu: &'static str,
    /// GPU observed on the reference machine.
    pub reference_gpu: &'static str,
    /// Kernel drivers observed providing Lenovo features.
    pub observed_drivers: &'static [&'static str],
    /// Free-form notes.
    pub notes: &'static str,
}

/// Models examined so far.
pub const KNOWN_MODELS: &[KnownModel] = &[KnownModel {
    name: "Legion 5 15IAX10",
    machine_types: &["83F0"],
    boards: &["LNVNB161216"],
    reference_cpu: "Intel Core Ultra 7 255HX",
    reference_gpu: "NVIDIA GeForce RTX 5070 Laptop GPU",
    observed_drivers: &[
        "lenovo_wmi_gamezone",
        "lenovo_wmi_other",
        "lenovo_wmi_capdata",
        "lenovo_wmi_events",
        "lenovo_wmi_hotkey_utilities",
        "ideapad_acpi",
    ],
    notes: "Observed on BIOS S2CN26WW with Linux 7.2. platform_profile provided by \
            lenovo-wmi-gamezone; fan RPM and firmware tunables provided by lenovo-wmi-other; \
            battery conservation via ideapad-laptop. Phase 1: read-only.",
}];

/// Looks up the current machine in the support matrix.
///
/// Matching requires a Lenovo vendor string **and** a matching board name or
/// machine type.
pub fn lookup(info: &SystemInfo) -> Option<&'static KnownModel> {
    if !is_lenovo(info) {
        return None;
    }
    KNOWN_MODELS.iter().find(|m| {
        info.board_name
            .as_deref()
            .is_some_and(|b| m.boards.contains(&b))
            || info
                .product_name
                .as_deref()
                .is_some_and(|p| m.machine_types.contains(&p))
    })
}

fn is_lenovo(info: &SystemInfo) -> bool {
    info.manufacturer
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("lenovo"))
}

/// Returns `true` if the machine looks like any Lenovo Legion.
pub fn is_legion(info: &SystemInfo) -> bool {
    let legion = [&info.product_version, &info.product_family]
        .iter()
        .any(|f| {
            f.as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains("legion"))
        });
    is_lenovo(info) && legion
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(vendor: &str, board: &str, product: &str, version: &str) -> SystemInfo {
        SystemInfo {
            manufacturer: Some(vendor.into()),
            board_name: Some(board.into()),
            product_name: Some(product.into()),
            product_version: Some(version.into()),
            ..Default::default()
        }
    }

    #[test]
    fn matches_legion_5_15iax10() {
        let i = info("LENOVO", "LNVNB161216", "83F0", "Legion 5 15IAX10");
        assert_eq!(lookup(&i).unwrap().name, "Legion 5 15IAX10");
        assert!(is_legion(&i));
    }

    #[test]
    fn unknown_legion_is_not_matched() {
        let i = info("LENOVO", "LNVNB999999", "82XX", "Legion 7 16IRX9");
        assert!(lookup(&i).is_none());
        assert!(is_legion(&i));
    }

    #[test]
    fn other_vendor_never_matches() {
        let i = info("ACME", "LNVNB161216", "83F0", "Legion 5 15IAX10");
        assert!(lookup(&i).is_none());
        assert!(!is_legion(&i));
    }
}
