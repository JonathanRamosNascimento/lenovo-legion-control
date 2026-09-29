//! Power profile model and the mapping between logical profiles and the raw
//! names exposed by the kernel.
//!
//! The kernel `platform_profile` ABI
//! (`Documentation/ABI/testing/sysfs-platform_profile`) defines the possible
//! names `low-power`, `cool`, `quiet`, `balanced`, `balanced-performance`,
//! `performance`, `max-power` and `custom`. Each driver exposes only a subset,
//! so the mapping is always computed from the real `platform_profile_choices`
//! and never assumed.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Logical power profile shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerProfile {
    /// Quiet / low power.
    Quiet,
    /// Balanced.
    Balanced,
    /// Performance.
    Performance,
    /// Maximum power (`max-power` in the kernel ABI).
    MaxPower,
    /// Firmware custom mode.
    Custom,
    /// Anything that does not map to the above.
    Unknown,
}

impl PowerProfile {
    /// Logical profiles that may be offered to users, in display order.
    pub const SELECTABLE: [PowerProfile; 5] = [
        PowerProfile::Quiet,
        PowerProfile::Balanced,
        PowerProfile::Performance,
        PowerProfile::MaxPower,
        PowerProfile::Custom,
    ];

    /// Raw kernel names that can represent this logical profile, in order of
    /// preference.
    pub fn kernel_candidates(self) -> &'static [&'static str] {
        match self {
            PowerProfile::Quiet => &["quiet", "low-power", "cool"],
            PowerProfile::Balanced => &["balanced"],
            PowerProfile::Performance => &["performance"],
            PowerProfile::MaxPower => &["max-power"],
            PowerProfile::Custom => &["custom"],
            PowerProfile::Unknown => &[],
        }
    }

    /// Maps a raw `platform_profile` value to a logical profile.
    pub fn from_platform_profile(raw: &str) -> PowerProfile {
        let raw = raw.trim();
        PowerProfile::SELECTABLE
            .into_iter()
            .find(|p| p.kernel_candidates().contains(&raw))
            .unwrap_or(PowerProfile::Unknown)
    }

    /// Maps a power-profiles-daemon profile name (`power-saver`, `balanced`,
    /// `performance`).
    pub fn from_ppd(raw: &str) -> PowerProfile {
        match raw.trim() {
            "power-saver" => PowerProfile::Quiet,
            "balanced" => PowerProfile::Balanced,
            "performance" => PowerProfile::Performance,
            _ => PowerProfile::Unknown,
        }
    }

    /// CLI-style name.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerProfile::Quiet => "quiet",
            PowerProfile::Balanced => "balanced",
            PowerProfile::Performance => "performance",
            PowerProfile::MaxPower => "max-power",
            PowerProfile::Custom => "custom",
            PowerProfile::Unknown => "unknown",
        }
    }
}

impl fmt::Display for PowerProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PowerProfile {
    type Err = crate::HwError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "quiet" => Ok(PowerProfile::Quiet),
            "balanced" => Ok(PowerProfile::Balanced),
            "performance" => Ok(PowerProfile::Performance),
            "max-power" | "max_power" | "maxpower" => Ok(PowerProfile::MaxPower),
            "custom" => Ok(PowerProfile::Custom),
            other => Err(crate::HwError::InvalidValue(format!(
                "unknown profile {other:?} (expected quiet, balanced, performance, max-power or custom)"
            ))),
        }
    }
}

/// One entry of the logical ↔ raw mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMapping {
    /// Logical profile.
    pub logical: PowerProfile,
    /// Raw kernel name that implements it on this machine.
    pub raw: String,
}

/// Builds the mapping for the choices the kernel actually offers.
///
/// Raw names with no logical equivalent are mapped to
/// [`PowerProfile::Unknown`] so they are still visible.
pub fn build_mapping(choices: &[String]) -> Vec<ProfileMapping> {
    let mut out = Vec::new();
    for logical in PowerProfile::SELECTABLE {
        if let Some(raw) = logical
            .kernel_candidates()
            .iter()
            .find(|c| choices.iter().any(|x| x == *c))
        {
            out.push(ProfileMapping {
                logical,
                raw: (*raw).to_owned(),
            });
        }
    }
    for raw in choices {
        if !out.iter().any(|m| &m.raw == raw) {
            out.push(ProfileMapping {
                logical: PowerProfile::Unknown,
                raw: raw.clone(),
            });
        }
    }
    out
}

/// A `platform_profile` class device (`/sys/class/platform-profile/*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformProfileHandler {
    /// Class device name (`platform-profile-0`).
    pub id: String,
    /// Driver-provided name (`lenovo-wmi-gamezone`).
    pub name: Option<String>,
    /// Current raw profile of this handler.
    pub profile: Option<String>,
    /// Choices of this handler.
    pub choices: Vec<String>,
}

/// State of `/sys/firmware/acpi/platform_profile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformProfileInfo {
    /// Current raw value.
    pub current_raw: Option<String>,
    /// Current logical value.
    pub current: PowerProfile,
    /// Raw choices.
    pub choices: Vec<String>,
    /// Logical ↔ raw mapping.
    pub mapping: Vec<ProfileMapping>,
    /// Individual handlers registered with the platform_profile class.
    pub handlers: Vec<PlatformProfileHandler>,
    /// Whether the attribute is writable by root (informational).
    pub writable_by_root: bool,
}

/// One profile offered by power-profiles-daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PpdProfile {
    /// Profile name (`power-saver`, `balanced`, `performance`).
    pub profile: String,
    /// Driver backing it (`platform_profile`, `intel_pstate`, `multiple`...).
    pub driver: Option<String>,
}

/// State of power-profiles-daemon (read over D-Bus).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PpdInfo {
    /// Bus name that answered.
    pub bus_name: String,
    /// Active profile.
    pub active: String,
    /// Offered profiles.
    pub profiles: Vec<PpdProfile>,
    /// `PerformanceDegraded` reason, empty when not degraded.
    pub performance_degraded: Option<String>,
}

/// Which backend is authoritative for the current profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileBackend {
    /// power-profiles-daemon (preferred).
    PowerProfilesDaemon,
    /// Kernel `platform_profile`.
    PlatformProfile,
    /// None available.
    None,
}

/// Aggregated power profile state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerProfileState {
    /// Logical profile currently active (from the most precise source).
    pub current: PowerProfile,
    /// Preferred backend for future changes (Phase 2).
    pub backend: ProfileBackend,
    /// Kernel platform_profile, if present.
    pub platform_profile: Option<PlatformProfileInfo>,
    /// power-profiles-daemon, if running.
    pub ppd: Option<PpdInfo>,
    /// Problems encountered while probing (e.g. D-Bus unreachable).
    pub notes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn legion_gamezone_choices() {
        // Real choices from a Legion 5 15IAX10 (lenovo-wmi-gamezone).
        let m = build_mapping(&s(&[
            "low-power",
            "balanced",
            "performance",
            "max-power",
            "custom",
        ]));
        let get = |p| m.iter().find(|x| x.logical == p).map(|x| x.raw.as_str());
        assert_eq!(get(PowerProfile::Quiet), Some("low-power"));
        assert_eq!(get(PowerProfile::Balanced), Some("balanced"));
        assert_eq!(get(PowerProfile::Performance), Some("performance"));
        assert_eq!(get(PowerProfile::MaxPower), Some("max-power"));
        assert_eq!(get(PowerProfile::Custom), Some("custom"));
        assert!(m.iter().all(|x| x.logical != PowerProfile::Unknown));
    }

    #[test]
    fn prefers_quiet_over_low_power() {
        let m = build_mapping(&s(&["quiet", "low-power", "balanced"]));
        assert_eq!(m[0].raw, "quiet");
        // low-power is kept visible as unmapped.
        assert!(
            m.iter()
                .any(|x| x.raw == "low-power" && x.logical == PowerProfile::Unknown)
        );
    }

    #[test]
    fn unknown_names_are_kept() {
        let m = build_mapping(&s(&["balanced-performance", "balanced"]));
        assert_eq!(m.len(), 2);
        assert!(m.iter().any(|x| x.raw == "balanced-performance"));
        assert!(!m.iter().any(|x| x.logical == PowerProfile::Performance));
    }

    #[test]
    fn raw_to_logical() {
        assert_eq!(
            PowerProfile::from_platform_profile("low-power"),
            PowerProfile::Quiet
        );
        assert_eq!(
            PowerProfile::from_platform_profile("max-power\n"),
            PowerProfile::MaxPower
        );
        assert_eq!(
            PowerProfile::from_platform_profile("weird"),
            PowerProfile::Unknown
        );
        assert_eq!(PowerProfile::from_ppd("power-saver"), PowerProfile::Quiet);
    }

    #[test]
    fn parse_cli_names() {
        assert_eq!(
            "Quiet".parse::<PowerProfile>().unwrap(),
            PowerProfile::Quiet
        );
        assert_eq!(
            "max-power".parse::<PowerProfile>().unwrap(),
            PowerProfile::MaxPower
        );
        assert!("turbo".parse::<PowerProfile>().is_err());
        assert!("../../etc".parse::<PowerProfile>().is_err());
    }
}
