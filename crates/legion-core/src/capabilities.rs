//! Capability reporting.
//!
//! A capability is only [`CapabilityState::Supported`] when Legion Control has
//! an implementation that was validated for the interface. An interface that
//! merely *exists* but whose write behaviour has not been validated is
//! reported as [`CapabilityState::Detected`].

use serde::{Deserialize, Serialize};

/// Support state of a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    /// Implemented and usable.
    Supported,
    /// Implemented as read-only monitoring.
    ReadOnly,
    /// A kernel interface exists but Legion Control does not use it for
    /// writes yet (not validated).
    Detected,
    /// No interface found.
    Unsupported,
}

impl CapabilityState {
    /// `true` if some interface exists.
    pub fn is_present(self) -> bool {
        !matches!(self, CapabilityState::Unsupported)
    }
}

/// One feature with its detection evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// Stable identifier (`power_profiles`, `fan_control`...).
    pub id: String,
    /// State.
    pub state: CapabilityState,
    /// Kernel interface that provides it, if any.
    pub interface: Option<String>,
    /// Human explanation.
    pub note: String,
}

impl Capability {
    /// Convenience constructor.
    pub fn new(
        id: &str,
        state: CapabilityState,
        interface: Option<String>,
        note: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            state,
            interface,
            note: note.into(),
        }
    }
}

/// Summary flags: `true` means an interface for the feature was detected on
/// this machine. See [`Capability`] for details and validation state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareCapabilities {
    /// Power profile selection (platform_profile / power-profiles-daemon).
    pub power_profiles: bool,
    /// Fan speed monitoring.
    pub fan_monitoring: bool,
    /// Manual fan control interface.
    pub fan_control: bool,
    /// Fan curve interface.
    pub fan_curve: bool,
    /// Numeric battery charge threshold.
    pub battery_threshold: bool,
    /// Battery conservation mode (fixed charge limit).
    pub battery_conservation: bool,
    /// Graphics mode (MUX) detection.
    pub gpu_mode: bool,
    /// GPU monitoring via NVML.
    pub gpu_monitoring: bool,
    /// CPU power limit interface.
    pub cpu_power_limit: bool,
    /// GPU power limit interface.
    pub gpu_power_limit: bool,
    /// Keyboard RGB control.
    pub rgb_control: bool,
}

impl HardwareCapabilities {
    /// Derives the summary flags from the detailed list.
    pub fn from_list(list: &[Capability]) -> Self {
        let has = |id: &str| list.iter().any(|c| c.id == id && c.state.is_present());
        Self {
            power_profiles: has("power_profiles"),
            fan_monitoring: has("fan_monitoring"),
            fan_control: has("fan_control"),
            fan_curve: has("fan_curve"),
            battery_threshold: has("battery_threshold"),
            battery_conservation: has("battery_conservation"),
            gpu_mode: has("gpu_mode"),
            gpu_monitoring: has("gpu_monitoring"),
            cpu_power_limit: has("cpu_power_limit"),
            gpu_power_limit: has("gpu_power_limit"),
            rgb_control: has("rgb_control"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_from_list() {
        let list = vec![
            Capability::new("power_profiles", CapabilityState::ReadOnly, None, ""),
            Capability::new("fan_control", CapabilityState::Unsupported, None, ""),
            Capability::new("cpu_power_limit", CapabilityState::Detected, None, ""),
        ];
        let c = HardwareCapabilities::from_list(&list);
        assert!(c.power_profiles);
        assert!(!c.fan_control);
        assert!(c.cpu_power_limit);
        assert!(!c.rgb_control);
    }
}
