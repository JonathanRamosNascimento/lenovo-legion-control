//! Manual control of the built-in keyboard, through the same code the daemon
//! uses (`legion_keyboard::Keyboard`).
//!
//! ```sh
//! sudo target/debug/examples/spectrum-ctl status             # read-only
//! sudo target/debug/examples/spectrum-ctl color '#ff0000'    # steady colour (writes a backup first)
//! sudo target/debug/examples/spectrum-ctl restore            # put the backed-up effect back
//! sudo target/debug/examples/spectrum-ctl brightness 5       # 0..=9
//! sudo target/debug/examples/spectrum-ctl profile 1          # activate profile 1..=6
//! ```
//!
//! The backup is `keyboard-backup.hex` in the current directory.

use std::path::Path;
use std::sync::Arc;

use legion_core::SysRoot;
use legion_core::keyboard::Rgb;
use legion_keyboard::spectrum::{self, Spectrum};
use legion_keyboard::{Detection, HidrawOpener, Keyboard, Opener};

const BACKUP: &str = "keyboard-backup.hex";

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let node = match legion_keyboard::detect(&SysRoot::host()) {
        Detection::Found(n) | Detection::Unverified(n) => *n,
        other => return println!("no usable controller: {other:?}"),
    };
    let kb = Keyboard::new(node.clone(), Arc::new(HidrawOpener));
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("status") => match kb.snapshot() {
            Ok(s) => {
                println!("brightness {} / {}", s.brightness, spectrum::MAX_BRIGHTNESS);
                println!("active profile {}", s.profile);
                println!("keys {:?}", s.key_count);
                for e in &s.effect.effects {
                    println!(
                        "effect #{}: {} colours {:?} keys {}",
                        e.number,
                        spectrum::effect::name(e.kind),
                        e.colors.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        e.keys.len()
                    );
                }
            }
            Err(e) => println!("ERROR {e}"),
        },
        Some("color") => {
            let Some(color) = arg(1).and_then(|c| c.parse::<Rgb>().ok()) else {
                return println!("usage: color <#rrggbb|name>");
            };
            // keep the FIRST backup: later colours must not overwrite the original
            let save = |_profile: u8, raw: &[u8]| {
                if Path::new(BACKUP).exists() {
                    println!("{BACKUP} already exists; kept");
                } else {
                    std::fs::write(BACKUP, to_hex(raw)).map_err(|e| {
                        legion_core::HwError::InvalidValue(format!("cannot save the backup: {e}"))
                    })?;
                    println!("backup saved to {BACKUP}");
                }
                Ok(())
            };
            match kb.set_static_color(color, save) {
                Ok(a) => println!(
                    "colour {color} on profile {} (written: {})",
                    a.profile, a.changed
                ),
                Err(e) => println!("ERROR {e}"),
            }
        }
        Some("restore") => {
            let Some(raw) = std::fs::read_to_string(BACKUP)
                .ok()
                .and_then(|t| from_hex(&t))
            else {
                return println!("no readable {BACKUP}");
            };
            println!("restore: {:?}", kb.restore(&raw));
        }
        Some("brightness") => match arg(1).and_then(|v| v.parse::<u8>().ok()) {
            Some(n) => println!("brightness {n}: {:?}", kb.set_brightness(n)),
            None => println!("usage: brightness <0-9>"),
        },
        Some("profile") => match arg(1).and_then(|v| v.parse::<u8>().ok()) {
            Some(n) => {
                let r = HidrawOpener
                    .open(&node)
                    .map(Spectrum::new)
                    .and_then(|mut s| s.select_profile(n));
                println!("profile {n}: {r:?}");
            }
            None => println!("usage: profile <1-6>"),
        },
        _ => println!(
            "usage: spectrum-ctl status | color <c> | restore | brightness <n> | profile <n>"
        ),
    }
}
