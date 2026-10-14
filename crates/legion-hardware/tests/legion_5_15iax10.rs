//! End-to-end tests of the Linux controller against the Legion 5 15IAX10
//! fixture tree (no real hardware involved).

use legion_core::capabilities::CapabilityState;
use legion_core::doctor::CheckStatus;
use legion_core::model::{GpuMode, NvmlStatus};
use legion_core::profile::{PowerProfile, ProfileBackend};
use legion_core::{HardwareController, HwError};
use legion_hardware::{ControllerOptions, LinuxController, capabilities, doctor, fixtures, report};

fn controller(fs: &legion_core::testing::FakeSysfs) -> LinuxController {
    LinuxController::new(ControllerOptions::fixture(fs.root()))
}

#[test]
fn system_info() {
    let fs = fixtures::legion_5_15iax10();
    let s = controller(&fs).get_system_info().unwrap();
    assert_eq!(s.manufacturer.as_deref(), Some("LENOVO"));
    assert_eq!(s.product_version.as_deref(), Some("Legion 5 15IAX10"));
    assert_eq!(s.board_name.as_deref(), Some("LNVNB161216"));
    assert_eq!(s.cpu_model.as_deref(), Some("Intel Core Ultra 7 255HX"));
    assert_eq!(s.known_model.as_deref(), Some("Legion 5 15IAX10"));
    assert_eq!(s.gpus.len(), 2);
    assert_eq!(s.os_name.as_deref(), Some("CachyOS"));
}

#[test]
fn profile_switching() {
    let fs = fixtures::legion_5_15iax10();
    let c = controller(&fs);
    let p = c.get_power_profile().unwrap();
    assert_eq!(p.current, PowerProfile::Performance);
    assert_eq!(p.backend, ProfileBackend::PlatformProfile);
    c.set_power_profile(PowerProfile::Quiet).unwrap();
    assert_eq!(
        fs.root()
            .read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "low-power"
    );
    c.set_power_profile(PowerProfile::MaxPower).unwrap();
    assert_eq!(
        c.get_power_profile().unwrap().current,
        PowerProfile::MaxPower
    );
    assert!(matches!(
        c.set_power_profile(PowerProfile::Unknown),
        Err(HwError::NotSupported(_))
    ));
}

fn enabled(fs: &legion_core::testing::FakeSysfs) -> LinuxController {
    let mut o = ControllerOptions::fixture(fs.root());
    o.config.enable_fan_control = true;
    o.config.enable_power_limits = true;
    o.config.enable_keyboard_rgb = true;
    LinuxController::new(o)
}

#[test]
fn experimental_features_are_gated() {
    let fs = fixtures::legion_5_15iax10();
    let c = controller(&fs);
    assert!(matches!(
        c.set_fan_target(None, 3000),
        Err(HwError::NotSupported(_))
    ));
    // returning to auto is always allowed
    c.set_fan_target(None, 0).unwrap();
    c.set_power_profile(PowerProfile::Custom).unwrap();
    assert!(matches!(
        c.set_firmware_attribute("ppt_pl1_spl", 90),
        Err(HwError::NotSupported(_))
    ));
}

#[test]
fn fan_and_power_limit_writes() {
    let fs = fixtures::legion_5_15iax10();
    let c = enabled(&fs);
    c.set_fan_target(Some(1), 3333).unwrap();
    assert_eq!(
        fs.root()
            .read("/sys/class/hwmon/hwmon3/fan1_target")
            .unwrap(),
        "3300"
    );
    assert!(c.set_fan_target(None, 100).is_err());
    // power limits need the custom profile
    let e = c.set_firmware_attribute("ppt_pl1_spl", 90).unwrap_err();
    assert!(e.to_string().contains("custom"));
    c.set_power_profile(PowerProfile::Custom).unwrap();
    c.set_firmware_attribute("ppt_pl1_spl", 90).unwrap();
    assert!(c.set_firmware_attribute("gpu_nv_ctgp", 90).is_err()); // min=max=0
    // PL2 cannot go below PL1 (90)
    assert!(c.set_firmware_attribute("ppt_pl2_sppt", 80).is_err());
    c.set_firmware_attribute("ppt_pl2_sppt", 100).unwrap();
    let pl = c.get_power_limits().unwrap();
    assert!(pl.custom_profile_active && pl.requires_custom_profile);
}

#[test]
fn battery_and_cpu_writes() {
    let fs = fixtures::legion_5_15iax10();
    let c = controller(&fs);
    c.set_battery_conservation(false).unwrap();
    assert_eq!(c.get_battery_info().unwrap().conservation_mode, Some(false));
    c.set_charge_type("Fast").unwrap();
    assert!(c.set_charge_threshold(80).is_err());
    c.set_cpu_epp("balance_power").unwrap();
    assert!(c.set_cpu_epp("turbo").is_err());
    c.set_turbo(false).unwrap();
    c.set_cpu_governor("performance").unwrap();
}

#[test]
fn custom_profile_roundtrip() {
    use legion_core::model::FanMode;
    use legion_hardware::custom;
    let fs = fixtures::legion_5_15iax10();
    let c = enabled(&fs);
    let mut p = legion_profiles::CustomProfile {
        platform_profile: Some("custom".into()),
        cpu_epp: Some("balance_performance".into()),
        battery_conservation: Some(false),
        fan_mode: Some(FanMode::Fixed { rpm: 2500 }),
        ..Default::default()
    };
    p.power_limits.insert("ppt_pl1_spl".into(), 80);
    let done = custom::apply(&c, &p, custom::direct_fan_handler(&c)).unwrap();
    assert_eq!(done.len(), 5, "{done:?}");
    let root = fs.root();
    assert_eq!(
        root.read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "custom"
    );
    assert_eq!(
        root.read(
            "/sys/class/firmware-attributes/lenovo-wmi-other-0/attributes/ppt_pl1_spl/current_value"
        )
        .unwrap(),
        "80"
    );
    assert_eq!(
        root.read("/sys/class/hwmon/hwmon3/fan2_target").unwrap(),
        "2500"
    );
    let captured = custom::capture(&c, None);
    assert_eq!(captured.platform_profile.as_deref(), Some("custom"));
    assert_eq!(captured.power_limits.get("ppt_pl1_spl"), Some(&80));
    assert_eq!(captured.battery_conservation, Some(false));

    // invalid profiles change nothing
    let bad = legion_profiles::CustomProfile {
        platform_profile: Some("low-power".into()),
        cpu_epp: Some("nope".into()),
        ..Default::default()
    };
    assert!(custom::apply(&c, &bad, custom::direct_fan_handler(&c)).is_err());
    assert_eq!(
        root.read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "custom"
    );
}

#[test]
fn gpu_mode_and_suspended_dgpu() {
    let fs = fixtures::legion_5_15iax10();
    let g = controller(&fs).get_gpu_info().unwrap();
    assert_eq!(g.mode, GpuMode::Hybrid);
    let nv = g.devices.iter().find(|d| d.vendor_id == "10de").unwrap();
    assert_eq!(
        nv.name.as_deref(),
        Some("NVIDIA GeForce RTX 5070 Laptop GPU")
    );
    // fixture roots never touch NVML
    assert_eq!(nv.nvml, NvmlStatus::Disabled);
}

#[test]
fn capabilities_match_observed_hardware() {
    let fs = fixtures::legion_5_15iax10();
    let c = controller(&fs);
    let caps = capabilities::detect(&c);
    let state = |id: &str| caps.iter().find(|c| c.id == id).unwrap().state;
    assert_eq!(state("power_profiles"), CapabilityState::Supported);
    assert_eq!(state("fan_monitoring"), CapabilityState::ReadOnly);
    // experimental features are off by default
    assert_eq!(state("fan_control"), CapabilityState::Detected);
    assert_eq!(state("fan_curve"), CapabilityState::Detected);
    assert_eq!(state("battery_threshold"), CapabilityState::Unsupported);
    assert_eq!(state("battery_conservation"), CapabilityState::Supported);
    assert_eq!(state("charge_type"), CapabilityState::Supported);
    assert_eq!(state("cpu_tuning"), CapabilityState::Supported);
    assert_eq!(state("cpu_power_limit"), CapabilityState::Detected);
    assert_eq!(state("gpu_power_limit"), CapabilityState::Detected);
    // experimental, off by default (the controller is detected through sysfs)
    assert_eq!(state("rgb_control"), CapabilityState::Detected);
    assert_eq!(state("gpu_mode"), CapabilityState::ReadOnly);
    let caps = capabilities::detect(&enabled(&fs));
    let state = |id: &str| caps.iter().find(|c| c.id == id).unwrap().state;
    assert_eq!(state("fan_control"), CapabilityState::Supported);
    assert_eq!(state("cpu_power_limit"), CapabilityState::Supported);
    assert_eq!(state("rgb_control"), CapabilityState::Supported);
    let summary = c.get_capabilities().unwrap();
    assert!(summary.power_profiles && !summary.battery_threshold && summary.battery_conservation);
}

#[test]
fn doctor_on_fixture() {
    let fs = fixtures::legion_5_15iax10();
    let r = doctor::run(&controller(&fs));
    let get = |n: &str| {
        r.checks
            .iter()
            .find(|c| c.name == n)
            .unwrap_or_else(|| panic!("missing check {n}"))
    };
    assert_eq!(get("Hardware detection").status, CheckStatus::Pass);
    assert_eq!(get("Support matrix").status, CheckStatus::Pass);
    assert_eq!(get("Power profile").status, CheckStatus::Pass);
    assert_eq!(get("Lenovo interfaces").status, CheckStatus::Pass);
    assert_eq!(get("Fan monitoring").status, CheckStatus::Pass);
    assert_eq!(get("Fan control").status, CheckStatus::Pass);
    assert_eq!(get("Battery threshold").status, CheckStatus::Unsupported);
    assert_eq!(get("Battery conservation").status, CheckStatus::Pass);
    assert_eq!(get("Thermal").status, CheckStatus::Pass);
    assert_eq!(get("systemd").status, CheckStatus::Pass);
    assert_ne!(r.overall(), CheckStatus::Fail, "{r:#?}");
}

#[test]
fn report_json() {
    let fs = fixtures::legion_5_15iax10();
    let r = report::build(&controller(&fs));
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["manufacturer"], "LENOVO");
    assert_eq!(v["product"], "Legion 5 15IAX10");
    assert_eq!(v["board"], "LNVNB161216");
    assert_eq!(v["cpu"], "Intel Core Ultra 7 255HX");
    assert_eq!(v["features"]["power_profiles"], true);
    assert_eq!(v["features"]["battery_threshold"], false);
    assert_eq!(v["hwmon"].as_array().unwrap().len(), 6);
    let kb = &v["keyboard_controllers"];
    assert_eq!(kb[0]["usb_id"], "048d:c195");
    assert_eq!(kb[0]["feature_report_7_bytes"], 959);
    assert!(v["acpi"]["wmi_devices"].as_array().unwrap().len() >= 7);
    let text = v.to_string();
    for forbidden in ["serial", "uuid"] {
        assert!(
            !text.to_lowercase().contains(forbidden),
            "report contains {forbidden}"
        );
    }
}

#[test]
fn empty_machine_does_not_panic() {
    let fs = legion_core::testing::FakeSysfs::new();
    let c = controller(&fs);
    let r = doctor::run(&c);
    assert_eq!(r.overall(), CheckStatus::Fail);
    let _ = report::build(&c);
}

#[test]
fn power_limit_write_order() {
    use legion_hardware::custom::limit_order;
    use std::collections::BTreeMap;
    let fs = fixtures::legion_5_15iax10(); // PL2 currently 168
    let c = enabled(&fs);
    let mut m = BTreeMap::new();
    m.insert("ppt_pl1_spl".to_string(), 100);
    m.insert("ppt_pl2_sppt".to_string(), 120);
    m.insert("cpu_temp".to_string(), 95);
    // PL1 100 <= current PL2 168 → PL1 first
    let names: Vec<_> = limit_order(&c, &m).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["ppt_pl1_spl", "ppt_pl2_sppt", "cpu_temp"]);
    // current PL2 lowered to 90 → raising PL1 to 100 needs PL2 first
    fs.file(
        "/sys/class/firmware-attributes/lenovo-wmi-other-0/attributes/ppt_pl2_sppt/current_value",
        "90",
    );
    let names: Vec<_> = limit_order(&c, &m).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["ppt_pl2_sppt", "ppt_pl1_spl", "cpu_temp"]);
}
