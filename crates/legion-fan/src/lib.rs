//! Read-only fan monitoring via hwmon (`fanN_input`).
//!
//! Fan monitoring via hwmon (`fanN_input`) and, for validated drivers only,
//! manual control via `fanN_target`.
//!
//! On the Legion 5 15IAX10 the fans are exposed by the upstream
//! `lenovo-wmi-other` driver (hwmon name `lenovo_wmi_other`). Its
//! documentation (`Documentation/wmi/devices/lenovo-wmi-other.rst`) defines
//! `fanX_target` as "target RPM (tunable, 0=auto)", `fanX_min`/`fanX_max`
//! from the firmware fan test data, and notes that RPMs are rounded down to
//! a multiple of `fanX_div`. Only chips listed in [`VALIDATED_CHIPS`] are
//! ever written; the EC is never touched.
//!
//! The driver publishes no labels, so fans are called `Fan N` — we do not
//! guess which fan is the CPU or GPU one.

use legion_core::HwError;
use legion_core::hwmon::{self, HwmonChip};
use legion_core::model::{Fan, FanCurve, FanInfo, FanMode};
use legion_core::sysfs::{self, SysRoot};

/// Message shown when manual control is not available.
pub const FAN_CONTROL_UNSUPPORTED: &str = "Manual fan control is not supported on this hardware.";

/// hwmon chips whose `fanN_target` semantics are documented upstream.
pub const VALIDATED_CHIPS: &[&str] = &["lenovo_wmi_other"];

fn fans_of(chip: &HwmonChip) -> Vec<Fan> {
    chip.channels("fan")
        .into_iter()
        .map(|n| {
            let control_interface_present = [format!("fan{n}_target"), format!("pwm{n}")]
                .iter()
                .any(|a| sysfs::is_writable_mode(&chip.attr_path(a)));
            Fan {
                label: chip.label("fan", n).unwrap_or_else(|| format!("Fan {n}")),
                chip: chip.name.clone(),
                rpm: chip.read_parse(&format!("fan{n}_input")),
                min_rpm: chip.read_parse(&format!("fan{n}_min")),
                max_rpm: chip.read_parse(&format!("fan{n}_max")),
                target_rpm: chip.read_parse(&format!("fan{n}_target")),
                control_interface_present,
                source: chip.source(&format!("fan{n}")),
            }
        })
        .collect()
}

/// A controllable fan channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllableFan {
    /// Chip.
    pub chip: HwmonChip,
    /// Channel index `N`.
    pub channel: u32,
    /// Minimum RPM accepted (non-zero targets).
    pub min_rpm: u32,
    /// Maximum RPM accepted.
    pub max_rpm: u32,
    /// RPM divisor (targets are rounded down to a multiple).
    pub div: u32,
}

/// Fans whose target can be set (validated chips with a writable
/// `fanN_target` and known min/max).
pub fn controllable_fans(root: &SysRoot) -> Vec<ControllableFan> {
    hwmon::chips(root)
        .into_iter()
        .filter(|c| VALIDATED_CHIPS.contains(&c.name.as_str()))
        .flat_map(|chip| {
            chip.channels("fan")
                .into_iter()
                .filter_map(|n| {
                    if !sysfs::is_writable_mode(&chip.attr_path(&format!("fan{n}_target"))) {
                        return None;
                    }
                    let min_rpm = chip.read_parse(&format!("fan{n}_min"))?;
                    let max_rpm = chip.read_parse(&format!("fan{n}_max"))?;
                    (max_rpm > min_rpm).then(|| ControllableFan {
                        min_rpm,
                        max_rpm,
                        div: chip
                            .read_parse(&format!("fan{n}_div"))
                            .filter(|d| *d > 0)
                            .unwrap_or(1),
                        channel: n,
                        chip: chip.clone(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Validates and normalises a target for one fan. `0` = automatic.
pub fn normalise_target(fan: &ControllableFan, rpm: u32) -> legion_core::Result<u32> {
    if rpm == 0 {
        return Ok(0);
    }
    if !(fan.min_rpm..=fan.max_rpm).contains(&rpm) {
        return Err(HwError::InvalidValue(format!(
            "fan target must be 0 (auto) or between {} and {} RPM, got {rpm}",
            fan.min_rpm, fan.max_rpm
        )));
    }
    // round down to the driver's divisor (documented behaviour), but never
    // below the minimum
    Ok((rpm / fan.div * fan.div).max(fan.min_rpm))
}

/// Sets the target of fan `index` (1-based, in [`controllable_fans`] order)
/// or of all fans. `0` returns control to the firmware.
pub fn set_target(root: &SysRoot, index: Option<usize>, rpm: u32) -> legion_core::Result<()> {
    let fans = controllable_fans(root);
    if fans.is_empty() {
        return Err(HwError::NotSupported(FAN_CONTROL_UNSUPPORTED.into()));
    }
    let selected: Vec<&ControllableFan> = match index {
        None => fans.iter().collect(),
        Some(i) => vec![fans.get(i.wrapping_sub(1)).ok_or_else(|| {
            HwError::InvalidValue(format!("fan {i} does not exist (1..={})", fans.len()))
        })?],
    };
    // validate everything before writing anything
    let targets = selected
        .iter()
        .map(|f| normalise_target(f, rpm).map(|t| (*f, t)))
        .collect::<legion_core::Result<Vec<_>>>()?;
    for (f, t) in targets {
        let path = format!(
            "{}/{}/fan{}_target",
            legion_core::hwmon::HWMON_CLASS,
            f.chip.id,
            f.channel
        );
        sysfs::write_attr(root, &path, &t.to_string())?;
    }
    Ok(())
}

/// Current targets of the controllable fans.
pub fn current_targets(root: &SysRoot) -> Vec<Option<u32>> {
    controllable_fans(root)
        .iter()
        .map(|f| f.chip.read_parse(&format!("fan{}_target", f.channel)))
        .collect()
}

/// Validates a fan mode against the controllable fans.
pub fn validate_mode(root: &SysRoot, mode: &FanMode) -> legion_core::Result<()> {
    let fans = controllable_fans(root);
    let check_rpm = |rpm: u32| -> legion_core::Result<()> {
        if fans.is_empty() && rpm != 0 {
            return Err(HwError::NotSupported(FAN_CONTROL_UNSUPPORTED.into()));
        }
        fans.iter()
            .try_for_each(|f| normalise_target(f, rpm).map(|_| ()))
    };
    match mode {
        FanMode::Auto => Ok(()),
        FanMode::Fixed { rpm } => check_rpm(*rpm),
        FanMode::Curve(c) => {
            validate_curve(c)?;
            c.points.iter().try_for_each(|p| check_rpm(p.rpm))
        }
    }
}

/// Structural validation of a curve.
pub fn validate_curve(c: &FanCurve) -> legion_core::Result<()> {
    let err = |m: &str| Err(HwError::InvalidValue(format!("invalid fan curve: {m}")));
    if !(1..=10).contains(&c.points.len()) {
        return err("1 to 10 points required");
    }
    if c.hysteresis_c > 15 {
        return err("hysteresis must be at most 15 °C");
    }
    for w in c.points.windows(2) {
        if w[1].temp_c <= w[0].temp_c {
            return err("temperatures must be strictly increasing");
        }
        if w[1].rpm != 0 && w[1].rpm < w[0].rpm {
            return err("RPM must not decrease as temperature rises");
        }
    }
    if c.points.iter().any(|p| !(20..=100).contains(&p.temp_c)) {
        return err("temperatures must be between 20 and 100 °C");
    }
    Ok(())
}

/// Evaluates a step curve.
///
/// `previous` is the target currently applied; the curve only steps *down*
/// once the temperature falls `hysteresis_c` below the step threshold.
/// Below the first point the result is `0` (automatic).
pub fn evaluate_curve(c: &FanCurve, temp_c: f64, previous: Option<u32>) -> u32 {
    let step_for = |t: f64| {
        c.points
            .iter()
            .rev()
            .find(|p| t >= f64::from(p.temp_c))
            .map(|p| p.rpm)
            .unwrap_or(0)
    };
    let up = step_for(temp_c);
    match previous {
        // stepping down: only once the temperature is `hysteresis_c` below
        // the threshold of the step currently applied
        Some(prev) if up < prev => step_for(temp_c + f64::from(c.hysteresis_c)).min(prev),
        _ => up,
    }
}

/// A conservative example curve for the Legion 5 15IAX10 fan range
/// (1800–4600 RPM): firmware control below 55 °C.
pub fn example_curve() -> FanCurve {
    use legion_core::model::CurvePoint as P;
    FanCurve {
        points: vec![
            P {
                temp_c: 55,
                rpm: 2200,
            },
            P {
                temp_c: 65,
                rpm: 2800,
            },
            P {
                temp_c: 75,
                rpm: 3500,
            },
            P {
                temp_c: 85,
                rpm: 4600,
            },
        ],
        hysteresis_c: 4,
    }
}

/// Reads all fans. `control_available` reflects whether the caller (the
/// daemon) has manual control enabled.
pub fn read_fans(root: &SysRoot) -> FanInfo {
    let fans: Vec<Fan> = hwmon::chips(root).iter().flat_map(fans_of).collect();
    let controllable = !controllable_fans(root).is_empty();
    let note = if controllable {
        "Manual fan control available through legion-control-daemon (experimental, disabled by \
         default in /etc/legion-control/daemon.json). Interface: lenovo-wmi-other fanN_target."
            .to_owned()
    } else if fans.iter().any(|f| f.control_interface_present) {
        format!(
            "{FAN_CONTROL_UNSUPPORTED} A kernel fan target/pwm interface exists but its driver \
             is not documented/validated; Legion Control only reads it."
        )
    } else {
        FAN_CONTROL_UNSUPPORTED.to_owned()
    };
    FanInfo {
        fans,
        fan_control_supported: controllable,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    #[test]
    fn reads_lenovo_wmi_other_fans() {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/hwmon/hwmon3/name", "lenovo_wmi_other")
            .file("/sys/class/hwmon/hwmon3/fan1_input", "3000")
            .file("/sys/class/hwmon/hwmon3/fan1_min", "1800")
            .file("/sys/class/hwmon/hwmon3/fan1_max", "4600")
            .file("/sys/class/hwmon/hwmon3/fan1_target", "0")
            .mode("/sys/class/hwmon/hwmon3/fan1_target", 0o644)
            .file("/sys/class/hwmon/hwmon3/fan2_input", "2900")
            .file("/sys/class/hwmon/hwmon3/fan2_label", "GPU Fan")
            .mode("/sys/class/hwmon/hwmon3/fan2_input", 0o444)
            .file("/sys/class/hwmon/hwmon4/name", "coretemp");
        let info = read_fans(&fs.root());
        assert_eq!(info.fans.len(), 2);
        let f1 = &info.fans[0];
        assert_eq!(f1.label, "Fan 1");
        assert_eq!(f1.rpm, Some(3000));
        assert_eq!(f1.min_rpm, Some(1800));
        assert_eq!(f1.max_rpm, Some(4600));
        assert_eq!(f1.target_rpm, Some(0));
        assert!(f1.control_interface_present);
        assert_eq!(info.fans[1].label, "GPU Fan");
        assert!(!info.fans[1].control_interface_present);
        // fan1 has target + min/max on a validated chip
        assert!(info.fan_control_supported);
    }

    fn two_fans() -> FakeSysfs {
        let fs = FakeSysfs::new();
        for n in 1..=2 {
            fs.file("/sys/class/hwmon/hwmon3/name", "lenovo_wmi_other")
                .file(&format!("/sys/class/hwmon/hwmon3/fan{n}_input"), "3000")
                .file(&format!("/sys/class/hwmon/hwmon3/fan{n}_min"), "1800")
                .file(&format!("/sys/class/hwmon/hwmon3/fan{n}_max"), "4600")
                .file(&format!("/sys/class/hwmon/hwmon3/fan{n}_div"), "100")
                .file(&format!("/sys/class/hwmon/hwmon3/fan{n}_target"), "0")
                .mode(&format!("/sys/class/hwmon/hwmon3/fan{n}_target"), 0o644);
        }
        fs
    }

    #[test]
    fn target_writes_are_validated_and_rounded() {
        let fs = two_fans();
        let root = fs.root();
        set_target(&root, None, 3050).unwrap();
        assert_eq!(current_targets(&root), [Some(3000), Some(3000)]);
        set_target(&root, Some(2), 0).unwrap();
        assert_eq!(current_targets(&root), [Some(3000), Some(0)]);
        assert!(set_target(&root, None, 1000).is_err());
        assert!(set_target(&root, None, 9000).is_err());
        assert!(set_target(&root, Some(3), 3000).is_err());
        assert!(set_target(&root, Some(0), 3000).is_err());
        assert_eq!(current_targets(&root), [Some(3000), Some(0)]);
    }

    #[test]
    fn unvalidated_chip_is_never_written() {
        let fs = FakeSysfs::new();
        fs.file("/sys/class/hwmon/hwmon1/name", "some_ec_driver")
            .file("/sys/class/hwmon/hwmon1/fan1_input", "3000")
            .file("/sys/class/hwmon/hwmon1/fan1_min", "0")
            .file("/sys/class/hwmon/hwmon1/fan1_max", "5000")
            .file("/sys/class/hwmon/hwmon1/fan1_target", "0")
            .mode("/sys/class/hwmon/hwmon1/fan1_target", 0o644);
        assert!(matches!(
            set_target(&fs.root(), None, 3000),
            Err(HwError::NotSupported(_))
        ));
        assert!(!read_fans(&fs.root()).fan_control_supported);
    }

    #[test]
    fn curve_validation() {
        let fs = two_fans();
        let root = fs.root();
        validate_mode(&root, &FanMode::Curve(example_curve())).unwrap();
        validate_mode(&root, &FanMode::Fixed { rpm: 2000 }).unwrap();
        assert!(validate_mode(&root, &FanMode::Fixed { rpm: 100 }).is_err());
        let mut bad = example_curve();
        bad.points.swap(0, 1);
        assert!(validate_curve(&bad).is_err());
        bad = example_curve();
        bad.points[3].rpm = 2000;
        assert!(validate_curve(&bad).is_err());
        bad = example_curve();
        bad.points.clear();
        assert!(validate_curve(&bad).is_err());
        // no controllable fans → only Auto is valid
        let empty = FakeSysfs::new();
        assert!(validate_mode(&empty.root(), &FanMode::Auto).is_ok());
        assert!(validate_mode(&empty.root(), &FanMode::Fixed { rpm: 3000 }).is_err());
    }

    #[test]
    fn curve_evaluation_with_hysteresis() {
        let c = example_curve(); // 55→2200, 65→2800, 75→3500, 85→4600, hyst 4
        assert_eq!(evaluate_curve(&c, 40.0, None), 0);
        assert_eq!(evaluate_curve(&c, 56.0, None), 2200);
        assert_eq!(evaluate_curve(&c, 76.0, Some(2200)), 3500);
        // falls just below 75: stays at 3500 until < 71
        assert_eq!(evaluate_curve(&c, 73.0, Some(3500)), 3500);
        assert_eq!(evaluate_curve(&c, 70.0, Some(3500)), 2800);
        // back to auto only below 51
        assert_eq!(evaluate_curve(&c, 53.0, Some(2200)), 2200);
        assert_eq!(evaluate_curve(&c, 50.0, Some(2200)), 0);
        assert_eq!(evaluate_curve(&c, 90.0, Some(0)), 4600);
    }

    #[test]
    fn no_fans() {
        let fs = FakeSysfs::new();
        let info = read_fans(&fs.root());
        assert!(info.fans.is_empty());
        assert_eq!(info.note, FAN_CONTROL_UNSUPPORTED);
    }
}
