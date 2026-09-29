//! Read-only client for power-profiles-daemon.
//!
//! Only properties are read (`ActiveProfile`, `Profiles`,
//! `PerformanceDegraded`); no methods are called.

use std::collections::HashMap;

use legion_core::profile::{PpdInfo, PpdProfile};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

/// (bus name, object path, interface) pairs, newest first.
const ENDPOINTS: [(&str, &str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
        "org.freedesktop.UPower.PowerProfiles",
    ),
    (
        "net.hadess.PowerProfiles",
        "/net/hadess/PowerProfiles",
        "net.hadess.PowerProfiles",
    ),
];

fn string_of(v: Option<&OwnedValue>) -> Option<String> {
    v.and_then(|v| <&str>::try_from(&**v).ok())
        .map(str::to_owned)
}

fn read_from(conn: &Connection, (name, path, iface): (&str, &str, &str)) -> zbus::Result<PpdInfo> {
    let proxy = Proxy::new(conn, name, path, iface)?;
    let active: String = proxy.get_property("ActiveProfile")?;
    let raw: Vec<HashMap<String, OwnedValue>> = proxy.get_property("Profiles")?;
    let degraded: Option<String> = proxy
        .get_property::<String>("PerformanceDegraded")
        .ok()
        .filter(|s| !s.is_empty());
    let profiles = raw
        .iter()
        .filter_map(|m| {
            Some(PpdProfile {
                profile: string_of(m.get("Profile"))?,
                driver: string_of(m.get("Driver")),
            })
        })
        .collect();
    Ok(PpdInfo {
        bus_name: name.to_owned(),
        active,
        profiles,
        performance_degraded: degraded,
    })
}

/// Reads power-profiles-daemon state from the system bus.
pub fn read() -> Result<PpdInfo, String> {
    let conn = Connection::system().map_err(|e| format!("system bus unavailable: {e}"))?;
    let mut last = String::from("not running");
    for ep in ENDPOINTS {
        match read_from(&conn, ep) {
            Ok(info) => return Ok(info),
            Err(e) => {
                tracing::debug!(bus = ep.0, error = %e, "power-profiles-daemon endpoint failed");
                last = match e {
                    zbus::Error::MethodError(n, _, _)
                        if n.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
                    {
                        "not running".into()
                    }
                    other => other.to_string(),
                };
            }
        }
    }
    Err(last)
}

/// Asks power-profiles-daemon to switch profile (`power-saver`,
/// `balanced`, `performance`). power-profiles-daemon performs its own polkit
/// check (`org.freedesktop.UPower.PowerProfiles.switch-profile`).
pub fn set_active(profile: &str) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system bus unavailable: {e}"))?;
    let current = read()?;
    if !current.profiles.iter().any(|p| p.profile == profile) {
        return Err(format!("power-profiles-daemon does not offer {profile:?}"));
    }
    let ep = ENDPOINTS
        .iter()
        .find(|e| e.0 == current.bus_name)
        .copied()
        .unwrap_or(ENDPOINTS[0]);
    let proxy = Proxy::new(&conn, ep.0, ep.1, ep.2).map_err(|e| e.to_string())?;
    proxy
        .set_property("ActiveProfile", profile)
        .map_err(|e| format!("power-profiles-daemon refused: {e}"))?;
    tracing::info!(target: "legion_audit", profile, "power-profiles-daemon profile changed");
    Ok(())
}

/// Returns `true` if a bus name currently has an owner on the system bus.
pub fn name_has_owner(name: &str) -> Result<bool, String> {
    let conn = Connection::system().map_err(|e| format!("system bus unavailable: {e}"))?;
    let proxy = zbus::blocking::fdo::DBusProxy::new(&conn).map_err(|e| e.to_string())?;
    let bus = zbus::names::BusName::try_from(name).map_err(|e| e.to_string())?;
    proxy.name_has_owner(bus).map_err(|e| e.to_string())
}
