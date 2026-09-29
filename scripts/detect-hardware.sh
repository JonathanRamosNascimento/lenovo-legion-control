#!/usr/bin/env bash
# Read-only hardware summary without Rust tooling.
# Prints DMI (no serial numbers), CPU, GPUs, platform profile, Lenovo drivers,
# hwmon chips and battery charge controls. Writes nothing.
set -euo pipefail

rd() { [[ -r "$1" ]] && tr -d '\n' < "$1" || printf 'n/a'; }

echo "== DMI"
for f in sys_vendor product_name product_version board_name bios_version bios_date; do
    printf '  %-16s %s\n' "$f" "$(rd /sys/class/dmi/id/$f)"
done

echo "== CPU"
printf '  %s\n' "$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //')"
printf '  driver=%s governor=%s epp=%s\n' \
    "$(rd /sys/devices/system/cpu/cpufreq/policy0/scaling_driver)" \
    "$(rd /sys/devices/system/cpu/cpufreq/policy0/scaling_governor)" \
    "$(rd /sys/devices/system/cpu/cpufreq/policy0/energy_performance_preference)"

echo "== GPUs (PCI class 03)"
for d in /sys/bus/pci/devices/*; do
    [[ "$(rd "$d/class")" == 0x03* ]] || continue
    drv=$(basename "$(readlink "$d/driver" 2>/dev/null || echo none)")
    printf '  %s vendor=%s device=%s driver=%s runtime=%s\n' "$(basename "$d")" \
        "$(rd "$d/vendor")" "$(rd "$d/device")" "$drv" "$(rd "$d/power/runtime_status")"
done

echo "== Platform profile"
printf '  current=%s choices=%s\n' "$(rd /sys/firmware/acpi/platform_profile)" \
    "$(rd /sys/firmware/acpi/platform_profile_choices)"
for p in /sys/class/platform-profile/*; do
    [[ -e "$p" ]] && printf '  handler %s: %s\n' "$(rd "$p/name")" "$(rd "$p/profile")"
done

echo "== Lenovo WMI / ACPI drivers"
for d in /sys/bus/wmi/devices/*; do
    drv=$(basename "$(readlink "$d/driver" 2>/dev/null || echo)")
    [[ -n "$drv" ]] && printf '  %s -> %s\n' "$(rd "$d/guid")" "$drv"
done
[[ -d /sys/bus/platform/devices/VPC2004:00 ]] && echo "  ideapad-laptop VPC2004:00 present"

echo "== hwmon"
for h in /sys/class/hwmon/hwmon*; do printf '  %s %s\n' "$(basename "$h")" "$(rd "$h/name")"; done

echo "== Battery charge controls"
for b in /sys/class/power_supply/BAT*; do
    [[ -e "$b" ]] || continue
    printf '  %s capacity=%s status=%s charge_types=%s end_threshold=%s\n' "$(basename "$b")" \
        "$(rd "$b/capacity")" "$(rd "$b/status")" "$(rd "$b/charge_types")" \
        "$(rd "$b/charge_control_end_threshold")"
done
printf '  ideapad conservation_mode=%s\n' "$(rd /sys/bus/platform/devices/VPC2004:00/conservation_mode)"
