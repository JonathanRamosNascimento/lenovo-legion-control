//! End-to-end test: a private dbus-daemon, the real daemon binary serving
//! the Legion 5 15IAX10 fixture, and the shared `legion-dbus` client.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use legion_core::keyboard::{KeyboardInfo, Rgb};
use legion_core::model::{DaemonInfo, FanMode, Status};
use legion_core::profile::PowerProfile;
use legion_dbus::Client;

struct Guard(Vec<Child>);
impl Drop for Guard {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn private_bus() -> Option<(Child, String)> {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?)
        .read_line(&mut line)
        .ok()?;
    Some((child, line.trim().to_owned()))
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timeout waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn daemon_over_dbus() {
    let Some((bus, addr)) = private_bus() else {
        eprintln!("dbus-daemon not available; skipping");
        return;
    };
    let fs = legion_hardware::fixtures::legion_5_15iax10();
    fs.file(
        "/etc/legion-control/daemon.json",
        r#"{"enable_fan_control": true, "enable_power_limits": true, "enable_keyboard_rgb": true}"#,
    );
    let root = fs.root();
    let daemon = Command::new(env!("CARGO_BIN_EXE_legion-control-daemon"))
        .args([
            "--session",
            "--no-polkit",
            "--interval-ms",
            "250",
            "--sysroot",
        ])
        .arg(fs.path())
        .env("DBUS_SESSION_BUS_ADDRESS", &addr)
        .env("RUST_LOG", "warn")
        .spawn()
        .expect("spawn daemon");
    let pid = daemon.id();
    let mut guard = Guard(vec![daemon, bus]);

    let client = Client::connect_address(&addr).unwrap();
    wait_until("daemon", || client.is_running());

    let info: DaemonInfo = client.get(|p| p.get_daemon_info()).unwrap();
    assert!(info.fan_control_enabled && !info.polkit);

    // Phase 2: profiles
    client.call(|p| p.set_profile("quiet")).unwrap();
    assert_eq!(
        root.read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "low-power"
    );
    let st: Status = client.get(|p| p.get_status()).unwrap();
    assert_eq!(st.profile.current, PowerProfile::Quiet);
    let e = client.call(|p| p.set_profile("turbo")).unwrap_err();
    assert!(matches!(e, legion_core::HwError::InvalidValue(_)), "{e}");

    // signals: a change made outside the daemon (Fn+Q) is reported
    let proxy = client.proxy().clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut it = proxy.receive_profile_changed().unwrap();
        if let Some(sig) = it.next() {
            let _ = tx.send(sig.args().unwrap().profile.to_string());
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    fs.file(
        "/sys/class/platform-profile/platform-profile-0/profile",
        "balanced",
    );
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "balanced");

    // Phase 4: battery
    client.call(|p| p.set_battery_conservation(false)).unwrap();
    assert_eq!(
        root.read("/sys/bus/platform/devices/VPC2004:00/conservation_mode")
            .unwrap(),
        "0"
    );
    assert!(client.call(|p| p.set_charge_threshold(80)).is_err());

    // Phase 6: fans
    let fixed = serde_json::to_string(&FanMode::Fixed { rpm: 3000 }).unwrap();
    client.call(|p| p.set_fan_mode(&fixed)).unwrap();
    assert_eq!(
        root.read("/sys/class/hwmon/hwmon3/fan1_target").unwrap(),
        "3000"
    );
    assert!(
        client
            .call(|p| p.set_fan_mode("{\"mode\":\"fixed\",\"rpm\":50}"))
            .is_err()
    );
    assert!(client.call(|p| p.set_fan_mode("not json")).is_err());

    // Phase 7: power limits need the custom profile
    assert!(
        client
            .call(|p| p.set_firmware_attribute("ppt_pl1_spl", 90))
            .is_err()
    );
    client.call(|p| p.set_profile("custom")).unwrap();
    client
        .call(|p| p.set_firmware_attribute("ppt_pl1_spl", 90))
        .unwrap();
    assert!(
        client
            .call(|p| p.set_firmware_attribute("ppt_pl1_spl", 999))
            .is_err()
    );

    // Phase 8: custom profiles
    let changes: Vec<String> = client
        .get(|p| p.apply_profile(r#"{"platform_profile":"balanced","cpu_epp":"balance_power"}"#))
        .unwrap();
    assert_eq!(changes.len(), 2);
    assert!(
        client
            .get::<Vec<String>>(|p| p.apply_profile(r#"{"bogus":1}"#))
            .is_err()
    );
    let captured: legion_profiles::CustomProfile = client.get(|p| p.capture_profile()).unwrap();
    assert_eq!(captured.platform_profile.as_deref(), Some("balanced"));
    assert_eq!(captured.fan_mode, Some(FanMode::Fixed { rpm: 3000 }));

    // keyboard backlight (experimental): colour, backup, restore, gating
    let kb: KeyboardInfo = client.get(|p| p.get_keyboard_info()).unwrap();
    assert_eq!((kb.usb_id.as_str(), kb.enabled), ("048d:c195", true));
    let st = kb.state.unwrap();
    assert_eq!(
        (st.effect.as_str(), st.color, st.backup),
        ("rainbow-wave", None, false)
    );
    client.call(|p| p.set_keyboard_color("#ff8000")).unwrap();
    let st = client
        .get::<KeyboardInfo>(|p| p.get_keyboard_info())
        .unwrap()
        .state
        .unwrap();
    assert_eq!(
        (st.effect.as_str(), st.color),
        ("static", Some(Rgb::new(255, 128, 0)))
    );
    assert!(st.backup);
    assert!(root.exists("/var/lib/legion-control/keyboard-profile-1.hex"));
    client.call(|p| p.set_keyboard_color("blue")).unwrap();
    client.call(|p| p.set_keyboard_brightness(7)).unwrap();
    let st = client
        .get::<KeyboardInfo>(|p| p.get_keyboard_info())
        .unwrap()
        .state
        .unwrap();
    assert_eq!((st.brightness, st.color), (7, Some(Rgb::new(0, 0, 255))));
    assert!(matches!(
        client
            .call(|p| p.set_keyboard_color("not-a-colour"))
            .unwrap_err(),
        legion_core::HwError::InvalidValue(_)
    ));
    assert!(client.call(|p| p.set_keyboard_brightness(10)).is_err());
    client.call(|p| p.restore_keyboard()).unwrap();
    let st = client
        .get::<KeyboardInfo>(|p| p.get_keyboard_info())
        .unwrap()
        .state
        .unwrap();
    assert_eq!((st.effect.as_str(), st.backup), ("rainbow-wave", false));
    assert!(client.call(|p| p.restore_keyboard()).is_err()); // nothing saved any more
    // switching the feature off: no state is read and changes are refused
    client
        .call(|p| p.set_experimental("enable_keyboard_rgb", false))
        .unwrap();
    let kb: KeyboardInfo = client.get(|p| p.get_keyboard_info()).unwrap();
    assert!(!kb.enabled && kb.state.is_none());
    assert!(matches!(
        client.call(|p| p.set_keyboard_color("red")).unwrap_err(),
        legion_core::HwError::NotSupported(_)
    ));

    // configuration toggles are persisted
    client
        .call(|p| p.set_experimental("enable_fan_control", false))
        .unwrap();
    assert!(!legion_hardware::Config::load(&root).enable_fan_control);
    assert!(
        client
            .call(|p| p.set_experimental("enable_ec_writes", true))
            .is_err()
    );
    client
        .call(|p| p.set_experimental("enable_fan_control", true))
        .unwrap();
    client.call(|p| p.set_fan_mode(&fixed)).unwrap();

    // SIGTERM → fans back to firmware control
    Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    let _ = guard.0[0].wait();
    assert_eq!(
        root.read("/sys/class/hwmon/hwmon3/fan1_target").unwrap(),
        "0"
    );
    assert_eq!(
        root.read("/sys/class/hwmon/hwmon3/fan2_target").unwrap(),
        "0"
    );
}
