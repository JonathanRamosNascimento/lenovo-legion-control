//! Keyboard backlight types shared by the daemon, CLI and GUI.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::HwError;

/// A 24-bit colour. Serialised as `"#rrggbb"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// Builds a colour.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Named colours accepted on the command line.
    pub const NAMED: [(&'static str, Rgb); 9] = [
        ("white", Rgb::new(255, 255, 255)),
        ("red", Rgb::new(255, 0, 0)),
        ("orange", Rgb::new(255, 128, 0)),
        ("yellow", Rgb::new(255, 255, 0)),
        ("green", Rgb::new(0, 255, 0)),
        ("cyan", Rgb::new(0, 255, 255)),
        ("blue", Rgb::new(0, 0, 255)),
        ("purple", Rgb::new(160, 32, 255)),
        ("pink", Rgb::new(255, 64, 160)),
    ];
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl FromStr for Rgb {
    type Err = HwError;

    /// Accepts `#rrggbb`, `rrggbb`, `#rgb` and the names in [`Rgb::NAMED`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_lowercase();
        if let Some((_, c)) = Self::NAMED.iter().find(|(n, _)| *n == t) {
            return Ok(*c);
        }
        let hex = t.strip_prefix('#').unwrap_or(&t);
        let bad = || {
            HwError::InvalidValue(format!(
                "invalid colour {s:?} (use #rrggbb or one of: {})",
                Self::NAMED.map(|(n, _)| n).join(", ")
            ))
        };
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
        match hex.len() {
            6 => Ok(Rgb::new(byte(0)?, byte(2)?, byte(4)?)),
            3 => {
                let n = |i: usize| u8::from_str_radix(&hex[i..=i], 16).map(|v| v * 17);
                Ok(Rgb::new(
                    n(0).map_err(|_| bad())?,
                    n(1).map_err(|_| bad())?,
                    n(2).map_err(|_| bad())?,
                ))
            }
            _ => Err(bad()),
        }
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// State of the keyboard lighting, read from the controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardState {
    /// Brightness level, `0..=max_brightness`.
    pub brightness: u8,
    /// Active lighting profile stored in the keyboard.
    pub profile: u8,
    /// Name of the active effect (`static`, `rainbow-wave`, …).
    pub effect: String,
    /// The colour, when the effect is one steady colour on every key.
    pub color: Option<Rgb>,
    /// Whether the effect that was there before Legion Control changed it is
    /// saved, so it can be restored.
    pub backup: bool,
}

/// Built-in keyboard backlight (experimental, vendor protocol).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardInfo {
    /// HID device name.
    pub device: String,
    /// USB id (`048d:c195`).
    pub usb_id: String,
    /// `hidraw` node used.
    pub node: String,
    /// Whether the experimental control is enabled in
    /// `/etc/legion-control/daemon.json` (`enable_keyboard_rgb`).
    pub enabled: bool,
    /// Highest brightness level.
    pub max_brightness: u8,
    /// Current state; `None` while the control is disabled (the device is
    /// not queried at all) .
    pub state: Option<KeyboardState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colours() {
        assert_eq!("#ff8000".parse::<Rgb>().unwrap(), Rgb::new(255, 128, 0));
        assert_eq!("FF8000".parse::<Rgb>().unwrap(), Rgb::new(255, 128, 0));
        assert_eq!("#f80".parse::<Rgb>().unwrap(), Rgb::new(255, 136, 0));
        assert_eq!(" Blue ".parse::<Rgb>().unwrap(), Rgb::new(0, 0, 255));
        for bad in ["", "#", "#12345", "#gg0000", "ff00ff00", "teal", "#ff 000"] {
            assert!(bad.parse::<Rgb>().is_err(), "{bad:?}");
        }
        // multibyte input must not panic on slicing
        assert!("#ééé".parse::<Rgb>().is_err());
    }

    #[test]
    fn json_is_a_hex_string() {
        let c = Rgb::new(1, 2, 255);
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"#0102ff\"");
        assert_eq!(serde_json::from_str::<Rgb>("\"#0102ff\"").unwrap(), c);
        assert!(serde_json::from_str::<Rgb>("\"nope\"").is_err());
        assert_eq!(c.to_string(), "#0102ff");
    }
}
