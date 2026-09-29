//! RAM detection from `/proc/meminfo`.

use legion_core::model::MemoryInfo;
use legion_core::sysfs::SysRoot;

/// Parses `/proc/meminfo` (values in kB).
pub fn parse_meminfo(text: &str) -> Option<MemoryInfo> {
    let get = |key: &str| -> Option<u64> {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse().ok())
    };
    let total = get("MemTotal")?;
    let available = get("MemAvailable").or_else(|| {
        Some(get("MemFree")? + get("Buffers").unwrap_or(0) + get("Cached").unwrap_or(0))
    })?;
    Some(MemoryInfo {
        total_mib: total / 1024,
        available_mib: available / 1024,
        used_mib: total.saturating_sub(available) / 1024,
        swap_total_mib: get("SwapTotal").unwrap_or(0) / 1024,
        swap_free_mib: get("SwapFree").unwrap_or(0) / 1024,
    })
}

/// Reads memory information.
pub fn read_memory(root: &SysRoot) -> Option<MemoryInfo> {
    root.read_opt("/proc/meminfo")
        .and_then(|t| parse_meminfo(&t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo() {
        let t = "MemTotal:       15774724 kB\nMemFree:         1671664 kB\nMemAvailable:    4278388 kB\n\
                 SwapTotal:      15773692 kB\nSwapFree:        6250000 kB\n";
        let m = parse_meminfo(t).unwrap();
        assert_eq!(m.total_mib, 15405);
        assert_eq!(m.available_mib, 4178);
        assert_eq!(m.used_mib, 11226);
        assert_eq!(m.swap_total_mib, 15403);
    }

    #[test]
    fn fallback_without_memavailable() {
        let m = parse_meminfo("MemTotal: 2048 kB\nMemFree: 512 kB\nCached: 512 kB\n").unwrap();
        assert_eq!(m.available_mib, 1);
        assert!(parse_meminfo("garbage").is_none());
    }
}
