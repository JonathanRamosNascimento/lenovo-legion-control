//! Applying and capturing [`CustomProfile`]s (Phase 8).

use legion_core::model::{FanMode, TurboState};
use legion_core::{HardwareController, HwError, Result};
use legion_profiles::{CustomProfile, ProfileConstraints};

use crate::controller::LinuxController;

/// Builds the constraints of the current machine.
pub fn constraints(ctrl: &LinuxController) -> ProfileConstraints {
    let cpu = legion_cpu::read_cpu_info(ctrl.root(), std::time::Duration::ZERO);
    let battery = ctrl.get_battery_info().unwrap_or_default();
    let bat = battery.batteries.first();
    let pp = legion_profiles::read_platform_profile(ctrl.root());
    let cfg = &ctrl.options().config;
    ProfileConstraints {
        epp: cpu.available_epp,
        governors: cpu.available_governors,
        turbo: cpu.turbo != TurboState::Unknown,
        platform_profiles: pp.map(|p| p.choices).unwrap_or_default(),
        firmware_attributes: if cfg.enable_power_limits {
            ctrl.get_power_limits()
                .map(|p| p.attributes)
                .unwrap_or_default()
        } else {
            vec![]
        },
        charge_types: bat
            .and_then(|b| b.charge_types.as_ref())
            .map(|c| c.available.clone())
            .unwrap_or_default(),
        conservation: battery.conservation_mode.is_some()
            || bat
                .and_then(|b| b.charge_types.as_ref())
                .is_some_and(|c| c.available.iter().any(|a| a == "Long_Life")),
        battery_threshold: bat.is_some_and(|b| b.charge_end_threshold.is_some()),
        fan_control: cfg.enable_fan_control
            && !legion_fan::controllable_fans(ctrl.root()).is_empty(),
    }
}

/// Captures the current machine state as a profile. `fan_mode` is supplied
/// by the caller (only the daemon knows it).
pub fn capture(ctrl: &LinuxController, fan_mode: Option<FanMode>) -> CustomProfile {
    let cpu = legion_cpu::read_cpu_info(ctrl.root(), std::time::Duration::ZERO);
    let battery = ctrl.get_battery_info().unwrap_or_default();
    let bat = battery.batteries.first();
    let raw = legion_profiles::read_platform_profile(ctrl.root()).and_then(|p| p.current_raw);
    let mut p = CustomProfile {
        platform_profile: raw.clone(),
        cpu_governor: (cpu.governors.len() == 1).then(|| cpu.governors[0].clone()),
        cpu_epp: (cpu.epp.len() == 1).then(|| cpu.epp[0].clone()),
        cpu_turbo: match cpu.turbo {
            TurboState::Enabled => Some(true),
            TurboState::Disabled => Some(false),
            TurboState::Unknown => None,
        },
        battery_conservation: battery.conservation_mode,
        charge_type: if battery.conservation_mode.is_some() {
            // conservation_mode already describes Long_Life
            None
        } else {
            bat.and_then(|b| b.charge_types.as_ref()?.active.clone())
        },
        battery_threshold: bat.and_then(|b| b.charge_end_threshold),
        fan_mode,
        ..Default::default()
    };
    if raw.as_deref() == Some("custom") && ctrl.options().config.enable_power_limits {
        if let Ok(pl) = ctrl.get_power_limits() {
            for a in pl.attributes {
                if let (Some(_), Some(v)) = (a.range(), a.current()) {
                    p.power_limits.insert(a.name, v);
                }
            }
        }
    }
    p
}

/// Applies a profile. The order matters: the platform profile is switched
/// first (power limits only work in `custom`), fan mode last.
///
/// `fan_handler` receives the fan mode (the daemon runs curves itself;
/// without a daemon only `Auto`/`Fixed` can be applied).
/// Returns a human-readable list of the changes made.
pub fn apply(
    ctrl: &LinuxController,
    profile: &CustomProfile,
    mut fan_handler: impl FnMut(&FanMode) -> Result<()>,
) -> Result<Vec<String>> {
    profile.validate(&constraints(ctrl))?;
    let mut done = Vec::new();
    if let Some(raw) = &profile.platform_profile {
        let logical = legion_core::PowerProfile::from_platform_profile(raw);
        if logical == legion_core::PowerProfile::Unknown {
            legion_profiles::set_platform_profile(ctrl.root(), raw)?;
        } else {
            ctrl.set_power_profile(logical)?;
        }
        done.push(format!("platform profile → {raw}"));
    }
    for (name, value) in limit_order(ctrl, &profile.power_limits) {
        ctrl.set_firmware_attribute(name, value)?;
        done.push(format!("{name} → {value}"));
    }
    if let Some(g) = &profile.cpu_governor {
        ctrl.set_cpu_governor(g)?;
        done.push(format!("governor → {g}"));
    }
    if let Some(e) = &profile.cpu_epp {
        ctrl.set_cpu_epp(e)?;
        done.push(format!("EPP → {e}"));
    }
    if let Some(t) = profile.cpu_turbo {
        ctrl.set_turbo(t)?;
        done.push(format!("turbo → {}", if t { "on" } else { "off" }));
    }
    if let Some(c) = profile.battery_conservation {
        ctrl.set_battery_conservation(c)?;
        done.push(format!(
            "conservation mode → {}",
            if c { "on" } else { "off" }
        ));
    }
    if let Some(t) = &profile.charge_type {
        ctrl.set_charge_type(t)?;
        done.push(format!("charge type → {t}"));
    }
    if let Some(t) = profile.battery_threshold {
        ctrl.set_charge_threshold(t)?;
        done.push(format!("charge threshold → {t}%"));
    }
    if let Some(m) = &profile.fan_mode {
        fan_handler(m)?;
        done.push(format!("fan mode → {}", fan_mode_name(m)));
    }
    Ok(done)
}

/// Orders power-limit writes so PL1 never exceeds PL2 in between: when PL1
/// is raised above the current PL2, PL2 is written first; otherwise PL1 is
/// written first (so a lowered PL2 is never below the old PL1).
pub fn limit_order<'a>(
    ctrl: &LinuxController,
    limits: &'a std::collections::BTreeMap<String, i64>,
) -> Vec<(&'a str, i64)> {
    let mut v: Vec<(&str, i64)> = limits.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let current = |name: &str| {
        ctrl.get_power_limits()
            .ok()
            .and_then(|p| p.attributes.into_iter().find(|a| a.name == name))
            .and_then(|a| a.current())
    };
    // PL1 goes first unless it would exceed the PL2 currently in effect
    let pl1_first = match (limits.get("ppt_pl1_spl"), current("ppt_pl2_sppt")) {
        (Some(new_pl1), Some(cur_pl2)) => *new_pl1 <= cur_pl2,
        _ => true,
    };
    v.sort_by_key(|(name, _)| match *name {
        "ppt_pl1_spl" if pl1_first => 0,
        "ppt_pl2_sppt" if pl1_first => 1,
        "ppt_pl2_sppt" => 0,
        "ppt_pl1_spl" => 1,
        _ => 2,
    });
    v
}

/// Short name of a fan mode.
pub fn fan_mode_name(m: &FanMode) -> String {
    match m {
        FanMode::Auto => "auto".into(),
        FanMode::Fixed { rpm } => format!("fixed {rpm} RPM"),
        FanMode::Curve(c) => format!("curve ({} points)", c.points.len()),
    }
}

/// Fan handler for callers without a daemon: supports `Auto` and `Fixed`.
pub fn direct_fan_handler(ctrl: &LinuxController) -> impl FnMut(&FanMode) -> Result<()> + '_ {
    move |m| match m {
        FanMode::Auto => ctrl.set_fan_target(None, 0),
        FanMode::Fixed { rpm } => ctrl.set_fan_target(None, *rpm),
        FanMode::Curve(_) => Err(HwError::NotSupported(
            "fan curves are evaluated continuously by legion-control-daemon".into(),
        )),
    }
}
