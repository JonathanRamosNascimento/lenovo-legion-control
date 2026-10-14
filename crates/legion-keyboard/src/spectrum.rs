//! The Lenovo **Spectrum** keyboard-lighting protocol (ITE 8258 controllers
//! of the Gen 10 Legion laptops).
//!
//! # Provenance — read this first
//!
//! This is a **vendor protocol with no public Lenovo documentation**. The
//! packet layouts below come from the community reverse engineering done in
//! the MIT-licensed *LenovoLegionToolkit* (`Native.cs`, `SpectrumKeyboard*`)
//! and were cross-checked with the keyboard itself: the report exists in the
//! device's own descriptor (vendor page `0xff89`, report `7`, 959 bytes of
//! payload) and the read-only queries are validated against the answers of
//! the real controller (see `docs/hardware-support.md`). Nothing here is
//! copied code; only the protocol facts are used.
//!
//! The standard HID LampArray interface of the same controller was tested on
//! the reference machine and is accepted but inert, so it is **not** used.
//!
//! # Wire format
//!
//! Everything is a 960-byte *feature* report with id `7`, in a
//! request → response style: a request is written with `SET_REPORT`, the
//! answer is read back with `GET_REPORT`.
//!
//! ```text
//! request : 07 <op> C0 03 <param> <param2> 00 …            (960 bytes)
//! response: 07 <op> <len> <?>  <payload …>                  (960 bytes)
//! ```
//!
//! Lighting is stored in **profiles** inside the keyboard (and survives
//! reboots), each holding a list of *effects*; nothing needs to be re-applied
//! at boot. Writing a profile writes flash memory in the controller, so the
//! daemon only does it on explicit user actions.

use std::time::Duration;

use legion_core::keyboard::Rgb;
use legion_core::{HwError, Result};

use crate::hid::{Descriptor, ReportKind};
use crate::transport::FeatureTransport;

/// Vendor usage page of the collection holding the report.
pub const USAGE_PAGE: u16 = 0xFF89;
/// Usage of the application collection that owns report 7.
pub const USAGE_APPLICATION: u16 = 0x07;
/// Report id.
pub const REPORT_ID: u8 = 7;
/// Length of every report including the report id.
pub const REPORT_LEN: usize = 960;
/// Highest brightness level.
pub const MAX_BRIGHTNESS: u8 = 9;
/// Highest profile number the protocol accepts.
pub const MAX_PROFILE: u8 = 6;
/// Key code that stands for "every key" in an effect's key list.
pub const ALL_KEYS: u16 = 0x65;

const HEAD: u8 = 0x07;
const TAIL: u8 = 0x03;
/// The size byte of every request (a constant of the protocol).
const REQUEST_SIZE: u8 = 0xC0;

/// Operation codes (`LENOVO_SPECTRUM_OPERATION_TYPE`) used here.
pub mod op {
    /// `GET_KEY_COUNT` / `GET_KEY_PAGE`: keyboard layout.
    pub const KEY_COUNT: u8 = 0xC4;
    /// Activate a profile.
    pub const PROFILE_CHANGE: u8 = 0xC8;
    /// Query the active profile.
    pub const PROFILE: u8 = 0xCA;
    /// Write the effects of a profile.
    pub const EFFECT_CHANGE: u8 = 0xCB;
    /// Read the effects of a profile.
    pub const EFFECT: u8 = 0xCC;
    /// Query the brightness.
    pub const GET_BRIGHTNESS: u8 = 0xCD;
    /// Set the brightness.
    pub const BRIGHTNESS: u8 = 0xCE;
    /// Query whether the controller is compatible.
    pub const COMPATIBILITY: u8 = 0xD1;
}

/// Effect type byte (`LENOVO_SPECTRUM_EFFECT_TYPE`).
pub mod effect {
    /// Steady colour.
    pub const ALWAYS: u8 = 11;
    /// Lights off (the profile the controller ships for "off").
    pub const OFF: u8 = 255;

    /// Name of an effect type.
    pub fn name(kind: u8) -> &'static str {
        match kind {
            1 => "screw-rainbow",
            2 => "rainbow-wave",
            3 => "color-change",
            4 => "color-pulse",
            5 => "color-wave",
            6 => "smooth",
            7 => "rain",
            8 => "ripple",
            9 => "audio-bounce",
            10 => "audio-ripple",
            ALWAYS => "static",
            12 => "type-lighting",
            OFF => "off",
            13 => "legion-aura-sync",
            _ => "unknown",
        }
    }
}

/// Colour source of an effect (`LENOVO_SPECTRUM_COLOR_MODE`).
const COLOR_MODE_LIST: u8 = 2;

/// Checks the device's report descriptor: application collection
/// `0xff89/0x07` with one feature report `7` of exactly 959 payload bytes.
pub fn check_descriptor(d: &Descriptor) -> std::result::Result<(), String> {
    let app = (USAGE_PAGE, USAGE_APPLICATION);
    if !d.applications().contains(&app) {
        return Err("no vendor collection 0xff89/0x07".into());
    }
    let mut report = d
        .report(ReportKind::Feature, REPORT_ID)
        .filter(|f| f.application == app);
    match (report.next(), report.next()) {
        (Some(f), None) if f.bits == 8 && f.count as usize == REPORT_LEN - 1 => Ok(()),
        _ => Err("unexpected layout of feature report 7".into()),
    }
}

fn bad(msg: impl Into<String>) -> HwError {
    HwError::Backend {
        backend: "hidraw",
        message: msg.into(),
    }
}

/// Builds a request report.
fn request(op: u8, params: &[u8]) -> Vec<u8> {
    debug_assert!(params.len() < 16);
    let mut v = vec![0u8; REPORT_LEN];
    v[0] = HEAD;
    v[1] = op;
    v[2] = REQUEST_SIZE;
    v[3] = TAIL;
    v[4..4 + params.len()].copy_from_slice(params);
    v
}

// ───────────────────────────── effects ─────────────────────────────

/// One effect of a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    /// 1-based position inside the profile.
    pub number: u8,
    /// Effect type (see [`effect`]).
    pub kind: u8,
    /// Speed (0 = none, 1–3).
    pub speed: u8,
    /// Clockwise direction (0 = none, 1, 2).
    pub clockwise: u8,
    /// Direction (0 = none, 1–4).
    pub direction: u8,
    /// Colour mode (0 = none, 1 = random, 2 = list).
    pub color_mode: u8,
    /// Colours.
    pub colors: Vec<Rgb>,
    /// Key codes the effect applies to.
    pub keys: Vec<u16>,
}

/// The effects stored in one profile (`LENOVO_SPECTRUM_EFFECT_DESCRIPTION`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    /// Profile number.
    pub profile: u8,
    /// Effects, in order.
    pub effects: Vec<Effect>,
}

/// Bytes of the effect header after the effect number:
/// `06 01 <type> 02 <speed> 03 <clockwise> 04 <direction> 05 <mode> 06 00`.
const EFFECT_HEADER_LEN: usize = 13;
/// Offset of the first effect inside a description.
const FIRST_EFFECT: usize = 7;
/// Sanity bound when decoding.
const MAX_EFFECTS: usize = 64;

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.at)?;
        self.at += 1;
        Some(v)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.at = self.at.checked_add(n).filter(|a| *a <= self.b.len())?;
        Some(())
    }
}

impl Description {
    /// Decodes a description read from the keyboard (response to
    /// "get effect"). Malformed data yields an error, never a panic.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let short = || bad("truncated effect description");
        if buf.len() < FIRST_EFFECT || buf[0] != HEAD {
            return Err(bad("not a Spectrum effect description"));
        }
        let profile = buf[4];
        let mut r = Reader {
            b: buf,
            at: FIRST_EFFECT,
        };
        let mut effects: Vec<Effect> = Vec::new();
        let mut previous = 1u8;
        while let Some(&number) = buf.get(r.at) {
            // numbers ascend; the zero padding after the last effect ends the list
            if number < previous || number == 0 {
                break;
            }
            if effects.len() >= MAX_EFFECTS {
                return Err(bad("too many effects in a profile"));
            }
            previous = number;
            r.skip(1).ok_or_else(short)?;
            let h = buf.get(r.at..r.at + EFFECT_HEADER_LEN).ok_or_else(short)?;
            r.skip(EFFECT_HEADER_LEN).ok_or_else(short)?;
            let n_colors = usize::from(r.u8().ok_or_else(short)?);
            let mut colors = Vec::with_capacity(n_colors);
            for _ in 0..n_colors {
                let c = buf.get(r.at..r.at + 3).ok_or_else(short)?;
                colors.push(Rgb::new(c[0], c[1], c[2]));
                r.skip(3).ok_or_else(short)?;
            }
            let n_keys = usize::from(r.u8().ok_or_else(short)?);
            let mut keys = Vec::with_capacity(n_keys);
            for _ in 0..n_keys {
                let k = buf.get(r.at..r.at + 2).ok_or_else(short)?;
                keys.push(u16::from_le_bytes([k[0], k[1]]));
                r.skip(2).ok_or_else(short)?;
            }
            effects.push(Effect {
                number,
                kind: h[2],
                speed: h[4],
                clockwise: h[6],
                direction: h[8],
                color_mode: h[10],
                colors,
                keys,
            });
        }
        Ok(Self { profile, effects })
    }

    /// Encodes the description as an "effect change" request.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.profile == 0 || self.profile > MAX_PROFILE {
            return Err(HwError::InvalidValue(format!(
                "profile must be 1..={MAX_PROFILE}"
            )));
        }
        let mut v = vec![HEAD, op::EFFECT_CHANGE, 0, 0, self.profile, 1, 1];
        for (n, e) in self.effects.iter().enumerate() {
            if usize::from(e.number) != n + 1 {
                return Err(HwError::InvalidValue(
                    "effect numbers must be 1, 2, 3…".into(),
                ));
            }
            v.push(e.number);
            v.extend([
                0x06,
                0x01,
                e.kind,
                0x02,
                e.speed,
                0x03,
                e.clockwise,
                0x04,
                e.direction,
                0x05,
                e.color_mode,
                0x06,
                0x00,
            ]);
            v.push(u8::try_from(e.colors.len()).map_err(|_| bad("too many colours"))?);
            for c in &e.colors {
                v.extend([c.r, c.g, c.b]);
            }
            v.push(u8::try_from(e.keys.len()).map_err(|_| bad("too many keys"))?);
            for k in &e.keys {
                v.extend(k.to_le_bytes());
            }
        }
        if v.len() > REPORT_LEN {
            return Err(HwError::InvalidValue("effect description too large".into()));
        }
        // the length field (bytes 2–3) counts everything after the 4-byte head
        let len = u16::try_from(v.len() - 4).expect("bounded by REPORT_LEN");
        v[2..4].copy_from_slice(&len.to_le_bytes());
        v.resize(REPORT_LEN, 0);
        Ok(v)
    }

    /// A single steady colour on every key.
    pub fn static_color(profile: u8, color: Rgb) -> Self {
        Self {
            profile,
            effects: vec![Effect {
                number: 1,
                kind: effect::ALWAYS,
                speed: 0,
                clockwise: 0,
                direction: 0,
                color_mode: COLOR_MODE_LIST,
                colors: vec![color],
                keys: vec![ALL_KEYS],
            }],
        }
    }

    /// The colour if the profile is one steady colour on all keys.
    pub fn as_static_color(&self) -> Option<Rgb> {
        match self.effects.as_slice() {
            [e] if e.kind == effect::ALWAYS && e.colors.len() == 1 => Some(e.colors[0]),
            _ => None,
        }
    }
}

// ───────────────────────────── client ─────────────────────────────

/// Layout answer of the keyboard (`GET_KEY_COUNT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCount {
    /// Number of key pages.
    pub indexes: u8,
    /// Keys per page.
    pub keys_per_index: u8,
}

/// A Spectrum controller reachable through a [`FeatureTransport`].
pub struct Spectrum<T> {
    transport: T,
    /// Pause between a request and reading its answer.
    pub settle: Duration,
}

impl<T: FeatureTransport> Spectrum<T> {
    /// Wraps a transport.
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            settle: Duration::from_millis(20),
        }
    }

    fn send(&mut self, report: &[u8]) -> Result<()> {
        debug_assert_eq!(report.len(), REPORT_LEN);
        self.transport.set_feature(report)
    }

    fn pause(&self) {
        if !self.settle.is_zero() {
            std::thread::sleep(self.settle);
        }
    }

    /// Sends a request and reads the answer.
    ///
    /// Answers are `07 <op> <payload length> <?> <payload…>` and are as long
    /// as their payload (4–7 bytes for simple queries), not 960 bytes.
    fn query(&mut self, op: u8, params: &[u8]) -> Result<Vec<u8>> {
        self.send(&request(op, params))?;
        self.pause();
        let r = self.transport.get_feature(REPORT_ID, REPORT_LEN)?;
        let declared = r.get(2).map_or(0, |n| usize::from(*n));
        if r.len() < 4 || r[0] != REPORT_ID || r.len() < 4 + declared {
            return Err(bad(format!(
                "unexpected answer to operation {op:#04x}: {} bytes, header {:02x?}",
                r.len(),
                &r[..r.len().min(4)]
            )));
        }
        Ok(r)
    }

    /// Payload byte `n` (0-based) of an answer.
    fn payload(answer: &[u8], n: usize) -> Result<u8> {
        answer
            .get(4 + n)
            .copied()
            .ok_or_else(|| bad("answer is shorter than expected"))
    }

    /// Sends one of the **read-only** queries and returns the raw answer
    /// (diagnostics). Any other operation is refused.
    pub fn read_raw(&mut self, op: u8, params: &[u8]) -> Result<Vec<u8>> {
        // NOT `op::EFFECT`: asking for the effect of a profile also *activates*
        // that profile (observed on hardware), so it is not a pure read.
        const READ_ONLY: [u8; 4] = [
            op::COMPATIBILITY,
            op::KEY_COUNT,
            op::PROFILE,
            op::GET_BRIGHTNESS,
        ];
        if !READ_ONLY.contains(&op) || params.len() > 2 {
            return Err(HwError::InvalidValue(format!(
                "operation {op:#04x} is not a read-only query"
            )));
        }
        self.query(op, params)
    }

    /// Whether the controller declares itself compatible (answer byte 4 = 0).
    pub fn compatible(&mut self) -> Result<bool> {
        Ok(Self::payload(&self.query(op::COMPATIBILITY, &[])?, 0)? == 0)
    }

    /// Current brightness level (`0..=`[`MAX_BRIGHTNESS`]).
    pub fn brightness(&mut self) -> Result<u8> {
        Self::payload(&self.query(op::GET_BRIGHTNESS, &[])?, 0)
    }

    /// Sets the brightness level.
    pub fn set_brightness(&mut self, level: u8) -> Result<()> {
        if level > MAX_BRIGHTNESS {
            return Err(HwError::InvalidValue(format!(
                "brightness must be 0..={MAX_BRIGHTNESS}"
            )));
        }
        self.send(&request(op::BRIGHTNESS, &[level]))?;
        self.pause();
        Ok(())
    }

    /// Active profile.
    pub fn profile(&mut self) -> Result<u8> {
        Self::payload(&self.query(op::PROFILE, &[])?, 0)
    }

    /// Activates a profile.
    pub fn select_profile(&mut self, profile: u8) -> Result<()> {
        if profile == 0 || profile > MAX_PROFILE {
            return Err(HwError::InvalidValue(format!(
                "profile must be 1..={MAX_PROFILE}"
            )));
        }
        self.send(&request(op::PROFILE_CHANGE, &[profile]))?;
        // the controller needs a moment to switch (the reference
        // implementation waits 100 ms)
        self.settle_long();
        Ok(())
    }

    fn settle_long(&self) {
        if !self.settle.is_zero() {
            std::thread::sleep(self.settle * 5);
        }
    }

    /// Number of key pages and keys per page.
    pub fn key_count(&mut self) -> Result<KeyCount> {
        let r = self.query(op::KEY_COUNT, &[7])?;
        Ok(KeyCount {
            indexes: Self::payload(&r, 1)?,
            keys_per_index: Self::payload(&r, 2)?,
        })
    }

    /// The active profile number and the raw answer describing its effects.
    ///
    /// Only the **active** profile is ever read: on the reference machine,
    /// asking for the effect of another profile activates it (and reading
    /// the "lights off" profile switched the backlight off).
    pub fn active_effect(&mut self) -> Result<(u8, Vec<u8>)> {
        let profile = self.profile()?;
        if profile == 0 || profile > MAX_PROFILE {
            return Err(bad(format!("controller reports profile {profile}")));
        }
        let raw = self.query(op::EFFECT, &[profile])?;
        if raw.get(4) != Some(&profile) {
            return Err(bad(format!(
                "asked for the effect of profile {profile}, got another one"
            )));
        }
        Ok((profile, raw))
    }

    /// Writes a profile description (flash write — use sparingly) and makes
    /// sure the controller shows it by re-selecting the profile.
    pub fn write_description(&mut self, d: &Description) -> Result<()> {
        let bytes = d.encode()?;
        self.send(&bytes)?;
        self.settle_long();
        self.select_profile(d.profile)
    }

    /// Writes back a description previously read with
    /// [`Spectrum::active_effect`], byte for byte (including anything this
    /// crate does not interpret). The data is validated first.
    pub fn write_raw(&mut self, raw: &[u8]) -> Result<u8> {
        let d = Description::decode(raw)?;
        if d.profile == 0 || d.profile > MAX_PROFILE || raw.len() > REPORT_LEN {
            return Err(HwError::InvalidValue("not a valid profile backup".into()));
        }
        let mut bytes = raw.to_vec();
        bytes[1] = op::EFFECT_CHANGE;
        bytes.resize(REPORT_LEN, 0);
        self.send(&bytes)?;
        self.settle_long();
        self.select_profile(d.profile)?;
        Ok(d.profile)
    }

    /// The transport (for tests).
    pub fn into_transport(self) -> T {
        self.transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptors::{ITE_8258_KEYBOARD_DESCRIPTOR, ITE_8258_LAMPARRAY_DESCRIPTOR};
    use crate::sim::SimSpectrum;

    fn client(sim: &SimSpectrum) -> Spectrum<crate::sim::SimTransport> {
        let mut s = Spectrum::new(sim.transport());
        s.settle = Duration::ZERO;
        s
    }

    #[test]
    fn the_real_descriptor_declares_report_7() {
        let d = Descriptor::parse(ITE_8258_KEYBOARD_DESCRIPTOR).unwrap();
        assert_eq!(check_descriptor(&d), Ok(()));
        assert_eq!(
            d.report_bytes(ReportKind::Feature, REPORT_ID) + 1,
            REPORT_LEN
        );
        // the LampArray interface of the same device is something else
        let d = Descriptor::parse(ITE_8258_LAMPARRAY_DESCRIPTOR).unwrap();
        assert!(check_descriptor(&d).is_err());
    }

    #[test]
    fn requests_have_the_documented_shape() {
        let r = request(op::BRIGHTNESS, &[5]);
        assert_eq!(r.len(), REPORT_LEN);
        assert_eq!(&r[..6], [0x07, 0xCE, 0xC0, 0x03, 5, 0]);
        assert!(r[6..].iter().all(|b| *b == 0));
    }

    #[test]
    fn static_color_encoding_is_exact() {
        let d = Description::static_color(2, Rgb::new(0x11, 0x22, 0x33));
        let b = d.encode().unwrap();
        assert_eq!(b.len(), REPORT_LEN);
        // head, EffectChange, length = 24 bytes after the 4-byte head
        assert_eq!(&b[..4], [0x07, 0xCB, 0x18, 0x00]);
        assert_eq!(&b[4..7], [2, 1, 1]);
        assert_eq!(b[7], 1); // effect number
        assert_eq!(
            &b[8..21],
            [
                0x06, 0x01, 11, 0x02, 0, 0x03, 0, 0x04, 0, 0x05, 2, 0x06, 0x00
            ]
        );
        assert_eq!(&b[21..25], [1, 0x11, 0x22, 0x33]); // one colour
        assert_eq!(&b[25..28], [1, 0x65, 0x00]); // one key: "all"
        assert!(b[28..].iter().all(|x| *x == 0));
    }

    #[test]
    fn descriptions_round_trip() {
        let d = Description {
            profile: 3,
            effects: vec![
                Effect {
                    number: 1,
                    kind: 2,
                    speed: 1,
                    clockwise: 0,
                    direction: 4,
                    color_mode: 1,
                    colors: vec![],
                    keys: vec![0x65],
                },
                Effect {
                    number: 2,
                    kind: effect::ALWAYS,
                    speed: 0,
                    clockwise: 0,
                    direction: 0,
                    color_mode: 2,
                    colors: vec![Rgb::new(1, 2, 3), Rgb::new(4, 5, 6)],
                    keys: vec![0x0101, 0x0202, 0x0303],
                },
            ],
        };
        assert_eq!(Description::decode(&d.encode().unwrap()).unwrap(), d);
        assert_eq!(d.as_static_color(), None);
        let s = Description::static_color(1, Rgb::new(9, 8, 7));
        assert_eq!(s.as_static_color(), Some(Rgb::new(9, 8, 7)));
    }

    #[test]
    fn decoding_never_panics_on_garbage() {
        let good = Description::static_color(1, Rgb::new(1, 2, 3))
            .encode()
            .unwrap();
        // truncate everywhere
        for n in 0..40 {
            let _ = Description::decode(&good[..n]);
        }
        // claim more colours/keys than the buffer holds
        let mut b = good.clone();
        b[21] = 255;
        assert!(Description::decode(&b[..40]).is_err());
        assert!(Description::decode(&[]).is_err());
        assert!(Description::decode(&[0u8; 960]).is_err()); // wrong head
        // a pseudo-random sweep
        let mut x = 0x1234_5678u32;
        for _ in 0..500 {
            let mut buf = vec![HEAD; 100];
            for b in buf.iter_mut().skip(1) {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                *b = (x >> 24) as u8;
            }
            let _ = Description::decode(&buf);
        }
    }

    #[test]
    fn invalid_encodings_are_rejected() {
        assert!(
            Description::static_color(0, Rgb::default())
                .encode()
                .is_err()
        );
        assert!(
            Description::static_color(7, Rgb::default())
                .encode()
                .is_err()
        );
        let mut d = Description::static_color(1, Rgb::default());
        d.effects[0].number = 2;
        assert!(d.encode().is_err());
        let mut d = Description::static_color(1, Rgb::default());
        d.effects[0].keys = vec![1; 900];
        assert!(d.encode().is_err());
    }

    #[test]
    fn talks_to_a_simulated_controller() {
        let sim = SimSpectrum::new();
        let mut s = client(&sim);
        assert!(s.compatible().unwrap());
        assert_eq!(s.brightness().unwrap(), 5);
        assert_eq!(s.profile().unwrap(), 1);
        assert_eq!(
            s.key_count().unwrap(),
            KeyCount {
                indexes: 9,
                keys_per_index: 22
            }
        );
        let (p, raw) = s.active_effect().unwrap();
        assert_eq!(p, 1);
        let original = Description::decode(&raw).unwrap();
        assert_eq!(original.effects[0].kind, 2); // rainbow wave by default
        assert_eq!(original.as_static_color(), None);

        s.set_brightness(9).unwrap();
        assert_eq!(s.brightness().unwrap(), 9);
        assert!(s.set_brightness(10).is_err());

        s.write_description(&Description::static_color(1, Rgb::new(255, 0, 0)))
            .unwrap();
        assert_eq!(sim.static_color(1), Some(Rgb::new(255, 0, 0)));
        assert!(s.select_profile(0).is_err());
        assert!(s.select_profile(7).is_err());
    }

    #[test]
    fn backups_restore_byte_for_byte() {
        let sim = SimSpectrum::new();
        let mut s = client(&sim);
        let (_, backup) = s.active_effect().unwrap();
        s.write_description(&Description::static_color(1, Rgb::new(0, 255, 0)))
            .unwrap();
        assert_ne!(
            Description::decode(&backup).unwrap().as_static_color(),
            Some(Rgb::new(0, 255, 0))
        );
        assert_eq!(s.write_raw(&backup).unwrap(), 1);
        let (_, now) = s.active_effect().unwrap();
        assert_eq!(now, backup);
        // garbage is refused before anything is sent
        sim.clear_log();
        assert!(s.write_raw(&[1, 2, 3]).is_err());
        assert!(sim.log().is_empty());
    }

    #[test]
    fn reading_never_switches_profiles() {
        let sim = SimSpectrum::new();
        let mut s = client(&sim);
        s.select_profile(4).unwrap();
        sim.clear_log();
        let (p, _) = s.active_effect().unwrap();
        assert_eq!(p, 4);
        assert_eq!(sim.profile(), 4);
        // the only effect query goes to the active profile
        let effect_queries: Vec<u8> = sim
            .log()
            .iter()
            .filter(|r| r[1] == op::EFFECT)
            .map(|r| r[4])
            .collect();
        assert_eq!(effect_queries, [4]);
        // the diagnostic helper refuses the effect query altogether
        assert!(s.read_raw(op::EFFECT, &[2]).is_err());
        assert!(s.read_raw(op::EFFECT_CHANGE, &[]).is_err());
        assert!(s.read_raw(op::BRIGHTNESS, &[0]).is_err());
    }

    #[test]
    fn answers_that_do_not_look_like_spectrum_are_errors() {
        let sim = SimSpectrum::new();
        sim.set_garbage_answers(true);
        let mut s = client(&sim);
        assert!(s.brightness().is_err());
        sim.set_failing(true);
        sim.set_garbage_answers(false);
        assert!(s.brightness().is_err());
    }
}
