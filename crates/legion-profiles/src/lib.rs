//! Power profile detection and custom profile definitions.
//!
//! Detection order follows the project rules:
//!
//! 1. power-profiles-daemon (D-Bus `org.freedesktop.UPower.PowerProfiles`,
//!    falling back to the legacy `net.hadess.PowerProfiles` name);
//! 2. kernel `platform_profile` (`/sys/firmware/acpi/platform_profile`,
//!    plus the per-driver handlers under `/sys/class/platform-profile`).
//!
//! Changing profiles (Phase 2) follows the same order: power-profiles-daemon
//! for the three profiles it knows (`power-saver`, `balanced`,
//! `performance`), otherwise a direct write of the raw `platform_profile`
//! value — only values listed in `platform_profile_choices` are accepted.

use legion_core::profile::{
    PlatformProfileHandler, PlatformProfileInfo, PowerProfile, PowerProfileState, PpdInfo,
    ProfileBackend, build_mapping,
};
use legion_core::sysfs::{self, SysRoot};

pub mod custom;
pub mod ppd;
pub mod store;

pub use custom::{CustomProfile, ProfileConstraints};

const PLATFORM_PROFILE: &str = "/sys/firmware/acpi/platform_profile";
const PLATFORM_PROFILE_CHOICES: &str = "/sys/firmware/acpi/platform_profile_choices";
const PLATFORM_PROFILE_CLASS: &str = "/sys/class/platform-profile";

fn words(s: Option<String>) -> Vec<String> {
    s.map(|s| s.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Reads the kernel `platform_profile` interface.
pub fn read_platform_profile(root: &SysRoot) -> Option<PlatformProfileInfo> {
    let legacy_raw = root.read_opt(PLATFORM_PROFILE);
    let mut choices = words(root.read_opt(PLATFORM_PROFILE_CHOICES));
    let handlers: Vec<PlatformProfileHandler> = root
        .list(PLATFORM_PROFILE_CLASS)
        .into_iter()
        .map(|id| {
            let dir = format!("{PLATFORM_PROFILE_CLASS}/{id}");
            PlatformProfileHandler {
                name: root.read_opt(format!("{dir}/name")),
                profile: root.read_opt(format!("{dir}/profile")),
                choices: words(root.read_opt(format!("{dir}/choices"))),
                id,
            }
        })
        .collect();
    if legacy_raw.is_none() && choices.is_empty() && handlers.is_empty() {
        return None;
    }
    if choices.is_empty() {
        for h in &handlers {
            for c in &h.choices {
                if !choices.contains(c) {
                    choices.push(c.clone());
                }
            }
        }
    }
    // Same aggregation as the kernel's legacy `platform_profile_show`: the
    // common profile of all handlers, or "custom" when they disagree.
    let handler_profiles: Vec<&str> = handlers
        .iter()
        .filter_map(|h| h.profile.as_deref())
        .collect();
    let current_raw = match handler_profiles.split_first() {
        Some((first, rest)) if rest.iter().all(|p| p == first) => Some((*first).to_owned()),
        Some(_) => Some("custom".to_owned()),
        None => legacy_raw,
    };
    Some(PlatformProfileInfo {
        current: current_raw
            .as_deref()
            .map(PowerProfile::from_platform_profile)
            .unwrap_or(PowerProfile::Unknown),
        mapping: build_mapping(&choices),
        writable_by_root: sysfs::is_writable_mode(&root.path(PLATFORM_PROFILE)),
        current_raw,
        choices,
        handlers,
    })
}

/// Combines platform_profile and power-profiles-daemon state.
///
/// The logical current profile comes from `platform_profile` when present
/// (it distinguishes more modes, e.g. `max-power` and `custom`); otherwise
/// from power-profiles-daemon.
pub fn combine(
    platform: Option<PlatformProfileInfo>,
    ppd: Result<PpdInfo, String>,
) -> PowerProfileState {
    let mut notes = Vec::new();
    let ppd = match ppd {
        Ok(p) => Some(p),
        Err(e) => {
            notes.push(format!("power-profiles-daemon: {e}"));
            None
        }
    };
    let current = platform
        .as_ref()
        .map(|p| p.current)
        .filter(|p| *p != PowerProfile::Unknown)
        .or_else(|| ppd.as_ref().map(|p| PowerProfile::from_ppd(&p.active)))
        .unwrap_or(PowerProfile::Unknown);
    let backend = if ppd.is_some() {
        ProfileBackend::PowerProfilesDaemon
    } else if platform.as_ref().is_some_and(|p| !p.choices.is_empty()) {
        ProfileBackend::PlatformProfile
    } else {
        ProfileBackend::None
    };
    if let (Some(pp), Some(d)) = (&platform, &ppd) {
        let expected = PowerProfile::from_ppd(&d.active);
        if expected != PowerProfile::Unknown && expected != pp.current {
            notes.push(format!(
                "power-profiles-daemon reports {:?} while platform_profile is {:?}; \
                 the firmware mode was probably changed outside power-profiles-daemon \
                 (e.g. Fn+Q)",
                d.active,
                pp.current_raw.as_deref().unwrap_or("?")
            ));
        }
    }
    PowerProfileState {
        current,
        backend,
        platform_profile: platform,
        ppd,
        notes,
    }
}

/// How a profile change will be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileAction {
    /// Ask power-profiles-daemon (keeps its state consistent).
    Ppd(String),
    /// Write the raw value to `/sys/firmware/acpi/platform_profile`.
    PlatformProfile(String),
}

/// power-profiles-daemon name for a logical profile.
pub fn ppd_name(p: PowerProfile) -> Option<&'static str> {
    match p {
        PowerProfile::Quiet => Some("power-saver"),
        PowerProfile::Balanced => Some("balanced"),
        PowerProfile::Performance => Some("performance"),
        _ => None,
    }
}

/// Decides how to switch to `target`, validating that the hardware offers it.
pub fn plan_change(
    state: &PowerProfileState,
    target: PowerProfile,
) -> legion_core::Result<ProfileAction> {
    if let (Some(ppd), Some(name)) = (&state.ppd, ppd_name(target)) {
        if ppd.profiles.iter().any(|p| p.profile == name) {
            return Ok(ProfileAction::Ppd(name.to_owned()));
        }
    }
    state
        .platform_profile
        .as_ref()
        .and_then(|pp| pp.mapping.iter().find(|m| m.logical == target))
        .map(|m| ProfileAction::PlatformProfile(m.raw.clone()))
        .ok_or_else(|| {
            legion_core::HwError::NotSupported(format!("profile {target} on this hardware"))
        })
}

/// Writes a raw `platform_profile` value after checking it is offered.
///
/// When the kernel exposes per-driver handlers (`/sys/class/platform-profile`,
/// `Documentation/ABI/testing/sysfs-class-platform-profile`), the value is
/// written to every handler that offers it. This is required for `custom`:
/// the legacy `/sys/firmware/acpi/platform_profile` rejects it (there it only
/// means "handlers disagree"). Without handlers, the legacy file is used.
pub fn set_platform_profile(root: &SysRoot, raw: &str) -> legion_core::Result<()> {
    let handlers: Vec<String> = root
        .list(PLATFORM_PROFILE_CLASS)
        .into_iter()
        .map(|id| format!("{PLATFORM_PROFILE_CLASS}/{id}"))
        .filter(|dir| root.exists(format!("{dir}/profile")))
        .collect();
    if handlers.is_empty() {
        let choices = words(root.read_opt(PLATFORM_PROFILE_CHOICES));
        if !choices.iter().any(|c| c == raw) || raw == "custom" {
            return Err(legion_core::HwError::InvalidValue(format!(
                "platform profile {raw:?} is not one of: {}",
                choices
                    .iter()
                    .filter(|c| *c != "custom")
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        sysfs::write_attr(root, PLATFORM_PROFILE, raw)?;
        return Ok(());
    }
    let targets: Vec<&String> = handlers
        .iter()
        .filter(|dir| {
            words(root.read_opt(format!("{dir}/choices")))
                .iter()
                .any(|c| c == raw)
        })
        .collect();
    if targets.is_empty() {
        let mut all: Vec<String> = handlers
            .iter()
            .flat_map(|dir| words(root.read_opt(format!("{dir}/choices"))))
            .collect();
        all.dedup();
        return Err(legion_core::HwError::InvalidValue(format!(
            "platform profile {raw:?} is not one of: {}",
            all.join(", ")
        )));
    }
    for dir in targets {
        sysfs::write_attr(root, &format!("{dir}/profile"), raw)?;
    }
    Ok(())
}

/// Detects the power profile state. `use_dbus = false` skips
/// power-profiles-daemon (used with fixture roots).
pub fn detect(root: &SysRoot, use_dbus: bool) -> PowerProfileState {
    let ppd = if use_dbus {
        ppd::read()
    } else {
        Err("D-Bus disabled".into())
    };
    combine(read_platform_profile(root), ppd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use legion_core::profile::PpdProfile;
    use legion_core::testing::FakeSysfs;

    fn legion() -> FakeSysfs {
        let fs = FakeSysfs::new();
        fs.file("/sys/firmware/acpi/platform_profile", "performance\n")
            .file(
                "/sys/firmware/acpi/platform_profile_choices",
                "low-power balanced performance max-power custom\n",
            )
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
                "low-power balanced performance max-power custom",
            );
        fs
    }

    #[test]
    fn reads_platform_profile() {
        let fs = legion();
        let p = read_platform_profile(&fs.root()).unwrap();
        assert_eq!(p.current, PowerProfile::Performance);
        assert_eq!(p.choices.len(), 5);
        assert_eq!(p.handlers[0].name.as_deref(), Some("lenovo-wmi-gamezone"));
        assert_eq!(p.mapping[0].raw, "low-power");
    }

    #[test]
    fn missing_platform_profile() {
        let fs = FakeSysfs::new();
        assert!(read_platform_profile(&fs.root()).is_none());
        let st = detect(&fs.root(), false);
        assert_eq!(st.current, PowerProfile::Unknown);
        assert_eq!(st.backend, ProfileBackend::None);
        assert_eq!(st.notes.len(), 1);
    }

    fn ppd(active: &str) -> PpdInfo {
        PpdInfo {
            bus_name: "org.freedesktop.UPower.PowerProfiles".into(),
            active: active.into(),
            profiles: ["power-saver", "balanced", "performance"]
                .iter()
                .map(|p| PpdProfile {
                    profile: (*p).into(),
                    driver: Some("multiple".into()),
                })
                .collect(),
            performance_degraded: None,
        }
    }

    #[test]
    fn ppd_is_preferred_backend() {
        let fs = legion();
        let st = combine(read_platform_profile(&fs.root()), Ok(ppd("performance")));
        assert_eq!(st.backend, ProfileBackend::PowerProfilesDaemon);
        assert_eq!(st.current, PowerProfile::Performance);
        assert!(st.notes.is_empty());
    }

    #[test]
    fn mismatch_is_reported() {
        let fs = legion();
        let st = combine(read_platform_profile(&fs.root()), Ok(ppd("power-saver")));
        assert_eq!(st.current, PowerProfile::Performance);
        assert_eq!(st.notes.len(), 1);
    }

    #[test]
    fn plans_changes() {
        let fs = legion();
        let with_ppd = combine(read_platform_profile(&fs.root()), Ok(ppd("performance")));
        assert_eq!(
            plan_change(&with_ppd, PowerProfile::Quiet).unwrap(),
            ProfileAction::Ppd("power-saver".into())
        );
        // max-power is unknown to ppd → direct platform_profile
        assert_eq!(
            plan_change(&with_ppd, PowerProfile::MaxPower).unwrap(),
            ProfileAction::PlatformProfile("max-power".into())
        );
        let without = combine(read_platform_profile(&fs.root()), Err("x".into()));
        assert_eq!(
            plan_change(&without, PowerProfile::Quiet).unwrap(),
            ProfileAction::PlatformProfile("low-power".into())
        );
        assert!(plan_change(&combine(None, Err("x".into())), PowerProfile::Quiet).is_err());
    }

    #[test]
    fn writes_platform_profile() {
        let fs = legion();
        let root = fs.root();
        set_platform_profile(&root, "custom").unwrap();
        assert_eq!(
            read_platform_profile(&root).unwrap().current,
            PowerProfile::Custom
        );
        assert!(set_platform_profile(&root, "quiet").is_err());
        assert!(set_platform_profile(&root, "../x").is_err());
    }

    #[test]
    fn custom_goes_through_the_handler() {
        let fs = legion();
        let root = fs.root();
        set_platform_profile(&root, "custom").unwrap();
        // the legacy file is untouched (the kernel rejects "custom" there)
        assert_eq!(
            root.read("/sys/firmware/acpi/platform_profile").unwrap(),
            "performance"
        );
        assert_eq!(
            root.read("/sys/class/platform-profile/platform-profile-0/profile")
                .unwrap(),
            "custom"
        );
        assert_eq!(
            read_platform_profile(&root).unwrap().current,
            PowerProfile::Custom
        );
    }

    #[test]
    fn legacy_only_kernel() {
        let fs = FakeSysfs::new();
        fs.file("/sys/firmware/acpi/platform_profile", "balanced")
            .file(
                "/sys/firmware/acpi/platform_profile_choices",
                "low-power balanced performance custom",
            );
        let root = fs.root();
        set_platform_profile(&root, "low-power").unwrap();
        assert_eq!(
            read_platform_profile(&root).unwrap().current,
            PowerProfile::Quiet
        );
        assert!(set_platform_profile(&root, "custom").is_err());
    }

    #[test]
    fn disagreeing_handlers_read_as_custom() {
        let fs = legion();
        fs.file(
            "/sys/class/platform-profile/platform-profile-1/profile",
            "balanced",
        )
        .file(
            "/sys/class/platform-profile/platform-profile-1/choices",
            "balanced performance",
        );
        assert_eq!(
            read_platform_profile(&fs.root()).unwrap().current,
            PowerProfile::Custom
        );
    }

    #[test]
    fn ppd_only() {
        let st = combine(None, Ok(ppd("power-saver")));
        assert_eq!(st.current, PowerProfile::Quiet);
    }
}
