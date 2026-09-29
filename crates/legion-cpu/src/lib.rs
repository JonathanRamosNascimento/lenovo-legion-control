//! Read-only CPU monitoring.
//!
//! Sources (all standard kernel ABIs):
//!
//! * `/proc/cpuinfo`, `/proc/stat` — model, utilisation.
//! * `/sys/devices/system/cpu/cpu*/cpufreq`, `/sys/devices/system/cpu/cpufreq/policy*`
//!   — frequency, governor, EPP (`Documentation/admin-guide/pm/cpufreq.rst`,
//!   `intel_pstate.rst`).
//! * `/sys/devices/system/cpu/intel_pstate/no_turbo`, `cpufreq/boost` — turbo.
//! * `/sys/devices/cpu_core/cpus`, `/sys/devices/cpu_atom/cpus` — hybrid topology.
//! * `/sys/class/powercap/intel-rapl:*` — package energy (usually root-only
//!   since CVE-2020-8694) and power limits (read-only here).
//! * hwmon `coretemp` / `k10temp`, thermal zone `x86_pkg_temp` — temperature.
//!
//! Writes (Phase 7) are limited to the documented cpufreq attributes
//! `scaling_governor`, `energy_performance_preference` and
//! `intel_pstate/no_turbo` / `cpufreq/boost`, validated against the values
//! the kernel advertises. RAPL power limits are never written.

use std::collections::BTreeSet;
use std::thread;
use std::time::{Duration, Instant};

use legion_core::hwmon;
use legion_core::model::{CoreTopology, CpuInfo, Metric, RaplConstraint, TurboState};
use legion_core::sysfs::{self, Access, SysRoot};

const CPU_DIR: &str = "/sys/devices/system/cpu";
const POWERCAP: &str = "/sys/class/powercap";

/// Aggregate jiffies from the first `cpu` line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    /// idle + iowait.
    pub idle: u64,
    /// Sum of all fields except guest time (already included in user/nice).
    pub total: u64,
}

/// Parses the aggregate `cpu` line of `/proc/stat`.
pub fn parse_proc_stat(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    if v.len() < 4 {
        return None;
    }
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    // Fields 8 and 9 (guest, guest_nice) are already accounted in user/nice.
    let total = v.iter().take(8).sum();
    Some(CpuTimes { idle, total })
}

/// Utilisation between two samples, in percent.
pub fn usage_between(a: CpuTimes, b: CpuTimes) -> Option<f64> {
    let total = b.total.checked_sub(a.total)?;
    let idle = b.idle.checked_sub(a.idle)?;
    if total == 0 {
        return None;
    }
    Some(((total - idle.min(total)) as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// Fields extracted from `/proc/cpuinfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CpuIdent {
    /// `model name`.
    pub model: Option<String>,
    /// `vendor_id`.
    pub vendor: Option<String>,
    /// Number of `processor` entries.
    pub logical_cpus: usize,
}

/// Parses `/proc/cpuinfo`.
pub fn parse_cpuinfo(text: &str) -> CpuIdent {
    let mut id = CpuIdent::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "processor" => id.logical_cpus += 1,
            "model name" if id.model.is_none() => id.model = Some(clean_model(v)),
            "vendor_id" if id.vendor.is_none() => id.vendor = Some(v.to_owned()),
            _ => {}
        }
    }
    id
}

/// Removes `(R)`, `(TM)` and duplicate spaces from a CPU model string.
pub fn clean_model(s: &str) -> String {
    s.replace("(R)", "")
        .replace("(TM)", "")
        .replace("(tm)", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads the CPU model string (used by system detection).
pub fn cpu_model(root: &SysRoot) -> Option<String> {
    root.read_opt("/proc/cpuinfo")
        .and_then(|t| parse_cpuinfo(&t).model)
}

/// A point-in-time sample used to compute rates.
#[derive(Debug, Clone)]
pub struct CpuSample {
    times: Option<CpuTimes>,
    energy: Metric<u64>,
    at: Instant,
}

/// RAPL package zone location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaplZone {
    /// Kernel path of the zone directory.
    pub dir: String,
}

/// Finds the top-level RAPL `package-0` zone.
pub fn find_rapl_package(root: &SysRoot) -> Option<RaplZone> {
    root.list(POWERCAP)
        .into_iter()
        // top-level zones have exactly one ':' (intel-rapl:0, not intel-rapl:0:1)
        .filter(|z| z.starts_with("intel-rapl:") && z.matches(':').count() == 1)
        .map(|z| format!("{POWERCAP}/{z}"))
        .find(|dir| root.read_opt(format!("{dir}/name")).as_deref() == Some("package-0"))
        .map(|dir| RaplZone { dir })
}

fn read_energy(root: &SysRoot, zone: Option<&RaplZone>) -> Metric<u64> {
    let Some(zone) = zone else {
        return Metric::NotSupported;
    };
    let path = format!("{}/energy_uj", zone.dir);
    match root.access(&path) {
        Access::Readable => root
            .read_parse_opt(&path)
            .map(Metric::Ok)
            .unwrap_or_else(|| Metric::Unavailable("unparsable energy_uj".into())),
        Access::PermissionDenied => Metric::PermissionRequired,
        Access::Missing => Metric::NotSupported,
        Access::Error => Metric::Unavailable("read error".into()),
    }
}

/// Takes a sample of `/proc/stat` and the RAPL energy counter.
pub fn sample(root: &SysRoot) -> CpuSample {
    let zone = find_rapl_package(root);
    CpuSample {
        times: root
            .read_opt("/proc/stat")
            .and_then(|s| parse_proc_stat(&s)),
        energy: read_energy(root, zone.as_ref()),
        at: Instant::now(),
    }
}

/// Package power between two energy readings (µJ) over `secs`,
/// handling counter wrap-around.
pub fn power_between(e0: u64, e1: u64, max_range: Option<u64>, secs: f64) -> Option<f64> {
    if secs <= 0.0 {
        return None;
    }
    let delta = if e1 >= e0 {
        e1 - e0
    } else {
        max_range? - e0 + e1
    };
    Some(delta as f64 / 1e6 / secs)
}

fn average_freq_mhz(root: &SysRoot) -> Option<u32> {
    let khz: Vec<u64> = root
        .list(CPU_DIR)
        .into_iter()
        .filter(|n| {
            n.strip_prefix("cpu")
                .is_some_and(|r| r.parse::<u32>().is_ok())
        })
        .filter_map(|n| {
            root.read_parse_opt::<u64>(format!("{CPU_DIR}/{n}/cpufreq/scaling_cur_freq"))
        })
        .collect();
    if khz.is_empty() {
        return None;
    }
    Some((khz.iter().sum::<u64>() / khz.len() as u64 / 1000) as u32)
}

fn policies(root: &SysRoot) -> Vec<String> {
    root.list(format!("{CPU_DIR}/cpufreq"))
        .into_iter()
        .filter(|p| p.starts_with("policy"))
        .map(|p| format!("{CPU_DIR}/cpufreq/{p}"))
        .collect()
}

fn collect_distinct(root: &SysRoot, dirs: &[String], attr: &str) -> Vec<String> {
    dirs.iter()
        .filter_map(|d| root.read_opt(format!("{d}/{attr}")))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn first_words(root: &SysRoot, dirs: &[String], attr: &str) -> Vec<String> {
    dirs.iter()
        .find_map(|d| root.read_opt(format!("{d}/{attr}")))
        .map(|s| s.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Turbo state from `intel_pstate/no_turbo` or `cpufreq/boost`.
pub fn turbo_state(root: &SysRoot) -> TurboState {
    if let Some(v) = root.read_opt(format!("{CPU_DIR}/intel_pstate/no_turbo")) {
        return match v.as_str() {
            "0" => TurboState::Enabled,
            "1" => TurboState::Disabled,
            _ => TurboState::Unknown,
        };
    }
    match root.read_opt(format!("{CPU_DIR}/cpufreq/boost")).as_deref() {
        Some("1") => TurboState::Enabled,
        Some("0") => TurboState::Disabled,
        _ => TurboState::Unknown,
    }
}

fn topology(root: &SysRoot) -> Option<CoreTopology> {
    let p = root
        .read_opt("/sys/devices/cpu_core/cpus")
        .and_then(|s| sysfs::cpu_list_len(&s))?;
    let e = root
        .read_opt("/sys/devices/cpu_atom/cpus")
        .and_then(|s| sysfs::cpu_list_len(&s))?;
    Some(CoreTopology {
        performance: p,
        efficiency: e,
    })
}

/// CPU package temperature: coretemp `Package id 0`, k10temp `Tctl`, or the
/// `x86_pkg_temp` thermal zone.
pub fn package_temperature(root: &SysRoot) -> Option<f64> {
    for chip in hwmon::chips(root) {
        let wanted = match chip.name.as_str() {
            "coretemp" => "Package id 0",
            "k10temp" | "zenpower" => "Tctl",
            _ => continue,
        };
        for n in chip.channels("temp") {
            if chip.label("temp", n).as_deref() == Some(wanted) {
                return chip.temp_c(n);
            }
        }
    }
    root.list("/sys/class/thermal")
        .into_iter()
        .filter(|z| z.starts_with("thermal_zone"))
        .find(|z| {
            root.read_opt(format!("/sys/class/thermal/{z}/type"))
                .as_deref()
                == Some("x86_pkg_temp")
        })
        .and_then(|z| root.read_parse_opt::<i64>(format!("/sys/class/thermal/{z}/temp")))
        .map(|m| m as f64 / 1000.0)
}

fn rapl_constraints(root: &SysRoot, zone: Option<&RaplZone>) -> Vec<RaplConstraint> {
    let Some(zone) = zone else { return vec![] };
    (0..8)
        .map_while(|i| {
            let name = root.read_opt(format!("{}/constraint_{i}_name", zone.dir))?;
            Some(RaplConstraint {
                name,
                limit_w: root
                    .read_parse_opt::<u64>(format!("{}/constraint_{i}_power_limit_uw", zone.dir))
                    .map(|v| v as f64 / 1e6),
                time_window_s: root
                    .read_parse_opt::<u64>(format!("{}/constraint_{i}_time_window_us", zone.dir))
                    .map(|v| v as f64 / 1e6),
            })
        })
        .collect()
}

/// Collects CPU information. Utilisation and package power are computed over
/// `interval` (the call sleeps for that long). Pass `Duration::ZERO` to skip
/// rate metrics.
pub fn read_cpu_info(root: &SysRoot, interval: Duration) -> CpuInfo {
    let first = (!interval.is_zero()).then(|| sample(root));
    if !interval.is_zero() {
        thread::sleep(interval);
    }
    let second = sample(root);
    info_from_samples(root, first.as_ref(), &second)
}

/// Builds [`CpuInfo`] from two samples (the first is optional).
pub fn info_from_samples(root: &SysRoot, first: Option<&CpuSample>, second: &CpuSample) -> CpuInfo {
    let ident = root
        .read_opt("/proc/cpuinfo")
        .map(|t| parse_cpuinfo(&t))
        .unwrap_or_default();
    let zone = find_rapl_package(root);
    let pol = policies(root);

    let usage_percent = match first {
        None => Metric::Unavailable("not sampled".into()),
        Some(a) => match (a.times, second.times) {
            (Some(x), Some(y)) => usage_between(x, y)
                .map(Metric::Ok)
                .unwrap_or_else(|| Metric::Unavailable("interval too short".into())),
            _ => Metric::NotSupported,
        },
    };

    let package_power_w = match (first.map(|a| &a.energy), &second.energy) {
        (_, Metric::PermissionRequired) => Metric::PermissionRequired,
        (_, Metric::NotSupported) => Metric::NotSupported,
        (Some(Metric::Ok(e0)), Metric::Ok(e1)) => {
            let max = zone
                .as_ref()
                .and_then(|z| root.read_parse_opt(format!("{}/max_energy_range_uj", z.dir)));
            let secs = second.at.duration_since(first.unwrap().at).as_secs_f64();
            power_between(*e0, *e1, max, secs)
                .map(Metric::Ok)
                .unwrap_or_else(|| Metric::Unavailable("invalid sample".into()))
        }
        _ => Metric::Unavailable("not sampled".into()),
    };

    let khz_mhz = |attr: &str, pick_max: bool| {
        let vals = pol
            .iter()
            .filter_map(|d| root.read_parse_opt::<u64>(format!("{d}/{attr}")));
        let v = if pick_max { vals.max() } else { vals.min() };
        v.map(|k| (k / 1000) as u32)
    };

    CpuInfo {
        model: ident.model,
        vendor: ident.vendor,
        logical_cpus: ident.logical_cpus,
        topology: topology(root),
        usage_percent,
        frequency_mhz: average_freq_mhz(root),
        max_frequency_mhz: khz_mhz("cpuinfo_max_freq", true),
        min_frequency_mhz: khz_mhz("cpuinfo_min_freq", false),
        temperature_c: package_temperature(root),
        scaling_driver: pol
            .iter()
            .find_map(|d| root.read_opt(format!("{d}/scaling_driver"))),
        pstate_status: root.read_opt(format!("{CPU_DIR}/intel_pstate/status")),
        governors: collect_distinct(root, &pol, "scaling_governor"),
        available_governors: first_words(root, &pol, "scaling_available_governors"),
        epp: collect_distinct(root, &pol, "energy_performance_preference"),
        available_epp: first_words(root, &pol, "energy_performance_available_preferences"),
        turbo: turbo_state(root),
        package_power_w,
        rapl_constraints: rapl_constraints(root, zone.as_ref()),
    }
}

fn set_on_all_policies(
    root: &SysRoot,
    attr: &str,
    avail_attr: &str,
    value: &str,
) -> legion_core::Result<()> {
    use legion_core::HwError;
    let pol = policies(root);
    if pol.is_empty() {
        return Err(HwError::NotSupported("cpufreq policies".into()));
    }
    let available = first_words(root, &pol, avail_attr);
    if !available.iter().any(|a| a == value) {
        return Err(HwError::InvalidValue(format!(
            "{value:?} is not one of: {}",
            if available.is_empty() {
                "(none)".into()
            } else {
                available.join(", ")
            }
        )));
    }
    for p in &pol {
        let path = format!("{p}/{attr}");
        if root.exists(&path) {
            sysfs::write_attr(root, &path, value).map_err(|e| match e {
                HwError::Backend { .. } if attr == "energy_performance_preference" => {
                    HwError::InvalidValue(
                        "EPP cannot be changed while the 'performance' governor is active".into(),
                    )
                }
                other => other,
            })?;
        }
    }
    Ok(())
}

/// Sets the energy performance preference on every policy.
pub fn set_epp(root: &SysRoot, epp: &str) -> legion_core::Result<()> {
    set_on_all_policies(
        root,
        "energy_performance_preference",
        "energy_performance_available_preferences",
        epp,
    )
}

/// Sets the scaling governor on every policy.
pub fn set_governor(root: &SysRoot, governor: &str) -> legion_core::Result<()> {
    set_on_all_policies(
        root,
        "scaling_governor",
        "scaling_available_governors",
        governor,
    )
}

/// Enables or disables turbo boost.
pub fn set_turbo(root: &SysRoot, enabled: bool) -> legion_core::Result<()> {
    let no_turbo = format!("{CPU_DIR}/intel_pstate/no_turbo");
    if root.exists(&no_turbo) {
        sysfs::write_attr(root, &no_turbo, if enabled { "0" } else { "1" })?;
        return Ok(());
    }
    let boost = format!("{CPU_DIR}/cpufreq/boost");
    if root.exists(&boost) {
        sysfs::write_attr(root, &boost, if enabled { "1" } else { "0" })?;
        return Ok(());
    }
    Err(legion_core::HwError::NotSupported("turbo control".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::testing::FakeSysfs;

    const STAT_A: &str = "cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 1 2 3 4\n";
    const STAT_B: &str = "cpu  150 0 150 900 0 0 0 0 0 0\n";

    #[test]
    fn proc_stat_parsing() {
        let a = parse_proc_stat(STAT_A).unwrap();
        assert_eq!(
            a,
            CpuTimes {
                idle: 800,
                total: 1000
            }
        );
        let b = parse_proc_stat(STAT_B).unwrap();
        assert_eq!(usage_between(a, b), Some(50.0));
        assert_eq!(usage_between(a, a), None);
        assert!(parse_proc_stat("garbage").is_none());
    }

    #[test]
    fn cpuinfo_parsing() {
        let t = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Core(TM) Ultra 7 255HX\n\n\
                 processor\t: 1\nmodel name\t: Intel(R) Core(TM) Ultra 7 255HX\n";
        let id = parse_cpuinfo(t);
        assert_eq!(id.model.as_deref(), Some("Intel Core Ultra 7 255HX"));
        assert_eq!(id.vendor.as_deref(), Some("GenuineIntel"));
        assert_eq!(id.logical_cpus, 2);
    }

    #[test]
    fn rapl_power_math() {
        assert_eq!(power_between(1_000_000, 11_000_000, None, 1.0), Some(10.0));
        // wrap-around
        assert_eq!(power_between(90, 10, Some(100), 1.0), Some(20.0 / 1e6));
        assert_eq!(power_between(90, 10, None, 1.0), None);
        assert_eq!(power_between(0, 10, None, 0.0), None);
    }

    fn fake() -> FakeSysfs {
        let fs = FakeSysfs::new();
        fs.file(
            "/proc/cpuinfo",
            "processor : 0\nvendor_id : GenuineIntel\nmodel name : Intel(R) Core(TM) Ultra 7 255HX\nprocessor : 1\n",
        )
        .file("/proc/stat", STAT_A)
        .file("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq", "4000000")
        .file("/sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq", "2000000")
        .file("/sys/devices/system/cpu/cpufreq/policy0/scaling_driver", "intel_pstate")
        .file("/sys/devices/system/cpu/cpufreq/policy0/scaling_governor", "powersave")
        .file("/sys/devices/system/cpu/cpufreq/policy1/scaling_governor", "powersave")
        .file("/sys/devices/system/cpu/cpufreq/policy0/scaling_available_governors", "performance powersave")
        .file("/sys/devices/system/cpu/cpufreq/policy0/energy_performance_preference", "balance_performance")
        .file("/sys/devices/system/cpu/cpufreq/policy1/energy_performance_preference", "performance")
        .file("/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_max_freq", "5300000")
        .file("/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_max_freq", "4500000")
        .file("/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_min_freq", "800000")
        .file("/sys/devices/system/cpu/intel_pstate/no_turbo", "0")
        .file("/sys/devices/system/cpu/intel_pstate/status", "active")
        .file("/sys/devices/cpu_core/cpus", "0-7")
        .file("/sys/devices/cpu_atom/cpus", "8-19")
        .file("/sys/class/hwmon/hwmon4/name", "coretemp")
        .file("/sys/class/hwmon/hwmon4/temp1_input", "64000")
        .file("/sys/class/hwmon/hwmon4/temp1_label", "Package id 0")
        .file("/sys/class/powercap/intel-rapl:0/name", "package-0")
        .file("/sys/class/powercap/intel-rapl:0/energy_uj", "1000")
        .file("/sys/class/powercap/intel-rapl:0/constraint_0_name", "long_term")
        .file("/sys/class/powercap/intel-rapl:0/constraint_0_power_limit_uw", "105000000")
        .file("/sys/class/powercap/intel-rapl:0/constraint_0_time_window_us", "56000000")
        .file("/sys/class/powercap/intel-rapl:0:0/name", "core");
        fs
    }

    #[test]
    fn full_read_from_fake_tree() {
        let fs = fake();
        let root = fs.root();
        let a = sample(&root);
        fs.file("/proc/stat", STAT_B);
        let b = sample(&root);
        let info = info_from_samples(&root, Some(&a), &b);
        assert_eq!(info.model.as_deref(), Some("Intel Core Ultra 7 255HX"));
        assert_eq!(info.logical_cpus, 2);
        assert_eq!(info.usage_percent, Metric::Ok(50.0));
        assert_eq!(info.frequency_mhz, Some(3000));
        assert_eq!(info.max_frequency_mhz, Some(5300));
        assert_eq!(info.min_frequency_mhz, Some(800));
        assert_eq!(info.temperature_c, Some(64.0));
        assert_eq!(info.governors, ["powersave"]);
        assert_eq!(info.epp, ["balance_performance", "performance"]);
        assert_eq!(info.available_governors, ["performance", "powersave"]);
        assert_eq!(info.turbo, TurboState::Enabled);
        assert_eq!(
            info.topology,
            Some(CoreTopology {
                performance: 8,
                efficiency: 12
            })
        );
        assert_eq!(info.scaling_driver.as_deref(), Some("intel_pstate"));
        assert_eq!(info.rapl_constraints.len(), 1);
        assert_eq!(info.rapl_constraints[0].limit_w, Some(105.0));
        assert!(info.package_power_w.is_ok());
    }

    #[test]
    fn rapl_permission_denied_is_reported() {
        // Running tests as root would bypass the permission bits.
        if std::fs::read_to_string("/proc/self/status")
            .unwrap_or_default()
            .lines()
            .any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(2) == Some("0"))
        {
            return;
        }
        let fs = fake();
        fs.mode("/sys/class/powercap/intel-rapl:0/energy_uj", 0o000);
        let root = fs.root();
        let info = info_from_samples(&root, Some(&sample(&root)), &sample(&root));
        assert_eq!(info.package_power_w, Metric::PermissionRequired);
    }

    #[test]
    fn cpufreq_writes() {
        let fs = fake();
        fs.file(
            "/sys/devices/system/cpu/cpufreq/policy0/energy_performance_available_preferences",
            "default performance balance_performance balance_power power",
        );
        let root = fs.root();
        set_epp(&root, "balance_power").unwrap();
        assert_eq!(
            root.read("/sys/devices/system/cpu/cpufreq/policy1/energy_performance_preference")
                .unwrap(),
            "balance_power"
        );
        assert!(set_epp(&root, "turbo").is_err());
        set_governor(&root, "performance").unwrap();
        assert!(set_governor(&root, "ondemand").is_err());
        set_turbo(&root, false).unwrap();
        assert_eq!(turbo_state(&root), TurboState::Disabled);
        assert!(set_turbo(&FakeSysfs::new().root(), true).is_err());
    }

    #[test]
    fn empty_tree_does_not_panic() {
        let fs = FakeSysfs::new();
        let info = read_cpu_info(&fs.root(), Duration::ZERO);
        assert_eq!(info.logical_cpus, 0);
        assert_eq!(info.turbo, TurboState::Unknown);
        assert_eq!(info.package_power_w, Metric::NotSupported);
        assert!(info.temperature_c.is_none());
    }
}
