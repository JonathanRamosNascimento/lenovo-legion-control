//! Runs the real `legionctl` binary against the Legion 5 15IAX10 fixture.

use std::process::Command;

use legion_hardware::fixtures;

fn legionctl(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_legionctl"))
        .arg("--sysroot")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .env("LEGION_LANG", "en")
        .env_remove("RUST_LOG")
        .output()
        .expect("run legionctl")
}

fn json(root: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    let out = legionctl(root, &a);
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: {e}\n{}", String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn doctor_json() {
    let fs = fixtures::legion_5_15iax10();
    let v = json(fs.path(), &["doctor"]);
    let checks = v["checks"].as_array().unwrap();
    let find = |n: &str| checks.iter().find(|c| c["name"] == n).unwrap()["status"].clone();
    assert_eq!(find("Hardware detection"), "PASS");
    assert_eq!(find("Power profile"), "PASS");
    assert_eq!(find("Fan control"), "PASS");
}

#[test]
fn doctor_text() {
    let fs = fixtures::legion_5_15iax10();
    let out = legionctl(fs.path(), &["doctor"]);
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("Hardware detection"));
    assert!(s.contains("Legion 5 15IAX10"));
}

#[test]
fn status_like_commands() {
    let fs = fixtures::legion_5_15iax10();
    assert_eq!(json(fs.path(), &["profile"])["current"], "performance");
    assert_eq!(
        json(fs.path(), &["cpu", "status"])["model"],
        "Intel Core Ultra 7 255HX"
    );
    assert_eq!(json(fs.path(), &["gpu", "status"])["mode"], "hybrid");
    assert_eq!(
        json(fs.path(), &["fan", "status"])["fans"]["fans"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(json(fs.path(), &["battery"])["conservation_mode"], true);
    assert_eq!(json(fs.path(), &["thermal"])["cpu_c"], 71.0);
    assert_eq!(json(fs.path(), &["hardware"])["board_name"], "LNVNB161216");
    assert_eq!(
        json(fs.path(), &["status"])["system"]["product_version"],
        "Legion 5 15IAX10"
    );
    assert_eq!(json(fs.path(), &["version"])["name"], "legionctl");
    for cmd in [
        "status",
        "profile",
        "thermal",
        "battery",
        "hardware",
        "capabilities",
    ] {
        let out = legionctl(fs.path(), &[cmd]);
        assert!(
            out.status.success(),
            "{cmd}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn write_commands_change_the_fixture() {
    let fs = fixtures::legion_5_15iax10();
    let root = fs.root();
    let ok = |args: &[&str]| {
        let out = legionctl(fs.path(), args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let fails = |args: &[&str]| {
        let out = legionctl(fs.path(), args);
        assert_eq!(out.status.code(), Some(1), "{args:?} should fail");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };

    // Phase 2
    ok(&["profile", "quiet"]);
    assert_eq!(
        root.read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "low-power"
    );
    fails(&["profile", "turbo"]);
    assert_eq!(
        root.read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "low-power"
    );

    // Phase 4
    ok(&["battery", "conservation", "off"]);
    assert_eq!(
        root.read("/sys/bus/platform/devices/VPC2004:00/conservation_mode")
            .unwrap(),
        "0"
    );
    ok(&["battery", "charge-type", "Fast"]);
    fails(&["battery", "charge-type", "Nitro"]);
    assert!(fails(&["battery", "threshold", "80"]).contains("conservation"));
    fails(&["battery", "threshold", "0"]);

    // Phase 6: gated by daemon.json
    assert!(fails(&["fan", "fixed", "3000"]).contains("enable_fan_control"));
    ok(&["fan", "auto"]);

    // Phase 7
    ok(&["cpu", "epp", "balance_power"]);
    fails(&["cpu", "epp", "ludicrous"]);
    ok(&["cpu", "turbo", "off"]);
    assert_eq!(
        root.read("/sys/devices/system/cpu/intel_pstate/no_turbo")
            .unwrap(),
        "1"
    );
    assert!(fails(&["power-limit", "set", "ppt_pl1_spl", "90"]).contains("enable_power_limits"));

    // enable experimental features for this fixture (direct backend)
    ok(&["config", "set", "enable_fan_control", "on"]);
    ok(&["config", "set", "enable_power_limits", "on"]);
    fails(&["config", "set", "enable_ec_writes", "on"]);
    assert_eq!(json(fs.path(), &["config"])["enable_power_limits"], true);
    ok(&["fan", "fixed", "3000"]);
    assert_eq!(
        root.read("/sys/class/hwmon/hwmon3/fan1_target").unwrap(),
        "3000"
    );
    fails(&["fan", "fixed", "99999"]);
    assert!(fails(&["power-limit", "set", "ppt_pl1_spl", "90"]).contains("custom"));
    ok(&["profile", "custom"]);
    ok(&["power-limit", "set", "ppt_pl1_spl", "90"]);
    fails(&["power-limit", "set", "ppt_pl1_spl", "500"]);
    let v = json(fs.path(), &["power-limit"]);
    assert_eq!(v["custom_profile_active"], true);
}

#[test]
fn custom_profiles() {
    let fs = fixtures::legion_5_15iax10();
    let cfg = std::env::temp_dir().join(format!("legionctl-cfg-{}", std::process::id()));
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_legionctl"))
            .arg("--sysroot")
            .arg(fs.path())
            .args(args)
            .env("XDG_CONFIG_HOME", &cfg)
            .env("LEGION_LANG", "en")
            .output()
            .unwrap()
    };
    assert!(
        run(&["custom", "save", "current", "-d", "baseline"])
            .status
            .success()
    );
    let file = cfg.join("legion-control/profiles/current.json");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved["platform_profile"], "performance");
    assert_eq!(saved["description"], "baseline");

    let quiet = cfg.join("quiet.json");
    std::fs::write(
        &quiet,
        r#"{"platform_profile":"low-power","battery_conservation":true,"cpu_epp":"power"}"#,
    )
    .unwrap();
    assert!(
        run(&["custom", "import", "quiet", quiet.to_str().unwrap()])
            .status
            .success()
    );
    let bad = cfg.join("bad.json");
    std::fs::write(&bad, r#"{"platform_profile":"warp"}"#).unwrap();
    assert!(
        !run(&["custom", "import", "bad", bad.to_str().unwrap()])
            .status
            .success()
    );
    assert!(!run(&["custom", "save", "../evil"]).status.success());

    let out = run(&["--json", "custom", "list"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["profiles"], serde_json::json!(["current", "quiet"]));

    assert!(run(&["custom", "apply", "quiet"]).status.success());
    assert_eq!(
        fs.root()
            .read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "low-power"
    );
    assert!(run(&["custom", "apply", "current"]).status.success());
    assert_eq!(
        fs.root()
            .read("/sys/class/platform-profile/platform-profile-0/profile")
            .unwrap(),
        "performance"
    );
    assert!(run(&["custom", "delete", "quiet"]).status.success());
    assert!(!run(&["custom", "apply", "quiet"]).status.success());
    let _ = std::fs::remove_dir_all(cfg);
}

#[test]
fn hardware_scan_writes_report() {
    let fs = fixtures::legion_5_15iax10();
    let dir = tempfile_dir();
    let out_path = dir.join("hardware-report.json");
    let out = legionctl(
        fs.path(),
        &["hardware-scan", "-o", out_path.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(v["manufacturer"], "LENOVO");
    assert_eq!(v["features"]["power_profiles"], true);
    assert_eq!(v["features"]["fan_control"], true); // interface detected, not used
    let _ = std::fs::remove_dir_all(dir);
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("legionctl-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn portuguese_output() {
    let fs = fixtures::legion_5_15iax10();
    let out = legionctl(fs.path(), &["--lang", "pt-BR", "doctor"]);
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("Diagnóstico do Legion Control"), "{s}");
    assert!(s.contains("Detecção de hardware"), "{s}");
    assert!(s.contains("Resultado geral"), "{s}");
    let out = legionctl(fs.path(), &["--lang", "pt-BR", "battery"]);
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("Não está carregando"), "{s}");
    assert!(s.contains("Conservação"), "{s}");
    // errors are translated too
    let out = legionctl(fs.path(), &["--lang", "pt-BR", "fan", "fixed", "3000"]);
    let e = String::from_utf8(out.stderr).unwrap();
    assert!(e.contains("desativado"), "{e}");
    // JSON stays language-neutral
    let out = legionctl(fs.path(), &["--lang", "pt-BR", "--json", "battery"]);
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("Not charging"), "{s}");
    // unknown language
    assert_eq!(
        legionctl(fs.path(), &["--lang", "xx", "status"])
            .status
            .code(),
        Some(2)
    );
}
