//! High-level operations on the built-in keyboard, the way the daemon uses
//! them: every call opens the device, checks the controller declares itself
//! compatible, does its work and closes it.
//!
//! Safety rules enforced here:
//!
//! * only the **active** profile is ever read or modified;
//! * the previous contents of a profile are handed back **before** it is
//!   overwritten, so the caller can keep a backup ([`Applied::previous`]);
//! * nothing is written when the keyboard already shows the requested
//!   colour (the controller stores profiles in flash).

use std::sync::Arc;

use legion_core::keyboard::Rgb;
use legion_core::{HwError, Result};

use crate::discover::HidNode;
use crate::spectrum::{self, Description, KeyCount, Spectrum};
use crate::transport::{FeatureTransport, Opener};

/// What the keyboard reports about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Brightness level (`0..=`[`spectrum::MAX_BRIGHTNESS`]).
    pub brightness: u8,
    /// Active profile (`1..=`[`spectrum::MAX_PROFILE`]).
    pub profile: u8,
    /// Effects of the active profile.
    pub effect: Description,
    /// Key pages / keys per page.
    pub key_count: KeyCount,
}

/// Result of [`Keyboard::set_static_color`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Profile that was (or already was) set to the colour.
    pub profile: u8,
    /// Raw contents of the profile before the change, as read from the
    /// keyboard — the backup to restore with [`Keyboard::restore`].
    pub previous: Vec<u8>,
    /// `false` when the keyboard already showed this colour (nothing written).
    pub changed: bool,
}

/// The built-in keyboard controller.
#[derive(Debug, Clone)]
pub struct Keyboard {
    node: HidNode,
    opener: Arc<dyn Opener>,
}

impl Keyboard {
    /// A keyboard reachable through `opener`.
    pub fn new(node: HidNode, opener: Arc<dyn Opener>) -> Self {
        Self { node, opener }
    }

    /// The HID node in use.
    pub fn node(&self) -> &HidNode {
        &self.node
    }

    fn session(&self) -> Result<Spectrum<Box<dyn FeatureTransport>>> {
        let mut s = Spectrum::new(self.opener.open(&self.node)?);
        if !s.compatible()? {
            return Err(HwError::NotSupported(format!(
                "the keyboard controller ({}) does not declare itself compatible",
                self.node.usb_id()
            )));
        }
        Ok(s)
    }

    /// Reads brightness, active profile and its effects. Changes nothing.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut s = self.session()?;
        let brightness = s.brightness()?;
        let (profile, raw) = s.active_effect()?;
        Ok(Snapshot {
            brightness,
            profile,
            effect: Description::decode(&raw)?,
            key_count: s.key_count()?,
        })
    }

    /// Sets one steady colour on every key of the active profile.
    ///
    /// `before_write(profile, previous)` runs with the profile's current raw
    /// contents right before they are overwritten (not at all when nothing
    /// needs writing); if it fails, nothing is written. This is where the
    /// caller saves its backup.
    pub fn set_static_color(
        &self,
        color: Rgb,
        before_write: impl FnOnce(u8, &[u8]) -> Result<()>,
    ) -> Result<Applied> {
        let mut s = self.session()?;
        let (profile, previous) = s.active_effect()?;
        let current = Description::decode(&previous)?;
        // use the key list the controller itself uses for this profile
        let mut keys: Vec<u16> = current
            .effects
            .iter()
            .flat_map(|e| e.keys.clone())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        if keys.is_empty() {
            keys.push(spectrum::ALL_KEYS);
        }
        let mut wanted = Description::static_color(profile, color);
        wanted.effects[0].keys = keys;
        if current == wanted {
            return Ok(Applied {
                profile,
                previous,
                changed: false,
            });
        }
        before_write(profile, &previous)?;
        tracing::info!(target: "legion_audit", profile, %color, "writing keyboard profile");
        s.write_description(&wanted)?;
        Ok(Applied {
            profile,
            previous,
            changed: true,
        })
    }

    /// Active profile number.
    pub fn active_profile(&self) -> Result<u8> {
        self.session()?.profile()
    }

    /// Writes back a backup made by [`Keyboard::set_static_color`]. Returns
    /// the profile restored.
    pub fn restore(&self, backup: &[u8]) -> Result<u8> {
        let mut s = self.session()?;
        tracing::info!(target: "legion_audit", "restoring keyboard profile from backup");
        s.write_raw(backup)
    }

    /// Sets the brightness level.
    pub fn set_brightness(&self, level: u8) -> Result<()> {
        let mut s = self.session()?;
        tracing::info!(target: "legion_audit", level, "setting keyboard brightness");
        s.set_brightness(level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{SimOpener, SimSpectrum, add_legion_5_15iax10_keyboard};
    use legion_core::testing::FakeSysfs;

    fn keyboard() -> (Keyboard, SimSpectrum) {
        let fs = FakeSysfs::new();
        add_legion_5_15iax10_keyboard(&fs);
        let crate::Detection::Found(node) = crate::detect(&fs.root()) else {
            panic!("fixture keyboard not found");
        };
        let sim = SimSpectrum::new();
        let kb = Keyboard::new(*node, Arc::new(SimOpener(sim.clone())));
        (kb, sim)
    }

    #[test]
    fn snapshot_reads_without_writing() {
        let (kb, sim) = keyboard();
        let snap = kb.snapshot().unwrap();
        assert_eq!((snap.brightness, snap.profile), (5, 1));
        assert_eq!(snap.effect.effects[0].kind, 2);
        assert_eq!(snap.key_count.indexes, 9);
        // only queries: no brightness/profile/effect-change request
        let writes: Vec<u8> = sim
            .log()
            .iter()
            .map(|r| r[1])
            .filter(|op| [0xCB, 0xCE, 0xC8].contains(op))
            .collect();
        assert!(writes.is_empty(), "{writes:02x?}");
    }

    #[test]
    fn colour_is_set_on_the_active_profile_with_a_backup() {
        let (kb, sim) = keyboard();
        let before = sim.profile_raw(1);
        let applied = kb
            .set_static_color(Rgb::new(255, 0, 0), |_, _| Ok(()))
            .unwrap();
        assert!(applied.changed);
        assert_eq!(applied.profile, 1);
        // the backup is what was on the keyboard, byte for byte (minus the
        // response/request type byte)
        assert_eq!(applied.previous[4..], before[4..applied.previous.len()]);
        assert_eq!(sim.static_color(1), Some(Rgb::new(255, 0, 0)));
        // other profiles are untouched
        assert_eq!(sim.static_color(2), None);
        assert_eq!(sim.profile(), 1);

        // the key list of the profile is reused (the sim has the ALL key)
        let after = Description::decode(&sim.profile_raw(1)).unwrap();
        assert_eq!(after.effects[0].keys, vec![spectrum::ALL_KEYS]);

        // restoring brings the original effect back
        assert_eq!(kb.restore(&applied.previous).unwrap(), 1);
        assert_eq!(sim.static_color(1), None);
        assert_eq!(
            Description::decode(&sim.profile_raw(1)).unwrap().effects[0].kind,
            2
        );
    }

    #[test]
    fn same_colour_is_not_written_twice() {
        let (kb, sim) = keyboard();
        assert!(
            kb.set_static_color(Rgb::new(1, 2, 3), |_, _| Ok(()))
                .unwrap()
                .changed
        );
        sim.clear_log();
        let again = kb
            .set_static_color(Rgb::new(1, 2, 3), |_, _| {
                panic!("nothing to back up when nothing is written")
            })
            .unwrap();
        assert!(!again.changed);
        assert!(
            sim.log().iter().all(|r| r[1] != 0xCB),
            "no flash write expected"
        );
        // and the backup of an already-ours profile is the static colour
        assert_eq!(
            Description::decode(&again.previous)
                .unwrap()
                .as_static_color(),
            Some(Rgb::new(1, 2, 3))
        );
    }

    #[test]
    fn nothing_is_written_when_the_backup_fails() {
        let (kb, sim) = keyboard();
        let before = sim.profile_raw(1);
        sim.clear_log();
        let r = kb.set_static_color(Rgb::new(9, 9, 9), |_, _| {
            Err(HwError::InvalidValue("disk full".into()))
        });
        assert!(r.is_err());
        assert!(sim.log().iter().all(|r| r[1] != 0xCB), "no write expected");
        assert_eq!(sim.profile_raw(1), before);
    }

    #[test]
    fn the_backup_hook_sees_the_original_bytes() {
        let (kb, sim) = keyboard();
        let mut seen = None;
        kb.set_static_color(Rgb::new(9, 9, 9), |p, raw| {
            seen = Some((p, raw.to_vec()));
            Ok(())
        })
        .unwrap();
        let (p, raw) = seen.unwrap();
        assert_eq!(p, 1);
        assert_eq!(Description::decode(&raw).unwrap().effects[0].kind, 2);
        assert_eq!(sim.static_color(1), Some(Rgb::new(9, 9, 9)));
    }

    #[test]
    fn brightness_is_validated() {
        let (kb, sim) = keyboard();
        kb.set_brightness(9).unwrap();
        assert_eq!(sim.brightness(), 9);
        assert!(kb.set_brightness(10).is_err());
        assert_eq!(sim.brightness(), 9);
    }

    #[test]
    fn failures_are_reported() {
        let (kb, sim) = keyboard();
        sim.set_failing(true);
        assert!(kb.snapshot().is_err());
        assert!(kb.set_static_color(Rgb::default(), |_, _| Ok(())).is_err());
    }
}
