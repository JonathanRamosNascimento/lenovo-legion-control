//! Keyboard backlight (experimental).
//!
//! The built-in keyboard controller stores its lighting in profiles, in its
//! own flash memory, using a **vendor protocol with no public Lenovo
//! documentation** (see [`legion_keyboard::spectrum`] for provenance). Hence:
//!
//! * the whole feature is off until `enable_keyboard_rgb` is set in
//!   `/etc/legion-control/daemon.json`; while off, the vendor interface is
//!   not queried at all (detection is pure sysfs);
//! * the effect that was in a profile is saved to
//!   `/var/lib/legion-control/keyboard-profile-N.hex` **before** it is first
//!   overwritten, and [`HardwareController::restore_keyboard`] writes it back
//!   (restoring is allowed even when the feature has been switched off again);
//! * only the active profile is touched, and nothing is written when the
//!   keyboard already shows the requested colour.

use legion_core::keyboard::{KeyboardInfo, KeyboardState, Rgb};
use legion_core::{HwError, Result, SysRoot};
use legion_keyboard::spectrum::{self, Description};
use legion_keyboard::{Detection, HidNode, Keyboard};

use crate::config::CONFIG_PATH;
use crate::controller::LinuxController;

/// Directory holding the saved effects (owned by the daemon: `StateDirectory`).
pub const STATE_DIR: &str = "/var/lib/legion-control";

fn backup_path(profile: u8) -> String {
    format!("{STATE_DIR}/keyboard-profile-{profile}.hex")
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.is_empty() || s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Reads a saved effect; only data that parses as a profile of the same
/// number is accepted.
fn load_backup(root: &SysRoot, profile: u8) -> Option<Vec<u8>> {
    let raw = from_hex(&root.read_opt(backup_path(profile))?)?;
    let d = Description::decode(&raw).ok()?;
    (d.profile == profile).then_some(raw)
}

/// Saves an effect atomically (mode 0600, directory 0700).
fn save_backup(root: &SysRoot, profile: u8, raw: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let path = root.path(backup_path(profile));
    let io = |e: std::io::Error| HwError::from_io(&path, e);
    let dir = path.parent().expect("backup path has a parent");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io)?;
    let tmp = dir.join(".keyboard-profile.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(io)?;
    f.write_all(to_hex(raw).as_bytes())
        .and_then(|()| f.write_all(b"\n"))
        .and_then(|()| f.sync_all())
        .map_err(io)?;
    std::fs::rename(&tmp, &path).map_err(io)?;
    tracing::info!(target: "legion_audit", profile, path = backup_path(profile), "keyboard effect saved");
    Ok(())
}

fn delete_backup(root: &SysRoot, profile: u8) {
    let _ = std::fs::remove_file(root.path(backup_path(profile)));
}

/// The backup could not be saved, so nothing was written to the keyboard.
fn backup_failed(e: &HwError) -> HwError {
    HwError::Backend {
        backend: "legion-control",
        message: format!(
            "could not save a backup of the keyboard's current effect ({e}); nothing was \
             changed. After an upgrade restart the service: sudo systemctl restart legion-control"
        ),
    }
}

/// What is known about the keyboard without touching the vendor interface.
fn disabled_hint() -> String {
    format!(
        "keyboard lighting control is disabled; set \"enable_keyboard_rgb\": true in {CONFIG_PATH}"
    )
}

impl LinuxController {
    fn keyboard_node(&self) -> Result<HidNode> {
        match legion_keyboard::detect(self.root()) {
            Detection::Found(n) => Ok(*n),
            Detection::Unverified(n) => Err(HwError::NotSupported(format!(
                "the keyboard controller {} is not in the verified list; \
                 see docs/hardware-support.md",
                n.usb_id()
            ))),
            Detection::Rejected(why) => Err(HwError::NotSupported(why)),
            Detection::Absent => Err(HwError::NotSupported(
                "no built-in keyboard backlight controller found".into(),
            )),
        }
    }

    fn keyboard(&self) -> Result<Keyboard> {
        Ok(Keyboard::new(self.keyboard_node()?, self.keyboard_opener()))
    }

    fn require_keyboard_enabled(&self) -> Result<()> {
        if self.options().config.enable_keyboard_rgb {
            Ok(())
        } else {
            Err(HwError::NotSupported(disabled_hint()))
        }
    }

    pub(crate) fn keyboard_info_impl(&self) -> Result<KeyboardInfo> {
        let node = self.keyboard_node()?;
        let enabled = self.options().config.enable_keyboard_rgb;
        let mut info = KeyboardInfo {
            device: node.hid_name.clone(),
            usb_id: node.usb_id(),
            node: node.name.clone(),
            enabled,
            max_brightness: spectrum::MAX_BRIGHTNESS,
            state: None,
        };
        if enabled {
            let snap = Keyboard::new(node, self.keyboard_opener()).snapshot()?;
            let effect = &snap.effect;
            info.state = Some(KeyboardState {
                brightness: snap.brightness,
                profile: snap.profile,
                effect: match effect.effects.as_slice() {
                    [] => "none".into(),
                    [e] => spectrum::effect::name(e.kind).into(),
                    many => format!("{} effects", many.len()),
                },
                color: effect.as_static_color(),
                backup: load_backup(self.root(), snap.profile).is_some(),
            });
        }
        Ok(info)
    }

    pub(crate) fn set_keyboard_color_impl(&self, color: Rgb) -> Result<()> {
        self.require_keyboard_enabled()?;
        let kb = self.keyboard()?;
        let root = self.root().clone();
        kb.set_static_color(color, |profile, previous| {
            // keep the original effect: save when there is none yet, or when
            // the profile holds something other than a colour we may have
            // set (the user changed it elsewhere in the meantime)
            let ours = Description::decode(previous)
                .ok()
                .and_then(|d| d.as_static_color())
                .is_some();
            if !ours || load_backup(&root, profile).is_none() {
                save_backup(&root, profile, previous).map_err(|e| backup_failed(&e))?;
            }
            Ok(())
        })?;
        Ok(())
    }

    pub(crate) fn set_keyboard_brightness_impl(&self, level: u8) -> Result<()> {
        self.require_keyboard_enabled()?;
        self.keyboard()?.set_brightness(level)
    }

    pub(crate) fn restore_keyboard_impl(&self) -> Result<()> {
        // allowed even when the feature was switched off again: it only
        // puts back what the keyboard had
        let kb = self.keyboard()?;
        let profile = kb.active_profile()?;
        let raw = load_backup(self.root(), profile).ok_or_else(|| {
            HwError::InvalidValue(format!(
                "no saved original effect for keyboard profile {profile}"
            ))
        })?;
        kb.restore(&raw)?;
        delete_backup(self.root(), profile);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ControllerOptions;
    use crate::fixtures;
    use legion_core::HardwareController;
    use legion_core::testing::FakeSysfs;
    use legion_keyboard::sim::{SimOpener, SimSpectrum};
    use std::sync::Arc;

    fn controller(fs: &FakeSysfs, enabled: bool) -> (LinuxController, SimSpectrum) {
        let mut o = ControllerOptions::fixture(fs.root());
        o.config.enable_keyboard_rgb = enabled;
        let sim = SimSpectrum::new();
        let c = LinuxController::new(o).with_keyboard_opener(Arc::new(SimOpener(sim.clone())));
        (c, sim)
    }

    #[test]
    fn hex_helpers() {
        assert_eq!(
            from_hex(&to_hex(&[0, 1, 0xab, 0xff])),
            Some(vec![0, 1, 0xab, 0xff])
        );
        for bad in ["", "abc", "zz", "é1", "0g"] {
            assert_eq!(from_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn disabled_means_no_traffic_to_the_device() {
        let fs = fixtures::legion_5_15iax10();
        let (c, sim) = controller(&fs, false);
        let info = c.get_keyboard_info().unwrap();
        assert_eq!(info.usb_id, "048d:c195");
        assert_eq!(info.node, "hidraw5");
        assert!(!info.enabled && info.state.is_none());
        assert!(c.set_keyboard_color(Rgb::new(1, 2, 3)).is_err());
        assert!(c.set_keyboard_brightness(3).is_err());
        assert!(
            sim.log().is_empty(),
            "the vendor interface must not be touched"
        );
    }

    #[test]
    fn colour_with_backup_and_restore() {
        let fs = fixtures::legion_5_15iax10();
        let (c, sim) = controller(&fs, true);
        let st = c.get_keyboard_info().unwrap().state.unwrap();
        assert_eq!((st.brightness, st.profile), (5, 1));
        assert_eq!(
            (st.effect.as_str(), st.color, st.backup),
            ("rainbow-wave", None, false)
        );

        c.set_keyboard_color(Rgb::new(255, 0, 0)).unwrap();
        let st = c.get_keyboard_info().unwrap().state.unwrap();
        assert_eq!(
            (st.effect.as_str(), st.color),
            ("static", Some(Rgb::new(255, 0, 0)))
        );
        assert!(st.backup);
        let saved = fs.root().path(backup_path(1));
        assert!(saved.is_file());
        let original = std::fs::read_to_string(&saved).unwrap();

        // a second colour must not replace the original backup
        c.set_keyboard_color(Rgb::new(0, 0, 255)).unwrap();
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), original);
        assert_eq!(sim.static_color(1), Some(Rgb::new(0, 0, 255)));

        c.restore_keyboard().unwrap();
        assert_eq!(sim.static_color(1), None);
        assert!(!saved.exists(), "the backup is consumed");
        assert!(!c.get_keyboard_info().unwrap().state.unwrap().backup);
        // nothing left to restore
        assert!(matches!(
            c.restore_keyboard(),
            Err(HwError::InvalidValue(_))
        ));
    }

    #[test]
    fn restore_works_after_the_feature_is_switched_off() {
        let fs = fixtures::legion_5_15iax10();
        let (c, sim) = controller(&fs, true);
        c.set_keyboard_color(Rgb::new(9, 9, 9)).unwrap();
        let (off, _) = controller(&fs, false);
        let off = off.with_keyboard_opener(Arc::new(SimOpener(sim.clone())));
        assert!(off.set_keyboard_color(Rgb::new(1, 1, 1)).is_err());
        off.restore_keyboard().unwrap();
        assert_eq!(sim.static_color(1), None);
    }

    #[test]
    fn a_profile_changed_elsewhere_gets_a_fresh_backup() {
        let fs = fixtures::legion_5_15iax10();
        let (c, sim) = controller(&fs, true);
        c.set_keyboard_color(Rgb::new(255, 0, 0)).unwrap();
        // the user switches to another effect with Vantage / Fn keys
        let rainbow = SimSpectrum::new().profile_raw(1);
        sim.set_profile_raw(1, &rainbow);
        c.set_keyboard_color(Rgb::new(0, 255, 0)).unwrap();
        let saved = std::fs::read_to_string(fs.root().path(backup_path(1))).unwrap();
        assert_eq!(
            from_hex(&saved).unwrap()[4..],
            rainbow[4..from_hex(&saved).unwrap().len()]
        );
    }

    #[test]
    fn an_unwritable_state_directory_blocks_the_change_with_a_clear_error() {
        let fs = fixtures::legion_5_15iax10();
        let (c, sim) = controller(&fs, true);
        // /var/lib is a file: the state directory cannot be created
        fs.file("/var/lib", "not a directory");
        let e = c
            .set_keyboard_color(Rgb::new(1, 2, 3))
            .unwrap_err()
            .to_string();
        assert!(e.contains("could not save a backup"), "{e}");
        assert!(
            e.contains("nothing was changed") && e.contains("systemctl restart"),
            "{e}"
        );
        assert!(sim.log().iter().all(|r| r[1] != 0xCB), "no write expected");
        assert_eq!(sim.static_color(1), None);
    }

    #[test]
    fn invalid_values_and_failures_change_nothing() {
        let fs = fixtures::legion_5_15iax10();
        let (c, sim) = controller(&fs, true);
        assert!(c.set_keyboard_brightness(10).is_err());
        sim.set_failing(true);
        assert!(c.set_keyboard_color(Rgb::new(1, 2, 3)).is_err());
        assert!(!fs.root().path(backup_path(1)).exists());
        sim.set_failing(false);
        assert_eq!(sim.static_color(1), None);
    }

    #[test]
    fn machines_without_the_controller_are_not_supported() {
        let fs = FakeSysfs::new();
        let (c, _) = controller(&fs, true);
        assert!(matches!(
            c.get_keyboard_info(),
            Err(HwError::NotSupported(_))
        ));
    }
}
