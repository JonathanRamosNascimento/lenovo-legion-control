//! Command line definition.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use legion_core::keyboard::Rgb;

/// Monitor and control Lenovo Legion laptops on Linux.
#[derive(Debug, Parser)]
#[command(name = "legionctl", version, about, long_about = None)]
pub struct Cli {
    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Increase log verbosity (-v info, -vv debug, -vvv trace). RUST_LOG overrides.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Read/write sysfs in a fixture directory instead of `/`
    /// (development and tests only; disables D-Bus and NVML).
    #[arg(long, global = true, hide = true, value_name = "DIR")]
    pub sysroot: Option<PathBuf>,

    /// Output language: `en`, `pt-BR` (default: from LEGION_LANG / LANG).
    #[arg(long, global = true, value_name = "LANG")]
    pub lang: Option<String>,

    /// Talk to a daemon on the session bus (development only).
    #[arg(long, global = true, hide = true)]
    pub session: bool,

    /// Command.
    #[command(subcommand)]
    pub command: Command,
}

/// `on` / `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OnOff {
    /// Enable.
    On,
    /// Disable.
    Off,
}

impl OnOff {
    /// As bool.
    pub fn enabled(self) -> bool {
        self == OnOff::On
    }
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Overview of profile, CPU, GPU, temperatures, fans, memory and battery.
    Status,
    /// Show or change the power profile.
    Profile {
        /// quiet | balanced | performance | max-power | custom
        profile: Option<String>,
    },
    /// CPU information and tuning.
    Cpu {
        #[command(subcommand)]
        cmd: CpuCmd,
    },
    /// GPU information.
    Gpu {
        #[command(subcommand)]
        cmd: GpuCmd,
    },
    /// Fan information and control.
    Fan {
        #[command(subcommand)]
        cmd: FanCmd,
    },
    /// Battery information and charging control.
    Battery {
        #[command(subcommand)]
        cmd: Option<BatteryCmd>,
    },
    /// Built-in keyboard backlight colour and brightness (experimental).
    Keyboard {
        #[command(subcommand)]
        cmd: Option<KeyboardCmd>,
    },
    /// Firmware power / thermal limits (lenovo-wmi-other).
    PowerLimit {
        #[command(subcommand)]
        cmd: Option<PowerLimitCmd>,
    },
    /// Daemon configuration (/etc/legion-control/daemon.json).
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCmd>,
    },
    /// Saved custom profiles (~/.config/legion-control/profiles).
    Custom {
        #[command(subcommand)]
        cmd: CustomCmd,
    },
    /// Temperatures.
    Thermal,
    /// System identification (DMI, BIOS, OS, kernel).
    Hardware,
    /// Detected capabilities with evidence.
    Capabilities,
    /// Write a full read-only discovery report as JSON.
    HardwareScan {
        /// Output file (`-` for stdout).
        #[arg(short, long, default_value = "hardware-report.json")]
        output: PathBuf,
    },
    /// Run all diagnostics.
    Doctor,
    /// Legion Control daemon.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
    /// Print version.
    Version,
}

/// `cpu` subcommands.
#[derive(Debug, Subcommand)]
pub enum CpuCmd {
    /// Current CPU status.
    Status,
    /// Set the energy performance preference on all cores.
    Epp {
        /// One of the values listed by `cpu status`.
        value: String,
    },
    /// Set the cpufreq governor on all cores.
    Governor {
        /// One of the values listed by `cpu status`.
        value: String,
    },
    /// Enable or disable turbo boost.
    Turbo {
        /// on | off
        state: OnOff,
    },
}

/// `gpu` subcommands.
#[derive(Debug, Subcommand)]
pub enum GpuCmd {
    /// Current GPU status.
    Status {
        /// Query NVML even when the dGPU is suspended (wakes it up, costs battery).
        #[arg(long)]
        wake_gpu: bool,
    },
}

/// `fan` subcommands.
#[derive(Debug, Subcommand)]
pub enum FanCmd {
    /// Current fan speeds and mode.
    Status,
    /// Give fan control back to the firmware.
    Auto,
    /// Fixed target speed on all fans (experimental).
    Fixed {
        /// Target RPM (within the range shown by `fan status`).
        rpm: u32,
    },
    /// Software fan curve evaluated by the daemon (experimental).
    Curve {
        /// JSON file with the curve (`-` for stdin). See `fan curve --example`.
        #[arg(required_unless_present = "example")]
        file: Option<PathBuf>,
        /// Print an example curve and exit.
        #[arg(long)]
        example: bool,
    },
}

/// `battery` subcommands.
#[derive(Debug, Subcommand)]
pub enum BatteryCmd {
    /// Battery status (default).
    Status,
    /// Enable or disable the firmware conservation mode (charge limit).
    Conservation {
        /// on | off
        state: OnOff,
    },
    /// Select a charge type (e.g. Fast, Standard, Long_Life).
    ChargeType {
        /// One of the values listed by `battery`.
        value: String,
    },
    /// Set the numeric charge threshold (only where supported).
    Threshold {
        /// Percentage (1-100).
        value: u8,
    },
}

/// `keyboard` subcommands.
#[derive(Debug, Subcommand)]
pub enum KeyboardCmd {
    /// Backlight state (default).
    Status,
    /// One steady colour on the whole keyboard (stored in the keyboard).
    Color {
        /// `#rrggbb`, `rrggbb`, `#rgb` or a name (white, red, orange, yellow,
        /// green, cyan, blue, purple, pink). Quote `#` in the shell.
        color: Rgb,
    },
    /// Backlight brightness level (0 = off, see `keyboard status` for the maximum).
    Brightness {
        /// Level.
        level: u8,
    },
    /// Put back the effect the keyboard had before the first colour change.
    Restore,
}

/// `power-limit` subcommands.
#[derive(Debug, Subcommand)]
pub enum PowerLimitCmd {
    /// List limits with their allowed ranges (default).
    List,
    /// Set a limit (requires the custom profile; experimental).
    Set {
        /// Attribute name, e.g. ppt_pl1_spl.
        name: String,
        /// New value.
        value: i64,
    },
}

/// `config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Show the configuration (default).
    Show,
    /// Enable/disable an experimental feature (asks for the administrator password).
    Set {
        /// enable_fan_control | enable_power_limits | enable_keyboard_rgb
        name: String,
        /// on | off
        state: OnOff,
    },
}

/// `custom` subcommands.
#[derive(Debug, Subcommand)]
pub enum CustomCmd {
    /// List saved profiles.
    List,
    /// Show a saved profile.
    Show {
        /// Profile name.
        name: String,
    },
    /// Save the current settings as a profile.
    Save {
        /// Profile name ([A-Za-z0-9_-], max 64).
        name: String,
        /// Description.
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Import a profile from a JSON file (`-` for stdin); it is validated.
    Import {
        /// Profile name.
        name: String,
        /// JSON file.
        file: PathBuf,
    },
    /// Apply a saved profile.
    Apply {
        /// Profile name.
        name: String,
    },
    /// Delete a saved profile.
    Delete {
        /// Profile name.
        name: String,
    },
}

/// `daemon` subcommands.
#[derive(Debug, Subcommand)]
pub enum DaemonCmd {
    /// Whether the daemon is running, and its state.
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_cli() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_commands() {
        let c = Cli::try_parse_from(["legionctl", "--json", "doctor"]).unwrap();
        assert!(c.json);
        assert!(matches!(c.command, Command::Doctor));
        let c = Cli::try_parse_from(["legionctl", "gpu", "status", "--wake-gpu"]).unwrap();
        assert!(matches!(
            c.command,
            Command::Gpu {
                cmd: GpuCmd::Status { wake_gpu: true }
            }
        ));
        let c = Cli::try_parse_from(["legionctl", "profile", "quiet", "--json"]).unwrap();
        assert!(matches!(c.command, Command::Profile { profile: Some(ref p) } if p == "quiet"));
        let c = Cli::try_parse_from(["legionctl", "battery", "conservation", "off"]).unwrap();
        assert!(matches!(
            c.command,
            Command::Battery {
                cmd: Some(BatteryCmd::Conservation { state: OnOff::Off })
            }
        ));
        let c = Cli::try_parse_from(["legionctl", "fan", "fixed", "3000"]).unwrap();
        assert!(matches!(
            c.command,
            Command::Fan {
                cmd: FanCmd::Fixed { rpm: 3000 }
            }
        ));
        let c = Cli::try_parse_from(["legionctl", "fan", "curve", "--example"]).unwrap();
        assert!(matches!(
            c.command,
            Command::Fan {
                cmd: FanCmd::Curve { example: true, .. }
            }
        ));
        let c =
            Cli::try_parse_from(["legionctl", "power-limit", "set", "ppt_pl1_spl", "90"]).unwrap();
        assert!(matches!(
            c.command,
            Command::PowerLimit {
                cmd: Some(PowerLimitCmd::Set { .. })
            }
        ));
        let c = Cli::try_parse_from(["legionctl", "--lang", "pt-BR", "status"]).unwrap();
        assert_eq!(c.lang.as_deref(), Some("pt-BR"));
        let c = Cli::try_parse_from(["legionctl", "custom", "save", "gaming", "-d", "x"]).unwrap();
        assert!(matches!(
            c.command,
            Command::Custom {
                cmd: CustomCmd::Save { .. }
            }
        ));
    }

    #[test]
    fn parses_keyboard() {
        let c = Cli::try_parse_from(["legionctl", "keyboard", "color", "#ff8000"]).unwrap();
        assert!(matches!(
            c.command,
            Command::Keyboard {
                cmd: Some(KeyboardCmd::Color { color }),
            } if color == Rgb::new(255, 128, 0)
        ));
        let c = Cli::try_parse_from(["legionctl", "keyboard", "brightness", "5"]).unwrap();
        assert!(matches!(
            c.command,
            Command::Keyboard {
                cmd: Some(KeyboardCmd::Brightness { level: 5 })
            }
        ));
        assert!(Cli::try_parse_from(["legionctl", "keyboard", "restore"]).is_ok());
        assert!(matches!(
            Cli::try_parse_from(["legionctl", "keyboard"])
                .unwrap()
                .command,
            Command::Keyboard { cmd: None }
        ));
        for bad in [
            &["keyboard", "color", "teal"][..],
            &["keyboard", "color"],
            &["keyboard", "brightness", "300"],
            &["keyboard", "brightness", "-1"],
        ] {
            let mut a = vec!["legionctl"];
            a.extend_from_slice(bad);
            assert!(Cli::try_parse_from(a).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_config() {
        let c = Cli::try_parse_from(["legionctl", "config", "set", "enable_power_limits", "on"])
            .unwrap();
        assert!(matches!(
            c.command,
            Command::Config {
                cmd: Some(ConfigCmd::Set {
                    state: OnOff::On,
                    ..
                })
            }
        ));
        assert!(Cli::try_parse_from(["legionctl", "config", "set", "x", "maybe"]).is_err());
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(Cli::try_parse_from(["legionctl", "battery", "threshold", "300"]).is_err());
        assert!(Cli::try_parse_from(["legionctl", "battery", "threshold", "-5"]).is_err());
        assert!(Cli::try_parse_from(["legionctl", "battery", "conservation", "maybe"]).is_err());
        assert!(Cli::try_parse_from(["legionctl", "fan", "fixed", "-1"]).is_err());
        assert!(Cli::try_parse_from(["legionctl", "fan", "curve"]).is_err());
        assert!(Cli::try_parse_from(["legionctl", "cpu", "turbo", "2"]).is_err());
        assert!(Cli::try_parse_from(["legionctl"]).is_err());
    }
}
