//! Fake sysfs/procfs trees reproducing real machines.
//!
//! Values were captured (read-only) on the reference hardware and trimmed.
//! Serial numbers and other personal data are not included.

use legion_core::testing::FakeSysfs;

/// Lenovo Legion 5 15IAX10 (83F0, board LNVNB161216, BIOS S2CN26WW,
/// Linux 7.2), captured with the dGPU runtime-suspended.
pub fn legion_5_15iax10() -> FakeSysfs {
    let fs = FakeSysfs::new();
    let dmi = "/sys/class/dmi/id";
    for (k, v) in [
        ("sys_vendor", "LENOVO"),
        ("product_name", "83F0"),
        ("product_version", "Legion 5 15IAX10"),
        ("product_family", "Legion 5 15IAX10"),
        ("board_vendor", "LENOVO"),
        ("board_name", "LNVNB161216"),
        ("bios_vendor", "LENOVO"),
        ("bios_version", "S2CN26WW"),
        ("bios_date", "03/24/2026"),
        ("ec_firmware_release", "1.22"),
    ] {
        fs.file(&format!("{dmi}/{k}"), &format!("{v}\n"));
    }

    // procfs
    let mut cpuinfo = String::new();
    for i in 0..20 {
        cpuinfo.push_str(&format!(
            "processor\t: {i}\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Core(TM) Ultra 7 255HX\n\n"
        ));
    }
    fs.file("/proc/cpuinfo", &cpuinfo)
        .file("/proc/stat", "cpu  1184453 384592 377350 20483678 4052108 58314 39282 0 0 0\n")
        .file(
            "/proc/meminfo",
            "MemTotal:       15774724 kB\nMemFree:         2626368 kB\nMemAvailable:    5105608 kB\n\
             SwapTotal:      15774716 kB\nSwapFree:        6384956 kB\n",
        )
        .file("/proc/sys/kernel/osrelease", "7.2.8-1-cachyos\n")
        .file("/proc/self/status", "Name:\tlegionctl\nUid:\t1000\t1000\t1000\t1000\n")
        .file("/etc/os-release", "NAME=\"CachyOS Linux\"\nPRETTY_NAME=\"CachyOS\"\nID=cachyos\n")
        .dir("/run/systemd/system");

    // CPU
    let cpu = "/sys/devices/system/cpu";
    for i in 0..20 {
        let khz = if i < 8 { "4733497" } else { "3900000" };
        fs.file(&format!("{cpu}/cpu{i}/cpufreq/scaling_cur_freq"), khz);
        let p = format!("{cpu}/cpufreq/policy{i}");
        fs.file(&format!("{p}/scaling_driver"), "intel_pstate")
            .file(&format!("{p}/scaling_governor"), "powersave")
            .file(
                &format!("{p}/scaling_available_governors"),
                "performance powersave",
            )
            .file(&format!("{p}/energy_performance_preference"), "performance")
            .file(
                &format!("{p}/energy_performance_available_preferences"),
                "default performance balance_performance balance_power power",
            )
            .file(
                &format!("{p}/cpuinfo_max_freq"),
                if i < 8 { "5300000" } else { "4500000" },
            )
            .file(&format!("{p}/cpuinfo_min_freq"), "800000");
    }
    fs.file(&format!("{cpu}/intel_pstate/no_turbo"), "0")
        .file(&format!("{cpu}/intel_pstate/status"), "active")
        .file("/sys/devices/cpu_core/cpus", "0-7")
        .file("/sys/devices/cpu_atom/cpus", "8-19");
    let rapl = "/sys/class/powercap/intel-rapl:0";
    fs.file(&format!("{rapl}/name"), "package-0")
        .file(&format!("{rapl}/max_energy_range_uj"), "262143328850")
        .file(&format!("{rapl}/constraint_0_name"), "long_term")
        .file(&format!("{rapl}/constraint_0_power_limit_uw"), "105000000")
        .file(&format!("{rapl}/constraint_0_time_window_us"), "159383552")
        .file(&format!("{rapl}/constraint_1_name"), "short_term")
        .file(&format!("{rapl}/constraint_1_power_limit_uw"), "168000000")
        .file(&format!("{rapl}/constraint_1_time_window_us"), "2440");

    // hwmon
    let h = "/sys/class/hwmon";
    fs.file(&format!("{h}/hwmon0/name"), "ADP0")
        .file(&format!("{h}/hwmon1/name"), "BAT0")
        .file(&format!("{h}/hwmon1/in0_input"), "16352")
        .file(&format!("{h}/hwmon2/name"), "nvme")
        .file(&format!("{h}/hwmon2/temp1_input"), "46850")
        .file(&format!("{h}/hwmon2/temp1_label"), "Composite")
        .file(&format!("{h}/hwmon2/temp1_crit"), "89850")
        .file(&format!("{h}/hwmon2/temp2_input"), "48850")
        .file(&format!("{h}/hwmon2/temp2_label"), "Sensor 1")
        .file(&format!("{h}/hwmon3/name"), "lenovo_wmi_other")
        .file(&format!("{h}/hwmon4/name"), "coretemp")
        .file(&format!("{h}/hwmon4/temp1_input"), "71000")
        .file(&format!("{h}/hwmon4/temp1_label"), "Package id 0")
        .file(&format!("{h}/hwmon4/temp1_crit"), "105000")
        .file(&format!("{h}/hwmon4/temp2_input"), "62000")
        .file(&format!("{h}/hwmon4/temp2_label"), "Core 0")
        .file(&format!("{h}/hwmon6/name"), "mt7925_phy0")
        .file(&format!("{h}/hwmon6/temp1_input"), "52000");
    for n in 1..=2 {
        fs.file(&format!("{h}/hwmon3/fan{n}_input"), "3000")
            .file(&format!("{h}/hwmon3/fan{n}_min"), "1800")
            .file(&format!("{h}/hwmon3/fan{n}_max"), "4600")
            .file(&format!("{h}/hwmon3/fan{n}_div"), "100")
            .file(&format!("{h}/hwmon3/fan{n}_target"), "0")
            .mode(&format!("{h}/hwmon3/fan{n}_target"), 0o644)
            .mode(&format!("{h}/hwmon3/fan{n}_input"), 0o444);
    }

    // thermal zones
    for (i, (t, v)) in [
        ("INT3400 Thermal", "20000"),
        ("SEN1", "55050"),
        ("SEN2", "68050"),
        ("SEN5", "73050"),
        ("TCPU", "64050"),
    ]
    .iter()
    .enumerate()
    {
        fs.file(&format!("/sys/class/thermal/thermal_zone{i}/type"), t)
            .file(&format!("/sys/class/thermal/thermal_zone{i}/temp"), v);
    }
    fs.file("/sys/class/thermal/thermal_zone10/type", "x86_pkg_temp")
        .file("/sys/class/thermal/thermal_zone10/temp", "73000");

    // power supply
    let bat = "/sys/class/power_supply/BAT0";
    for (k, v) in [
        ("type", "Battery"),
        ("status", "Not charging"),
        ("capacity", "75"),
        ("cycle_count", "55"),
        ("power_now", "0"),
        ("voltage_now", "16352000"),
        ("energy_now", "61500000"),
        ("energy_full", "81990000"),
        ("energy_full_design", "80000000"),
        ("technology", "Li-poly"),
        ("manufacturer", "BYD"),
        ("model_name", "L24B4PC0"),
        ("charge_types", "Fast Standard [Long_Life]"),
    ] {
        fs.file(&format!("{bat}/{k}"), v);
    }
    fs.file("/sys/class/power_supply/ADP0/type", "Mains")
        .file("/sys/class/power_supply/ADP0/online", "1")
        .file(
            "/sys/class/power_supply/ucsi-source-psy-USBC000:001/type",
            "USB",
        );

    // platform profile
    let choices = "low-power balanced performance max-power custom";
    fs.file("/sys/firmware/acpi/platform_profile", "performance\n")
        .mode("/sys/firmware/acpi/platform_profile", 0o644)
        .file("/sys/firmware/acpi/platform_profile_choices", choices)
        .file("/sys/firmware/acpi/tables/DSDT", "")
        .file("/sys/firmware/acpi/tables/SSDT1", "")
        .file(
            "/sys/class/platform-profile/platform-profile-0/name",
            "lenovo-wmi-gamezone",
        )
        .file(
            "/sys/class/platform-profile/platform-profile-0/profile",
            "performance",
        )
        .file(
            "/sys/class/platform-profile/platform-profile-0/choices",
            choices,
        );

    // ACPI / WMI
    fs.file("/sys/bus/acpi/devices/VPC2004:00/hid", "VPC2004")
        .file("/sys/bus/acpi/devices/PNP0C14:01/hid", "PNP0C14");
    for (guid, inst, drv) in [
        (
            "887B54E3-DDDC-4B2C-8B88-68A26A8835D0",
            2,
            Some("lenovo_wmi_gamezone"),
        ),
        (
            "DC2A8805-3A8C-41BA-A6F7-092E0089CD3B",
            5,
            Some("lenovo_wmi_other"),
        ),
        (
            "362A3AFE-3D96-4665-8530-96DAD5BB300E",
            16,
            Some("lenovo_wmi_capdata"),
        ),
        (
            "D320289E-8FEA-41E0-86F9-911D83151B5F",
            23,
            Some("lenovo_wmi_events"),
        ),
        (
            "CE6C0974-0407-4F50-88BA-4FC3B6559AD8",
            29,
            Some("lenovo_wmi_hotkey_utilities"),
        ),
        (
            "8FC0DE0C-B4E4-43FD-B0F3-8871711C1294",
            30,
            Some("ideapad_wmi"),
        ),
        ("0F7B30EC-AC90-4B34-B62F-87F85115505D", 26, None),
    ] {
        let d = format!("/sys/bus/wmi/devices/{guid}-{inst}");
        fs.file(&format!("{d}/guid"), guid)
            .file(&format!("{d}/instance_count"), "1");
        if let Some(drv) = drv {
            fs.symlink(
                &format!("{d}/driver"),
                &format!("/sys/bus/wmi/drivers/{drv}"),
            );
        }
    }
    let fw = "/sys/class/firmware-attributes/lenovo-wmi-other-0/attributes";
    for (name, cur, min, max, disp) in [
        (
            "ppt_pl1_spl",
            "105",
            "50",
            "105",
            "Set the CPU sustained power limit",
        ),
        (
            "ppt_pl2_sppt",
            "168",
            "60",
            "168",
            "Set the CPU slow package power tracking limit",
        ),
        (
            "cpu_temp",
            "103",
            "85",
            "105",
            "Set the CPU thermal load limit",
        ),
        (
            "gpu_nv_ctgp",
            "100",
            "0",
            "0",
            "Set the GPU configurable total graphics power",
        ),
        (
            "gpu_temp",
            "87",
            "75",
            "87",
            "Set the GPU thermal load limit",
        ),
    ] {
        let d = format!("{fw}/{name}");
        fs.file(&format!("{d}/current_value"), cur)
            .mode(&format!("{d}/current_value"), 0o644)
            .file(&format!("{d}/min_value"), min)
            .file(&format!("{d}/max_value"), max)
            .file(&format!("{d}/type"), "integer")
            .file(&format!("{d}/display_name"), disp);
    }
    let vpc = "/sys/bus/platform/devices/VPC2004:00";
    fs.file(&format!("{vpc}/conservation_mode"), "1")
        .file(&format!("{vpc}/fan_mode"), "0")
        .file(&format!("{vpc}/fn_lock"), "0")
        .file(&format!("{vpc}/usb_charging"), "1")
        .dir("/sys/bus/platform/drivers/ideapad_acpi/VPC2004:00");

    // GPUs
    let igpu = "/sys/devices/pci0000:00/0000:00:02.0";
    let dgpu = "/sys/devices/pci0000:00/0000:00:06.0/0000:02:00.0";
    fs.file(&format!("{igpu}/class"), "0x030000")
        .file(&format!("{igpu}/vendor"), "0x8086")
        .file(&format!("{igpu}/device"), "0x7d67")
        .file(&format!("{igpu}/boot_vga"), "1")
        .file(&format!("{igpu}/power/runtime_status"), "active")
        .symlink(&format!("{igpu}/driver"), "/sys/bus/pci/drivers/xe")
        .file(&format!("{dgpu}/class"), "0x030000")
        .file(&format!("{dgpu}/vendor"), "0x10de")
        .file(&format!("{dgpu}/device"), "0x2d18")
        .file(&format!("{dgpu}/boot_vga"), "0")
        .file(&format!("{dgpu}/power/runtime_status"), "suspended")
        .symlink(&format!("{dgpu}/driver"), "/sys/bus/pci/drivers/nvidia")
        .symlink("/sys/bus/pci/devices/0000:00:02.0", igpu)
        .symlink("/sys/bus/pci/devices/0000:02:00.0", dgpu)
        .symlink("/sys/class/drm/card1/device", igpu)
        .symlink("/sys/class/drm/card0/device", dgpu)
        .file("/sys/class/drm/card1-eDP-1/status", "connected")
        .file("/sys/class/drm/card0-eDP-2/status", "disconnected")
        .file(
            "/proc/driver/nvidia/gpus/0000:02:00.0/information",
            "Model: \t\t NVIDIA GeForce RTX 5070 Laptop GPU\nIRQ:   \t\t 16\n",
        )
        .file(
            "/proc/driver/nvidia/version",
            "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.71.09  Release Build\n",
        )
        .file(
            "/usr/share/hwdata/pci.ids",
            "8086  Intel Corporation\n\t7d67  Arrow Lake-S [Intel Graphics]\n\
             10de  NVIDIA Corporation\n\t2d18  GB206M [GeForce RTX 5070 Max-Q / Mobile]\n",
        )
        .file("/sys/class/leds/platform::fnlock/brightness", "0");

    // built-in keyboard controller: ITE 048d:c195, two USB interfaces
    // (real report descriptors; fixture roots use a simulated controller)
    legion_keyboard::sim::add_legion_5_15iax10_keyboard(&fs);
    fs
}
