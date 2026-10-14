//! `com.legion.Control.Manager` D-Bus interface.

use serde::Serialize;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, fdo, interface};

use legion_core::HardwareController;
use legion_core::keyboard::Rgb;
use legion_core::model::FanMode;
use legion_dbus::{actions, to_fdo};
use legion_profiles::CustomProfile;

use crate::polkit::{Policy, authorize};
use crate::worker::{Events, Hw};

/// Interface implementation.
pub struct Manager {
    hw: Hw,
    policy: Policy,
}

fn json<T: Serialize>(v: &T) -> fdo::Result<String> {
    serde_json::to_string(v).map_err(|e| fdo::Error::Failed(e.to_string()))
}

impl Manager {
    /// Creates the interface.
    pub fn new(hw: Hw, policy: Policy) -> Self {
        Self { hw, policy }
    }

    async fn auth(&self, conn: &Connection, hdr: &Header<'_>, action: &str) -> fdo::Result<()> {
        let sender = hdr.sender().map(|s| s.as_str().to_owned());
        authorize(self.policy, conn, sender.as_deref(), action).await
    }

    /// Emits the signals for `ev`.
    pub async fn emit(emitter: &SignalEmitter<'_>, ev: &Events) {
        if let Some(p) = &ev.profile {
            let _ = Self::profile_changed(emitter, p).await;
        }
        if let Some((cpu, gpu)) = ev.temperature {
            let j = serde_json::json!({ "cpu_c": cpu, "gpu_c": gpu }).to_string();
            let _ = Self::temperature_changed(emitter, &j).await;
        }
        if let Some(b) = &ev.battery {
            if let Ok(j) = serde_json::to_string(b) {
                let _ = Self::battery_changed(emitter, &j).await;
            }
        }
        if ev.hardware {
            let _ = Self::hardware_changed(emitter).await;
        }
    }

    /// Runs a setter on the worker, then refreshes and emits signals.
    async fn change(
        &self,
        emitter: &SignalEmitter<'_>,
        f: impl FnOnce(&legion_hardware::LinuxController) -> legion_core::Result<()> + Send + 'static,
    ) -> fdo::Result<()> {
        let ev = self
            .hw
            .run(move |w| {
                f(w.ctrl())?;
                Ok::<_, legion_core::HwError>(w.after_change())
            })
            .await
            .map_err(to_fdo)?;
        Self::emit(emitter, &ev).await;
        Ok(())
    }
}

#[interface(name = "com.legion.Control.Manager")]
impl Manager {
    async fn get_daemon_info(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.info()).await)
    }

    async fn get_hardware_info(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.system()).await)
    }

    async fn get_capabilities(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.capabilities()).await)
    }

    async fn get_profile(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.status().profile).await)
    }

    async fn get_cpu_info(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.status().cpu).await)
    }

    async fn get_gpu_info(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.status().gpu).await)
    }

    async fn get_battery_info(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.status().battery).await)
    }

    async fn get_thermal_info(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.status().thermal).await)
    }

    async fn get_fan_info(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.status().fans).await)
    }

    async fn get_status(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.status()).await)
    }

    async fn get_power_limits(&self) -> fdo::Result<String> {
        let pl = self
            .hw
            .run(|w| w.ctrl().get_power_limits())
            .await
            .map_err(to_fdo)?;
        json(&pl)
    }

    async fn get_keyboard_info(&self) -> fdo::Result<String> {
        let info = self
            .hw
            .run(|w| w.ctrl().get_keyboard_info())
            .await
            .map_err(to_fdo)?;
        json(&info)
    }

    async fn set_profile(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        profile: String,
    ) -> fdo::Result<()> {
        let p: legion_core::PowerProfile = profile.parse().map_err(to_fdo)?;
        self.auth(conn, &hdr, actions::PROFILE).await?;
        self.change(&emitter, move |c| c.set_power_profile(p)).await
    }

    async fn set_battery_conservation(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        enabled: bool,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::BATTERY).await?;
        self.change(&emitter, move |c| c.set_battery_conservation(enabled))
            .await
    }

    async fn set_charge_type(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        charge_type: String,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::BATTERY).await?;
        self.change(&emitter, move |c| c.set_charge_type(&charge_type))
            .await
    }

    async fn set_charge_threshold(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        end: u8,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::BATTERY).await?;
        self.change(&emitter, move |c| c.set_charge_threshold(end))
            .await
    }

    async fn set_keyboard_color(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        color: String,
    ) -> fdo::Result<()> {
        let color: Rgb = color.parse().map_err(to_fdo)?;
        self.auth(conn, &hdr, actions::KEYBOARD).await?;
        self.change(&emitter, move |c| c.set_keyboard_color(color))
            .await
    }

    async fn set_keyboard_brightness(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        level: u8,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::KEYBOARD).await?;
        self.change(&emitter, move |c| c.set_keyboard_brightness(level))
            .await
    }

    async fn restore_keyboard(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::KEYBOARD).await?;
        self.change(&emitter, |c| c.restore_keyboard()).await
    }

    async fn set_fan_mode(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        mode_json: String,
    ) -> fdo::Result<()> {
        let mode: FanMode = serde_json::from_str(&mode_json)
            .map_err(|e| fdo::Error::InvalidArgs(format!("invalid fan mode: {e}")))?;
        // returning to automatic control never needs authorisation
        if mode != FanMode::Auto {
            self.auth(conn, &hdr, actions::FAN).await?;
        }
        let ev = self
            .hw
            .run(move |w| w.set_fan_mode(mode))
            .await
            .map_err(to_fdo)?;
        Self::emit(&emitter, &ev).await;
        Ok(())
    }

    async fn set_firmware_attribute(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        name: String,
        value: i64,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::POWER_LIMITS).await?;
        self.change(&emitter, move |c| c.set_firmware_attribute(&name, value))
            .await
    }

    async fn set_cpu_epp(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        epp: String,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::CPU).await?;
        self.change(&emitter, move |c| c.set_cpu_epp(&epp)).await
    }

    async fn set_cpu_governor(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        governor: String,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::CPU).await?;
        self.change(&emitter, move |c| c.set_cpu_governor(&governor))
            .await
    }

    async fn set_turbo(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        enabled: bool,
    ) -> fdo::Result<()> {
        self.auth(conn, &hdr, actions::CPU).await?;
        self.change(&emitter, move |c| c.set_turbo(enabled)).await
    }

    async fn apply_profile(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        profile_json: String,
    ) -> fdo::Result<String> {
        let p = CustomProfile::from_json(&profile_json).map_err(to_fdo)?;
        // authorise every category the profile touches
        if p.platform_profile.is_some() {
            self.auth(conn, &hdr, actions::PROFILE).await?;
        }
        if !p.power_limits.is_empty() {
            self.auth(conn, &hdr, actions::POWER_LIMITS).await?;
        }
        if p.cpu_epp.is_some() || p.cpu_governor.is_some() || p.cpu_turbo.is_some() {
            self.auth(conn, &hdr, actions::CPU).await?;
        }
        if p.battery_conservation.is_some()
            || p.charge_type.is_some()
            || p.battery_threshold.is_some()
        {
            self.auth(conn, &hdr, actions::BATTERY).await?;
        }
        if p.fan_mode.as_ref().is_some_and(|m| *m != FanMode::Auto) {
            self.auth(conn, &hdr, actions::FAN).await?;
        }
        let (done, ev) = self
            .hw
            .run(move |w| w.apply_profile(&p))
            .await
            .map_err(to_fdo)?;
        Self::emit(&emitter, &ev).await;
        json(&done)
    }

    async fn set_experimental(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        feature: String,
        enabled: bool,
    ) -> fdo::Result<()> {
        if !legion_hardware::Config::FLAGS.contains(&feature.as_str()) {
            return Err(fdo::Error::InvalidArgs(format!(
                "unknown setting {feature:?}"
            )));
        }
        self.auth(conn, &hdr, actions::CONFIGURE).await?;
        let ev = self
            .hw
            .run(move |w| w.set_experimental(&feature, enabled))
            .await
            .map_err(to_fdo)?;
        Self::emit(&emitter, &ev).await;
        Ok(())
    }

    async fn capture_profile(&self) -> fdo::Result<String> {
        json(&self.hw.run(|w| w.capture_profile()).await)
    }

    #[zbus(signal)]
    async fn profile_changed(emitter: &SignalEmitter<'_>, profile: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn temperature_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn battery_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn hardware_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
