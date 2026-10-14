//! A simulated Spectrum keyboard controller.
//!
//! Used by the unit tests and by fixture roots (`--sysroot`, the D-Bus
//! integration test), where there is no `/dev/hidraw*`. It keeps the
//! request → response behaviour of the real controller, stores one effect
//! description per profile, and records every request it receives.

use std::sync::{Arc, Mutex, MutexGuard};

use legion_core::keyboard::Rgb;
use legion_core::{HwError, Result};

pub use crate::descriptors::{ITE_8258_KEYBOARD_DESCRIPTOR, ITE_8258_LAMPARRAY_DESCRIPTOR};
use crate::discover::HidNode;
use crate::spectrum::{self, Description, Effect, REPORT_ID, REPORT_LEN};
use crate::transport::{FeatureTransport, Opener};

#[derive(Debug)]
struct State {
    brightness: u8,
    profile: u8,
    /// Raw 960-byte description per profile (index 0 = profile 1).
    profiles: Vec<Vec<u8>>,
    answer: Vec<u8>,
    log: Vec<Vec<u8>>,
    failing: bool,
    garbage: bool,
}

/// The simulated hardware. Cloning shares the state.
#[derive(Debug, Clone)]
pub struct SimSpectrum {
    state: Arc<Mutex<State>>,
}

impl Default for SimSpectrum {
    fn default() -> Self {
        Self::new()
    }
}

fn stall() -> HwError {
    HwError::Backend {
        backend: "hidraw",
        message: "simulated device error".into(),
    }
}

/// Factory state of a profile: a rainbow wave.
fn rainbow(profile: u8) -> Vec<u8> {
    Description {
        profile,
        effects: vec![Effect {
            number: 1,
            kind: 2,
            speed: 1,
            clockwise: 0,
            direction: 4,
            color_mode: 1,
            colors: vec![],
            keys: vec![spectrum::ALL_KEYS],
        }],
    }
    .encode()
    .expect("valid description")
}

impl SimSpectrum {
    /// A keyboard in its factory state: profile 1 active, brightness 5,
    /// every profile a rainbow wave.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                brightness: 5,
                profile: 1,
                profiles: (1..=spectrum::MAX_PROFILE).map(rainbow).collect(),
                answer: answer(0, &[]),
                log: Vec::new(),
                failing: false,
                garbage: false,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("simulator state")
    }

    /// A transport talking to this device.
    pub fn transport(&self) -> SimTransport {
        SimTransport(self.clone())
    }

    /// Current brightness level.
    pub fn brightness(&self) -> u8 {
        self.lock().brightness
    }

    /// Active profile.
    pub fn profile(&self) -> u8 {
        self.lock().profile
    }

    /// Raw description stored for a profile (1-based).
    pub fn profile_raw(&self, profile: u8) -> Vec<u8> {
        self.lock().profiles[usize::from(profile) - 1].clone()
    }

    /// Replaces the stored contents of a profile, as if another program (or
    /// the Fn keys) had changed them.
    pub fn set_profile_raw(&self, profile: u8, raw: &[u8]) {
        let mut stored = raw.to_vec();
        stored[1] = 0xCC;
        self.lock().profiles[usize::from(profile) - 1] = stored;
    }

    /// Colour of profile `n` if it is one steady colour.
    pub fn static_color(&self, profile: u8) -> Option<Rgb> {
        Description::decode(&self.profile_raw(profile))
            .ok()?
            .as_static_color()
    }

    /// Every request received, oldest first.
    pub fn log(&self) -> Vec<Vec<u8>> {
        self.lock().log.clone()
    }

    /// Forgets the recorded requests.
    pub fn clear_log(&self) {
        self.lock().log.clear();
    }

    /// Makes every transfer fail (unplugged / stalled device).
    pub fn set_failing(&self, failing: bool) {
        self.lock().failing = failing;
    }

    /// Makes every answer something that is not a Spectrum report.
    pub fn set_garbage_answers(&self, garbage: bool) {
        self.lock().garbage = garbage;
    }
}

/// Transport of a [`SimSpectrum`].
#[derive(Debug)]
pub struct SimTransport(SimSpectrum);

/// A simple answer: as long as its payload, like the real controller's.
fn answer(op: u8, payload: &[u8]) -> Vec<u8> {
    let mut a = vec![REPORT_ID, op, payload.len() as u8, 0];
    a.extend(payload);
    a
}

impl FeatureTransport for SimTransport {
    fn get_feature(&mut self, report_id: u8, len: usize) -> Result<Vec<u8>> {
        let s = self.0.lock();
        if s.failing || report_id != REPORT_ID {
            return Err(stall());
        }
        if s.garbage {
            return Ok(vec![0xAA; 4]);
        }
        let mut a = s.answer.clone();
        a.truncate(len);
        Ok(a)
    }

    fn set_feature(&mut self, report: &[u8]) -> Result<()> {
        let mut s = self.0.lock();
        if s.failing {
            return Err(stall());
        }
        s.log.push(report.to_vec());
        if report.len() != REPORT_LEN || report[0] != REPORT_ID {
            return Err(stall());
        }
        let op = report[1];
        // every request except EffectChange (which carries a length) has size 0xC0
        if op != 0xCB && (report[2] != 0xC0 || report[3] != 0x03) {
            return Err(stall());
        }
        match op {
            0xD1 => s.answer = answer(op, &[0]),
            0xC4 => s.answer = answer(op, &[report[4], 9, 22]),
            0xCA => s.answer = answer(op, &[s.profile]),
            0xCD => s.answer = answer(op, &[s.brightness]),
            0xCE => {
                if report[4] > spectrum::MAX_BRIGHTNESS {
                    return Err(stall());
                }
                s.brightness = report[4];
            }
            0xC8 => {
                if !(1..=spectrum::MAX_PROFILE).contains(&report[4]) {
                    return Err(stall());
                }
                s.profile = report[4];
            }
            0xCC => {
                let p = report[4];
                if !(1..=spectrum::MAX_PROFILE).contains(&p) {
                    return Err(stall());
                }
                // like the real controller: `07 cc <len> 00 …`, `len` bytes of payload
                // observed on the real controller: this also activates the profile
                s.profile = p;
                let mut a = s.profiles[usize::from(p) - 1].clone();
                a[1] = op;
                a.truncate(4 + usize::from(u16::from_le_bytes([a[2], a[3]])));
                s.answer = a;
            }
            0xCB => {
                // must be a description the real parser accepts
                let d = Description::decode(report).map_err(|_| stall())?;
                if !(1..=spectrum::MAX_PROFILE).contains(&d.profile) {
                    return Err(stall());
                }
                let mut stored = report.to_vec();
                stored[1] = 0xCC;
                s.profiles[usize::from(d.profile) - 1] = stored;
            }
            _ => return Err(stall()),
        }
        Ok(())
    }
}

/// Opens the simulated device for any node.
#[derive(Debug, Clone, Default)]
pub struct SimOpener(pub SimSpectrum);

impl Opener for SimOpener {
    fn open(&self, _node: &HidNode) -> Result<Box<dyn FeatureTransport>> {
        Ok(Box::new(self.0.transport()))
    }
}

/// Adds a USB HID interface to a fake sysfs tree: `hidraw{n}` →
/// `…/usb3/3-5/3-5:1.{iface}`. The USB device is shared by all interfaces of
/// the same `(vid, pid)`.
#[cfg(any(test, feature = "testing"))]
pub fn add_node(
    fs: &legion_core::testing::FakeSysfs,
    n: u32,
    iface: u32,
    removable: &str,
    vid: u16,
    pid: u16,
    descriptor: &[u8],
) {
    let usb = "/sys/devices/pci0000:80/0000:80:14.0/usb3/3-5";
    let hid = format!("{usb}/3-5:1.{iface}/0003:{vid:04X}:{pid:04X}.000{n}");
    fs.file(&format!("{usb}/idVendor"), &format!("{vid:04x}\n"))
        .file(&format!("{usb}/removable"), &format!("{removable}\n"))
        .file(
            &format!("{hid}/uevent"),
            &format!(
                "DRIVER=hid-generic\nHID_ID=0003:0000{vid:04X}:0000{pid:04X}\n\
                 HID_NAME=ITE Tech. Inc. ITE Device(8258)\n"
            ),
        );
    std::fs::write(
        fs.root().path(format!("{hid}/report_descriptor")),
        descriptor,
    )
    .expect("write report descriptor");
    fs.symlink(&format!("/sys/class/hidraw/hidraw{n}/device"), &hid);
}

/// The two interfaces of the Legion 5 15IAX10 keyboard controller
/// (`hidraw5` = typing + vendor reports, `hidraw6` = standard LampArray).
#[cfg(any(test, feature = "testing"))]
pub fn add_legion_5_15iax10_keyboard(fs: &legion_core::testing::FakeSysfs) {
    add_node(
        fs,
        5,
        0,
        "fixed",
        0x048d,
        0xc195,
        ITE_8258_KEYBOARD_DESCRIPTOR,
    );
    add_node(
        fs,
        6,
        1,
        "fixed",
        0x048d,
        0xc195,
        ITE_8258_LAMPARRAY_DESCRIPTOR,
    );
}
