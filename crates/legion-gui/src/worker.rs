//! Background thread talking to `legion-control-daemon` over D-Bus.
//!
//! The GUI never touches `/sys`: everything goes through this worker, which
//! polls the daemon's cached status every second and executes user
//! commands. Results are sent to the GTK main loop through a channel.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use legion_core::PowerProfile;
use legion_core::capabilities::Capability;
use legion_core::keyboard::{KeyboardInfo, Rgb};
use legion_core::model::{DaemonInfo, FanMode, PowerLimits, Status};
use legion_dbus::{Bus, Client};
use legion_profiles::CustomProfile;
use legion_profiles::store::ProfileStore;

/// Commands from the UI.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Switch power profile.
    Profile(PowerProfile),
    /// Battery conservation.
    Conservation(bool),
    /// Charge type.
    ChargeType(String),
    /// Fan mode.
    Fan(FanMode),
    /// Firmware limits applied together in the custom profile.
    ApplyLimits(std::collections::BTreeMap<String, i64>),
    /// Toggle an experimental feature in the daemon configuration.
    SetExperimental(String, bool),
    /// EPP.
    Epp(String),
    /// Governor.
    Governor(String),
    /// Turbo.
    Turbo(bool),
    /// Save current settings as a named profile.
    SaveProfile(String),
    /// Apply a saved profile.
    ApplyProfile(String),
    /// Delete a saved profile.
    DeleteProfile(String),
    /// Keyboard colour.
    KeyboardColor(Rgb),
    /// Keyboard brightness level.
    KeyboardBrightness(u8),
    /// Restore the keyboard effect saved before the first change.
    KeyboardRestore,
    /// The keyboard page appeared / disappeared: only query the keyboard
    /// controller while it is on screen.
    KeyboardVisible(bool),
}

/// Everything the UI shows.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Daemon status.
    pub status: Status,
    /// Daemon state.
    pub info: DaemonInfo,
    /// Capabilities (refreshed every 30 s).
    pub caps: Vec<Capability>,
    /// Firmware limits.
    pub limits: PowerLimits,
    /// Saved profiles.
    pub profiles: Vec<String>,
}

/// Messages to the UI.
#[derive(Debug, Clone)]
pub enum Msg {
    /// New data.
    Update(Box<Snapshot>),
    /// Daemon unreachable.
    DaemonDown(String),
    /// A command succeeded.
    Done(String),
    /// A command failed.
    Failed(String),
    /// A command started and may wait for authentication.
    Busy(bool),
    /// Keyboard backlight state (or why it is not available).
    Keyboard(Box<Result<KeyboardInfo, String>>),
}

struct State {
    client: Option<Client>,
    caps: Vec<Capability>,
    caps_at: Option<Instant>,
    ticks: u64,
    limits: PowerLimits,
    limits_at: Option<Instant>,
    last_connect: Option<Instant>,
    keyboard_visible: bool,
    keyboard_at: Option<Instant>,
}

/// How often the keyboard state is refreshed while its page is visible.
const KEYBOARD_INTERVAL: Duration = Duration::from_secs(5);

fn describe(e: legion_core::HwError) -> String {
    match e {
        legion_core::HwError::NotSupported(m) => m,
        legion_core::HwError::InvalidValue(m) => m,
        legion_core::HwError::Backend { message, .. } => message,
        other => other.to_string(),
    }
}

/// Runs the worker loop (blocking; call from a dedicated thread).
pub fn run(bus: Bus, rx: Receiver<Cmd>, tx: async_channel::Sender<Msg>) {
    let mut st = State {
        client: None,
        caps: vec![],
        caps_at: None,
        ticks: 0,
        limits: PowerLimits::default(),
        limits_at: None,
        last_connect: None,
        keyboard_visible: false,
        keyboard_at: None,
    };
    let store = ProfileStore::user().ok();
    loop {
        let cmd = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if st.client.is_none()
            && st
                .last_connect
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
        {
            st.last_connect = Some(Instant::now());
            match Client::connect(bus) {
                Ok(c) if c.is_running() => st.client = Some(c),
                Ok(_) => {
                    let _ = tx.send_blocking(Msg::DaemonDown(
                        "legion-control-daemon is not running".into(),
                    ));
                }
                Err(e) => {
                    let _ = tx.send_blocking(Msg::DaemonDown(describe(e)));
                }
            }
        }
        let Some(client) = st.client.take() else {
            if cmd.is_some() {
                let _ =
                    tx.send_blocking(Msg::Failed("legion-control-daemon is not running".into()));
            }
            continue;
        };
        if let Some(cmd) = cmd {
            match &cmd {
                Cmd::KeyboardVisible(v) => {
                    st.keyboard_visible = *v;
                    st.keyboard_at = None; // refresh as soon as it appears
                }
                // anything that may have changed the keyboard
                Cmd::KeyboardColor(_)
                | Cmd::KeyboardBrightness(_)
                | Cmd::KeyboardRestore
                | Cmd::SetExperimental(..) => st.keyboard_at = None,
                _ => {}
            }
            let _ = tx.send_blocking(Msg::Busy(true));
            let res = execute(&client, store.as_ref(), cmd);
            let _ = tx.send_blocking(Msg::Busy(false));
            match res {
                Ok(Some(m)) => {
                    st.limits_at = None;
                    let _ = tx.send_blocking(Msg::Done(m));
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = tx.send_blocking(Msg::Failed(e));
                }
            }
        }
        if st.keyboard_visible
            && st
                .keyboard_at
                .is_none_or(|t| t.elapsed() >= KEYBOARD_INTERVAL)
        {
            st.keyboard_at = Some(Instant::now());
            let info = client.get(|p| p.get_keyboard_info()).map_err(describe);
            let _ = tx.send_blocking(Msg::Keyboard(Box::new(info)));
        }
        match snapshot(&client, &mut st, store.as_ref()) {
            Ok(s) => {
                st.client = Some(client);
                if tx.send_blocking(Msg::Update(Box::new(s))).is_err() {
                    return;
                }
            }
            Err(e) => {
                // dropped: reconnect on the next iteration
                let _ = tx.send_blocking(Msg::DaemonDown(e));
            }
        }
    }
}

fn snapshot(
    client: &Client,
    st: &mut State,
    store: Option<&ProfileStore>,
) -> Result<Snapshot, String> {
    let status: Status = client.get(|p| p.get_status()).map_err(describe)?;
    let info: DaemonInfo = client.get(|p| p.get_daemon_info()).map_err(describe)?;
    st.ticks += 1;
    // capabilities are slow to compute: skip them on the very first snapshot
    // so the window fills immediately
    if st.ticks > 1
        && st
            .caps_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(30))
    {
        st.caps = client.get(|p| p.get_capabilities()).unwrap_or_default();
        st.caps_at = Some(Instant::now());
    }
    if st
        .limits_at
        .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
    {
        st.limits = client.get(|p| p.get_power_limits()).unwrap_or_default();
        st.limits_at = Some(Instant::now());
    }
    Ok(Snapshot {
        status,
        info,
        caps: st.caps.clone(),
        limits: st.limits.clone(),
        profiles: store.map(|s| s.list()).unwrap_or_default(),
    })
}

fn execute(
    client: &Client,
    store: Option<&ProfileStore>,
    cmd: Cmd,
) -> Result<Option<String>, String> {
    let call = |f: &dyn Fn(
        &legion_dbus::ManagerProxyBlocking<'static>,
    ) -> legion_dbus::zbus::Result<()>| { client.call(|p| f(p)).map_err(describe) };
    match cmd {
        Cmd::Profile(p) => {
            call(&|x| x.set_profile(p.as_str()))?;
            Ok(Some(format!("Profile: {p}")))
        }
        Cmd::Conservation(on) => {
            call(&|x| x.set_battery_conservation(on))?;
            Ok(Some(format!(
                "Conservation mode {}",
                if on { "on" } else { "off" }
            )))
        }
        Cmd::ChargeType(t) => {
            call(&|x| x.set_charge_type(&t))?;
            Ok(Some(format!("Charge type: {t}")))
        }
        Cmd::Fan(m) => {
            let json = serde_json::to_string(&m).map_err(|e| e.to_string())?;
            call(&|x| x.set_fan_mode(&json))?;
            Ok(Some("Fan mode updated".into()))
        }
        Cmd::ApplyLimits(limits) => {
            let n = limits.len();
            let profile = CustomProfile {
                platform_profile: Some("custom".into()),
                power_limits: limits,
                ..Default::default()
            };
            let json = serde_json::to_string(&profile).map_err(|e| e.to_string())?;
            let _: Vec<String> = client.get(|x| x.apply_profile(&json)).map_err(describe)?;
            Ok(Some(format!("Applied {n} power settings")))
        }
        Cmd::SetExperimental(name, on) => {
            call(&|x| x.set_experimental(&name, on))?;
            Ok(Some(format!(
                "{name} {}",
                if on { "enabled" } else { "disabled" }
            )))
        }
        Cmd::Epp(v) => {
            call(&|x| x.set_cpu_epp(&v))?;
            Ok(Some(format!("EPP: {v}")))
        }
        Cmd::Governor(v) => {
            call(&|x| x.set_cpu_governor(&v))?;
            Ok(Some(format!("Governor: {v}")))
        }
        Cmd::Turbo(on) => {
            call(&|x| x.set_turbo(on))?;
            Ok(Some(format!("Turbo {}", if on { "on" } else { "off" })))
        }
        Cmd::KeyboardColor(c) => {
            call(&|x| x.set_keyboard_color(&c.to_string()))?;
            Ok(Some(format!("Keyboard colour set to {c}")))
        }
        Cmd::KeyboardBrightness(level) => {
            call(&|x| x.set_keyboard_brightness(level))?;
            Ok(Some(format!("Keyboard brightness set to {level}")))
        }
        Cmd::KeyboardRestore => {
            call(&|x| x.restore_keyboard())?;
            Ok(Some("Keyboard effect restored".into()))
        }
        // handled by the loop, not an action on the daemon
        Cmd::KeyboardVisible(_) => Ok(None),
        Cmd::SaveProfile(name) => {
            let store = store.ok_or("cannot determine the profile directory")?;
            let p: CustomProfile = client.get(|x| x.capture_profile()).map_err(describe)?;
            store.save(&name, &p).map_err(describe)?;
            Ok(Some(format!("Saved profile {name}")))
        }
        Cmd::ApplyProfile(name) => {
            let store = store.ok_or("cannot determine the profile directory")?;
            let p = store.load(&name).map_err(describe)?;
            let json = serde_json::to_string(&p).map_err(|e| e.to_string())?;
            let changes: Vec<String> = client.get(|x| x.apply_profile(&json)).map_err(describe)?;
            Ok(Some(format!("Applied {name} ({} changes)", changes.len())))
        }
        Cmd::DeleteProfile(name) => {
            let store = store.ok_or("cannot determine the profile directory")?;
            store.delete(&name).map_err(describe)?;
            Ok(Some(format!("Deleted profile {name}")))
        }
    }
}

/// Spawns the worker thread. Returns the command sender.
pub fn spawn(bus: Bus, tx: async_channel::Sender<Msg>) -> Sender<Cmd> {
    let (ctx, crx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("legion-dbus".into())
        .spawn(move || run(bus, crx, tx))
        .expect("spawn D-Bus worker");
    ctx
}
