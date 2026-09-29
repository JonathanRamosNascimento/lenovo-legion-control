#!/usr/bin/env bash
# Collects a debug bundle for bug reports. Read-only with respect to the
# hardware; writes only into the output directory.
#
# Usage: scripts/collect-debug.sh [output-dir]
#
# The bundle contains no serial numbers, UUIDs or MAC addresses, but please
# review it before sharing.
set -euo pipefail

out="${1:-legion-debug-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$out"
here="$(cd "$(dirname "$0")" && pwd)"
legionctl="${LEGIONCTL:-$(command -v legionctl || echo "$here/../target/release/legionctl")}"

run() { local name=$1; shift; "$@" > "$out/$name" 2>&1 || echo "(exit $?)" >> "$out/$name"; }

if [[ -x "$legionctl" ]]; then
    run doctor.txt "$legionctl" doctor
    run hardware-report.json "$legionctl" hardware-scan -o -
    run status.json "$legionctl" --json status
else
    echo "legionctl not found; build it with 'cargo build --release'" > "$out/legionctl-missing.txt"
fi

run detect-hardware.txt "$here/detect-hardware.sh"
run uname.txt uname -srmv
run os-release.txt cat /etc/os-release
run modules.txt sh -c "lsmod | grep -Ei 'lenovo|ideapad|legion|wmi|nvidia|platform_profile' || true"
run kernel-log.txt sh -c "journalctl -k -b --no-pager 2>/dev/null | grep -Ei 'lenovo|ideapad|legion|wmi|platform_profile|nvidia' | tail -n 300 || true"

echo "Debug bundle written to $out/ — review it before sharing."
