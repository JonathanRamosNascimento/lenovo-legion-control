//! Hardware worker: a single OS thread owning every hardware interaction.
//!
//! * Serialises all writes (no two changes can race).
//! * Keeps blocking sysfs / NVML / power-profiles-daemon calls out of the
//!   async runtime.
//! * Caches the latest [`Status`] so D-Bus getters are cheap.
//! * Runs the fan-control loop with thermal safety.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use legion_core::capabilities::Capability;
use legion_core::model::{BatteryInfo, DaemonInfo, FanMode, GpuInfo, GpuMode, Status, SystemInfo};
use legion_core::profile::PpdInfo;
use legion_core::{HardwareController, HwError, PowerProfile, Result};
use legion_hardware::{Config, LinuxController, capabilities, custom};
use legion_profiles::CustomProfile;

const GPU_INTERVAL: Duration = Duration::from_secs(2);
const PPD_INTERVAL: Duration = Duration::from_secs(5);
const BATTERY_INTERVAL: Duration = Duration::from_secs(10);
/// Consecutive fan write failures before giving control back to firmware.
const MAX_FAN_ERRORS: u32 = 3;

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

/// Handle to the worker thread.
#[derive(Clone)]
pub struct Hw {
    tx: mpsc::Sender<Job>,
}

impl Hw {
    /// Starts the worker thread.
    pub fn spawn(worker: Worker) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("legion-hw".into())
            .spawn(move || {
                let mut w = worker;
                for job in rx {
                    job(&mut w);
                }
            })?;
        Ok(Self { tx })
    }

    /// Runs `f` on the worker thread and awaits its result.
    pub async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Worker) -> R + Send + 'static,
    ) -> R {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |w| {
                let _ = tx.send(f(w));
            }))
            .expect("hardware worker thread terminated");
        rx.await.expect("hardware worker thread terminated")
    }
}

/// What changed during a tick (drives D-Bus signals).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Events {
    /// New logical profile.
    pub profile: Option<String>,
    /// New temperatures.
    pub temperature: Option<(Option<f64>, Option<f64>)>,
    /// New battery state.
    pub battery: Option<BatteryInfo>,
    /// Other state changed.
    pub hardware: bool,
}

/// Battery fields whose change triggers `BatteryChanged`:
/// (capacity, status, AC online, conservation mode).
type BatteryKey = (Option<u8>, Option<String>, Option<bool>, Option<bool>);

#[derive(Debug, Default)]
struct Last {
    profile: Option<PowerProfile>,
    cpu_c: Option<f64>,
    gpu_c: Option<f64>,
    battery: Option<BatteryKey>,
}

/// State owned by the worker thread.
pub struct Worker {
    ctrl: LinuxController,
    config: Config,
    polkit: bool,
    system: SystemInfo,
    status: Option<Status>,
    cpu_prev: Option<legion_cpu::CpuSample>,
    gpu: GpuInfo,
    gpu_at: Option<Instant>,
    battery: BatteryInfo,
    battery_at: Option<Instant>,
    ppd: std::result::Result<PpdInfo, String>,
    ppd_at: Option<Instant>,
    fan_mode: FanMode,
    fan_last: Option<u32>,
    fan_errors: u32,
    last_safety: Option<String>,
    last: Last,
}

fn changed(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() >= 1.0,
        (None, None) => false,
        _ => true,
    }
}

impl Worker {
    /// Creates the worker (reads the system identification once).
    pub fn new(ctrl: LinuxController, polkit: bool) -> Self {
        let config = ctrl.options().config.clone();
        let system = ctrl.get_system_info().unwrap_or_default();
        Self {
            ctrl,
            config,
            polkit,
            system,
            status: None,
            cpu_prev: None,
            gpu: GpuInfo {
                devices: vec![],
                mode: GpuMode::Unknown,
            },
            gpu_at: None,
            battery: BatteryInfo::default(),
            battery_at: None,
            ppd: Err("not queried yet".into()),
            ppd_at: None,
            fan_mode: FanMode::Auto,
            fan_last: None,
            fan_errors: 0,
            last_safety: None,
            last: Last::default(),
        }
    }

    /// Controller in use.
    pub fn ctrl(&self) -> &LinuxController {
        &self.ctrl
    }

    fn due(at: Option<Instant>, every: Duration) -> bool {
        at.is_none_or(|t| t.elapsed() >= every)
    }

    /// Refreshes the cached status. `resample_cpu` is false for refreshes
    /// right after a change (CPU usage needs a full interval).
    pub fn tick(&mut self, resample_cpu: bool) -> Events {
        let root = self.ctrl.root().clone();
        let mut ev = Events::default();

        let cpu = if resample_cpu || self.status.is_none() {
            let s = legion_cpu::sample(&root);
            let info = legion_cpu::info_from_samples(&root, self.cpu_prev.as_ref(), &s);
            self.cpu_prev = Some(s);
            info
        } else {
            self.status
                .as_ref()
                .map(|s| s.cpu.clone())
                .expect("checked")
        };

        if Self::due(self.gpu_at, GPU_INTERVAL) {
            self.gpu = self.ctrl.get_gpu_info().unwrap_or(GpuInfo {
                devices: vec![],
                mode: GpuMode::Unknown,
            });
            self.gpu_at = Some(Instant::now());
        }
        if Self::due(self.battery_at, BATTERY_INTERVAL) {
            self.battery = legion_battery::read_battery_info(&root);
            self.battery_at = Some(Instant::now());
        }
        let host = self.ctrl.options().host_services;
        if Self::due(self.ppd_at, PPD_INTERVAL) {
            self.ppd = if host {
                legion_profiles::ppd::read()
            } else {
                Err("D-Bus disabled".into())
            };
            self.ppd_at = Some(Instant::now());
        }
        let profile = legion_profiles::combine(
            legion_profiles::read_platform_profile(&root),
            self.ppd.clone(),
        );
        let thermal = self.ctrl.temperatures_with(&self.gpu);
        let fans = legion_fan::read_fans(&root);

        self.control_fans(thermal.cpu_c, thermal.gpu_c, &mut ev);

        // events
        if self.last.profile != Some(profile.current) {
            if self.last.profile.is_some() {
                ev.profile = Some(profile.current.to_string());
            }
            self.last.profile = Some(profile.current);
        }
        if changed(self.last.cpu_c, thermal.cpu_c) || changed(self.last.gpu_c, thermal.gpu_c) {
            ev.temperature = Some((thermal.cpu_c, thermal.gpu_c));
            self.last.cpu_c = thermal.cpu_c;
            self.last.gpu_c = thermal.gpu_c;
        }
        let b = self.battery.batteries.first();
        let bkey = (
            b.and_then(|b| b.capacity_percent),
            b.and_then(|b| b.status.clone()),
            self.battery.ac_online,
            self.battery.conservation_mode,
        );
        if self.last.battery.as_ref() != Some(&bkey) {
            ev.battery = Some(self.battery.clone());
            self.last.battery = Some(bkey);
        }

        self.status = Some(Status {
            system: self.system.clone(),
            profile,
            cpu,
            gpu: self.gpu.clone(),
            memory: self.ctrl.get_memory_info().unwrap_or_default(),
            thermal,
            fans,
            battery: self.battery.clone(),
        });
        ev
    }

    /// Refresh after a change: re-reads profile and battery immediately.
    pub fn after_change(&mut self) -> Events {
        self.ppd_at = None;
        self.battery_at = None;
        let mut ev = self.tick(false);
        ev.hardware = true;
        ev
    }

    fn revert_fans(&mut self, reason: String, ev: &mut Events) {
        tracing::warn!(target: "legion_audit", %reason, "returning fan control to firmware");
        if let Err(e) = self.ctrl.set_fan_target(None, 0) {
            tracing::error!(error = %e, "could not restore automatic fan control");
        }
        self.fan_mode = FanMode::Auto;
        self.fan_last = None;
        self.fan_errors = 0;
        self.last_safety = Some(reason);
        ev.hardware = true;
    }

    fn control_fans(&mut self, cpu_c: Option<f64>, gpu_c: Option<f64>, ev: &mut Events) {
        let desired = match &self.fan_mode {
            FanMode::Auto => return,
            _ if !self.config.enable_fan_control => {
                return self.revert_fans("fan control disabled in configuration".into(), ev);
            }
            FanMode::Fixed { rpm } => *rpm,
            FanMode::Curve(c) => {
                let Some(cpu) = cpu_c else {
                    return self.revert_fans("CPU temperature unavailable".into(), ev);
                };
                let hottest = gpu_c.map_or(cpu, |g| g.max(cpu));
                legion_fan::evaluate_curve(c, hottest, self.fan_last)
            }
        };
        // thermal safety net
        match cpu_c {
            None => return self.revert_fans("CPU temperature unavailable".into(), ev),
            Some(c) if c >= self.config.fan_safety_cpu_c => {
                return self.revert_fans(format!("CPU reached {c:.0} °C"), ev);
            }
            _ => {}
        }
        if let Some(g) = gpu_c.filter(|g| *g >= self.config.fan_safety_gpu_c) {
            return self.revert_fans(format!("GPU reached {g:.0} °C"), ev);
        }
        // write only when the firmware value differs (it may reset targets)
        let root = self.ctrl.root().clone();
        let fans = legion_fan::controllable_fans(&root);
        let current = legion_fan::current_targets(&root);
        for (i, f) in fans.iter().enumerate() {
            let Ok(want) = legion_fan::normalise_target(f, desired) else {
                return self.revert_fans(format!("invalid target {desired} RPM"), ev);
            };
            if current.get(i).copied().flatten() != Some(want) {
                match self.ctrl.set_fan_target(Some(i + 1), desired) {
                    Ok(()) => self.fan_errors = 0,
                    Err(e) => {
                        self.fan_errors += 1;
                        tracing::error!(error = %e, "fan write failed");
                        if self.fan_errors >= MAX_FAN_ERRORS {
                            return self.revert_fans(format!("fan writes failing: {e}"), ev);
                        }
                    }
                }
            }
        }
        self.fan_last = Some(desired);
    }

    /// Changes the fan mode (validated).
    pub fn set_fan_mode(&mut self, mode: FanMode) -> Result<Events> {
        if mode != FanMode::Auto && !self.config.enable_fan_control {
            return Err(HwError::NotSupported(format!(
                "manual fan control is disabled; set \"enable_fan_control\": true in {}",
                legion_hardware::config::CONFIG_PATH
            )));
        }
        legion_fan::validate_mode(self.ctrl.root(), &mode)?;
        tracing::info!(target: "legion_audit", mode = %custom::fan_mode_name(&mode), "fan mode changed");
        let was_manual = self.fan_mode != FanMode::Auto;
        self.fan_mode = mode;
        self.fan_last = None;
        self.fan_errors = 0;
        self.last_safety = None;
        if self.fan_mode == FanMode::Auto
            && (was_manual || !legion_fan::controllable_fans(self.ctrl.root()).is_empty())
        {
            self.ctrl.set_fan_target(None, 0)?;
        }
        Ok(self.after_change())
    }

    /// Toggles an experimental feature and persists the configuration.
    pub fn set_experimental(&mut self, feature: &str, enabled: bool) -> Result<Events> {
        let mut config = self.config.clone();
        config.set_flag(feature, enabled)?;
        config.save(self.ctrl.root())?;
        self.ctrl.set_config(config.clone());
        self.config = config;
        let mut ev = self.after_change();
        if !self.config.enable_fan_control && self.fan_mode != FanMode::Auto {
            self.revert_fans("fan control disabled in configuration".into(), &mut ev);
        }
        Ok(ev)
    }

    /// Applies a custom profile.
    pub fn apply_profile(&mut self, p: &CustomProfile) -> Result<(Vec<String>, Events)> {
        if let Some(m) = &p.fan_mode {
            legion_fan::validate_mode(self.ctrl.root(), m)?;
        }
        let ctrl = self.ctrl.clone();
        let mut fan = None;
        let done = custom::apply(&ctrl, p, |m| {
            fan = Some(m.clone());
            Ok(())
        })?;
        if let Some(m) = fan {
            self.set_fan_mode(m)?;
        }
        Ok((done, self.after_change()))
    }

    /// Current settings as a profile.
    pub fn capture_profile(&self) -> CustomProfile {
        custom::capture(&self.ctrl, Some(self.fan_mode.clone()))
    }

    /// Cached status (ticks once if empty).
    pub fn status(&mut self) -> Status {
        if self.status.is_none() {
            self.tick(true);
        }
        self.status.clone().expect("status after tick")
    }

    /// Daemon information.
    pub fn info(&self) -> DaemonInfo {
        DaemonInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            fan_mode: self.fan_mode.clone(),
            fan_control_enabled: self.config.enable_fan_control,
            power_limits_enabled: self.config.enable_power_limits,
            polkit: self.polkit,
            last_safety_event: self.last_safety.clone(),
        }
    }

    /// Capability list (computed on demand).
    pub fn capabilities(&self) -> Vec<Capability> {
        capabilities::detect(&self.ctrl)
    }

    /// System identification.
    pub fn system(&self) -> SystemInfo {
        self.system.clone()
    }

    /// Restores firmware fan control before exiting.
    pub fn shutdown(&mut self) {
        if self.fan_mode != FanMode::Auto {
            let mut ev = Events::default();
            self.revert_fans("daemon stopping".into(), &mut ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::model::{CurvePoint, FanCurve};
    use legion_hardware::{ControllerOptions, fixtures};

    fn worker(fs: &legion_core::testing::FakeSysfs, fans: bool) -> Worker {
        let mut o = ControllerOptions::fixture(fs.root());
        o.config.enable_fan_control = fans;
        Worker::new(LinuxController::new(o), false)
    }

    fn target(fs: &legion_core::testing::FakeSysfs, n: u32) -> String {
        fs.root()
            .read(format!("/sys/class/hwmon/hwmon3/fan{n}_target"))
            .unwrap()
    }

    #[test]
    fn tick_builds_status_and_events() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, false);
        let ev = w.tick(true);
        assert!(ev.temperature.is_some());
        assert!(ev.battery.is_some());
        assert!(ev.profile.is_none()); // first tick establishes the baseline
        assert_eq!(w.status().profile.current, PowerProfile::Performance);
        fs.file(
            "/sys/class/platform-profile/platform-profile-0/profile",
            "low-power",
        );
        let ev = w.tick(true);
        assert_eq!(ev.profile.as_deref(), Some("quiet"));
        assert!(w.tick(true).temperature.is_none()); // unchanged
    }

    #[test]
    fn fan_mode_requires_config() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, false);
        assert!(w.set_fan_mode(FanMode::Fixed { rpm: 3000 }).is_err());
        assert!(w.set_fan_mode(FanMode::Auto).is_ok());
    }

    #[test]
    fn fixed_and_curve_modes_with_safety() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, true);
        w.tick(true);
        w.set_fan_mode(FanMode::Fixed { rpm: 3000 }).unwrap();
        assert_eq!(target(&fs, 1), "3000");
        assert_eq!(target(&fs, 2), "3000");
        // firmware resets the target → daemon re-applies it
        fs.file("/sys/class/hwmon/hwmon3/fan1_target", "0");
        w.tick(true);
        assert_eq!(target(&fs, 1), "3000");

        // curve: package temp in fixture is 71 °C → 65 °C step
        let curve = FanCurve {
            points: vec![
                CurvePoint {
                    temp_c: 55,
                    rpm: 2200,
                },
                CurvePoint {
                    temp_c: 65,
                    rpm: 2800,
                },
                CurvePoint {
                    temp_c: 80,
                    rpm: 4000,
                },
            ],
            hysteresis_c: 3,
        };
        w.set_fan_mode(FanMode::Curve(curve)).unwrap();
        assert_eq!(target(&fs, 2), "2800");

        // overheating → back to firmware auto
        fs.file("/sys/class/hwmon/hwmon4/temp1_input", "97000");
        let ev = w.tick(true);
        assert!(ev.hardware);
        assert_eq!(target(&fs, 1), "0");
        assert_eq!(w.info().fan_mode, FanMode::Auto);
        assert!(w.info().last_safety_event.unwrap().contains("97"));
    }

    #[test]
    fn invalid_modes_are_rejected() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, true);
        assert!(w.set_fan_mode(FanMode::Fixed { rpm: 100 }).is_err());
        assert!(
            w.set_fan_mode(FanMode::Curve(FanCurve {
                points: vec![],
                hysteresis_c: 3
            }))
            .is_err()
        );
        assert_eq!(target(&fs, 1), "0");
    }

    #[test]
    fn experimental_flags_are_persisted() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, false);
        assert!(!w.info().power_limits_enabled);
        w.set_experimental("enable_power_limits", true).unwrap();
        assert!(w.info().power_limits_enabled);
        assert!(
            legion_hardware::Config::load(&fs.root()).enable_power_limits,
            "saved to daemon.json"
        );
        // the controller uses the new gate immediately
        w.ctrl().set_power_profile(PowerProfile::Custom).unwrap();
        w.ctrl().set_firmware_attribute("ppt_pl1_spl", 90).unwrap();
        assert!(w.set_experimental("enable_ec_writes", true).is_err());
    }

    #[test]
    fn disabling_fan_control_reverts_to_auto() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, true);
        w.tick(true);
        w.set_fan_mode(FanMode::Fixed { rpm: 3000 }).unwrap();
        w.set_experimental("enable_fan_control", false).unwrap();
        assert_eq!(w.info().fan_mode, FanMode::Auto);
        assert_eq!(target(&fs, 1), "0");
    }

    #[test]
    fn shutdown_restores_auto() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, true);
        w.tick(true);
        w.set_fan_mode(FanMode::Fixed { rpm: 4000 }).unwrap();
        w.shutdown();
        assert_eq!(target(&fs, 1), "0");
        assert_eq!(target(&fs, 2), "0");
    }

    #[test]
    fn apply_profile_through_worker() {
        let fs = fixtures::legion_5_15iax10();
        let mut w = worker(&fs, true);
        let p = CustomProfile {
            platform_profile: Some("balanced".into()),
            fan_mode: Some(FanMode::Fixed { rpm: 2500 }),
            ..Default::default()
        };
        let (done, ev) = w.apply_profile(&p).unwrap();
        assert_eq!(done.len(), 2);
        assert!(ev.hardware);
        assert_eq!(target(&fs, 1), "2500");
        assert_eq!(
            w.capture_profile().platform_profile.as_deref(),
            Some("balanced")
        );
    }
}
