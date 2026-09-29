//! Custom profile definition and validation (Phase 8).
//!
//! User profiles live in `~/.config/legion-control/profiles/<name>.json`
//! (see [`crate::store`]). They only *describe* desired settings; applying
//! them re-validates every field against the current hardware, and the
//! daemon validates again before writing.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use legion_core::model::{FanMode, FirmwareAttribute};
use legion_core::{HwError, Result};

/// A user-defined profile. Every field is optional: absent fields are left
/// untouched when the profile is applied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProfile {
    /// Free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Raw `platform_profile` value (e.g. `custom`, `low-power`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_profile: Option<String>,
    /// Firmware power/thermal limits (`lenovo-wmi-other` attribute → value).
    /// Only effective with `platform_profile = "custom"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub power_limits: BTreeMap<String, i64>,
    /// cpufreq governor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_governor: Option<String>,
    /// cpufreq energy performance preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_epp: Option<String>,
    /// Turbo boost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_turbo: Option<bool>,
    /// Battery conservation mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_conservation: Option<bool>,
    /// `charge_types` value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_type: Option<String>,
    /// Battery charge end threshold, %.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_threshold: Option<u8>,
    /// Fan mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_mode: Option<FanMode>,
}

/// What the current hardware accepts. Built from detection results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileConstraints {
    /// Available EPP values.
    pub epp: Vec<String>,
    /// Available governors.
    pub governors: Vec<String>,
    /// Turbo control available.
    pub turbo: bool,
    /// Available `platform_profile` choices.
    pub platform_profiles: Vec<String>,
    /// Firmware attributes present.
    pub firmware_attributes: Vec<FirmwareAttribute>,
    /// Available `charge_types`.
    pub charge_types: Vec<String>,
    /// Conservation mode available.
    pub conservation: bool,
    /// Numeric battery threshold supported.
    pub battery_threshold: bool,
    /// Manual fan control supported.
    pub fan_control: bool,
}

/// Profile names are restricted to a safe character set so they can never
/// be used for path traversal.
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(HwError::InvalidValue(format!(
            "profile name {name:?} must be 1-64 characters of [A-Za-z0-9_-]"
        )))
    }
}

/// Directory where user profiles are stored.
pub fn profiles_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("legion-control").join("profiles"))
}

fn one_of(field: &str, v: &Option<String>, allowed: &[String]) -> Result<()> {
    match v {
        Some(v) if !allowed.iter().any(|a| a == v) => Err(HwError::InvalidValue(format!(
            "{field} {v:?} is not supported here (allowed: {})",
            if allowed.is_empty() {
                "none".into()
            } else {
                allowed.join(", ")
            }
        ))),
        _ => Ok(()),
    }
}

impl CustomProfile {
    /// Parses a profile from JSON (unknown fields are rejected).
    pub fn from_json(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| HwError::InvalidValue(format!("invalid profile: {e}")))
    }

    /// Returns `true` if the profile changes nothing.
    pub fn is_empty(&self) -> bool {
        Self {
            description: None,
            ..self.clone()
        } == Self::default()
    }

    /// Validates every field against the hardware constraints.
    pub fn validate(&self, c: &ProfileConstraints) -> Result<()> {
        if self.description.as_ref().is_some_and(|d| d.len() > 200) {
            return Err(HwError::InvalidValue("description too long".into()));
        }
        one_of("cpu_epp", &self.cpu_epp, &c.epp)?;
        one_of("cpu_governor", &self.cpu_governor, &c.governors)?;
        one_of(
            "platform_profile",
            &self.platform_profile,
            &c.platform_profiles,
        )?;
        one_of("charge_type", &self.charge_type, &c.charge_types)?;
        if self.cpu_turbo.is_some() && !c.turbo {
            return Err(HwError::NotSupported("turbo control".into()));
        }
        if self.battery_conservation.is_some() && !c.conservation {
            return Err(HwError::NotSupported("battery conservation mode".into()));
        }
        if let Some(t) = self.battery_threshold {
            if !c.battery_threshold {
                return Err(HwError::NotSupported(
                    "Battery charge threshold is not supported on this hardware.".into(),
                ));
            }
            if !(1..=100).contains(&t) {
                return Err(HwError::InvalidValue(format!(
                    "battery_threshold must be 1-100, got {t}"
                )));
            }
        }
        if !self.power_limits.is_empty() && self.platform_profile.as_deref() != Some("custom") {
            return Err(HwError::InvalidValue(
                "power_limits require \"platform_profile\": \"custom\" (the firmware ignores \
                 them in any other profile)"
                    .into(),
            ));
        }
        let current = |n: &str| {
            c.firmware_attributes
                .iter()
                .find(|a| a.name == n)
                .and_then(|a| a.current())
        };
        let pl1 = self
            .power_limits
            .get("ppt_pl1_spl")
            .copied()
            .or_else(|| current("ppt_pl1_spl"));
        let pl2 = self
            .power_limits
            .get("ppt_pl2_sppt")
            .copied()
            .or_else(|| current("ppt_pl2_sppt"));
        if let (Some(pl1), Some(pl2)) = (pl1, pl2) {
            if pl1 > pl2 {
                return Err(HwError::InvalidValue(format!(
                    "the long-term limit PL1 ({pl1} W) cannot exceed the short-term limit PL2 ({pl2} W)"
                )));
            }
        }
        for (name, value) in &self.power_limits {
            let attr = c
                .firmware_attributes
                .iter()
                .find(|a| &a.name == name)
                .ok_or_else(|| HwError::NotSupported(format!("firmware attribute {name}")))?;
            legion_acpi::validate_firmware_value(attr, *value)?;
        }
        match &self.fan_mode {
            None | Some(FanMode::Auto) => {}
            Some(m) => {
                if !c.fan_control {
                    return Err(HwError::NotSupported(
                        "Manual fan control is not supported on this hardware.".into(),
                    ));
                }
                if let FanMode::Curve(curve) = m {
                    legion_fan::validate_curve(curve)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::model::{CurvePoint, FanCurve};

    fn constraints() -> ProfileConstraints {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect();
        ProfileConstraints {
            epp: s(&[
                "default",
                "performance",
                "balance_performance",
                "balance_power",
                "power",
            ]),
            governors: s(&["performance", "powersave"]),
            turbo: true,
            platform_profiles: s(&[
                "low-power",
                "balanced",
                "performance",
                "max-power",
                "custom",
            ]),
            firmware_attributes: vec![FirmwareAttribute {
                name: "ppt_pl1_spl".into(),
                display_name: None,
                kind: Some("integer".into()),
                current_value: Some("105".into()),
                default_value: None,
                min_value: Some("50".into()),
                max_value: Some("105".into()),
                scalar_increment: Some("1".into()),
                possible_values: None,
                writable_by_root: true,
            }],
            charge_types: s(&["Fast", "Standard", "Long_Life"]),
            conservation: true,
            battery_threshold: false,
            fan_control: false,
        }
    }

    #[test]
    fn valid_profile() {
        let p = CustomProfile::from_json(
            r#"{"cpu_epp":"balance_power","cpu_governor":"powersave","platform_profile":"low-power",
                "battery_conservation":true,"fan_mode":{"mode":"auto"}}"#,
        )
        .unwrap();
        p.validate(&constraints()).unwrap();
    }

    #[test]
    fn power_limits_need_custom_profile() {
        let c = constraints();
        let mut p = CustomProfile::default();
        p.power_limits.insert("ppt_pl1_spl".into(), 80);
        assert!(p.validate(&c).is_err());
        p.platform_profile = Some("custom".into());
        p.validate(&c).unwrap();
        p.power_limits.insert("ppt_pl1_spl".into(), 200);
        assert!(p.validate(&c).is_err());
        p.power_limits.clear();
        p.power_limits.insert("ec_register".into(), 1);
        assert!(p.validate(&c).is_err());
    }

    #[test]
    fn pl1_cannot_exceed_pl2() {
        let mut c = constraints();
        c.firmware_attributes.push(FirmwareAttribute {
            name: "ppt_pl2_sppt".into(),
            display_name: None,
            kind: Some("integer".into()),
            current_value: Some("80".into()),
            default_value: None,
            min_value: Some("60".into()),
            max_value: Some("168".into()),
            scalar_increment: Some("1".into()),
            possible_values: None,
            writable_by_root: true,
        });
        let mut p = CustomProfile {
            platform_profile: Some("custom".into()),
            ..Default::default()
        };
        p.power_limits.insert("ppt_pl1_spl".into(), 100);
        assert!(p.validate(&c).is_err()); // current PL2 is 80
        p.power_limits.insert("ppt_pl2_sppt".into(), 120);
        p.validate(&c).unwrap();
    }

    #[test]
    fn rejects_unknown_values() {
        let c = constraints();
        let p = CustomProfile {
            platform_profile: Some("quiet".into()),
            ..Default::default()
        };
        assert!(p.validate(&c).is_err());
        let p = CustomProfile {
            cpu_epp: Some("turbo".into()),
            ..Default::default()
        };
        assert!(p.validate(&c).is_err());
        let p = CustomProfile {
            battery_threshold: Some(80),
            ..Default::default()
        };
        assert!(matches!(p.validate(&c), Err(HwError::NotSupported(_))));
        let p = CustomProfile {
            fan_mode: Some(FanMode::Fixed { rpm: 3000 }),
            ..Default::default()
        };
        assert!(matches!(p.validate(&c), Err(HwError::NotSupported(_))));
    }

    #[test]
    fn fan_curve_is_validated() {
        let c = ProfileConstraints {
            fan_control: true,
            ..constraints()
        };
        let bad = CustomProfile {
            fan_mode: Some(FanMode::Curve(FanCurve {
                points: vec![
                    CurvePoint {
                        temp_c: 70,
                        rpm: 3000,
                    },
                    CurvePoint {
                        temp_c: 60,
                        rpm: 2000,
                    },
                ],
                hysteresis_c: 3,
            })),
            ..Default::default()
        };
        assert!(bad.validate(&c).is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_bad_names() {
        assert!(CustomProfile::from_json(r#"{"ec_register":"0xC0"}"#).is_err());
        assert!(validate_name("gaming_1").is_ok());
        assert!(validate_name("../../etc/passwd").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("a/b").is_err());
    }

    #[test]
    fn json_roundtrip() {
        let mut p = CustomProfile {
            cpu_epp: Some("performance".into()),
            fan_mode: Some(FanMode::Auto),
            platform_profile: Some("custom".into()),
            ..Default::default()
        };
        p.power_limits.insert("ppt_pl1_spl".into(), 90);
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(CustomProfile::from_json(&s).unwrap(), p);
        assert!(CustomProfile::default().is_empty());
        assert!(!p.is_empty());
    }
}
