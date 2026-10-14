//! `legionctl` — command line interface for Legion Control.
//!
//! Reads come straight from sysfs/procfs (no privileges needed). Changes go
//! through `legion-control-daemon` when it runs (polkit-authorised), or are
//! written directly when running as root (see [`backend`]).

mod backend;
mod cli;
mod render;

use std::io::{Read, Write};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Serialize;

use legion_core::doctor::CheckStatus;
use legion_core::model::{DaemonInfo, FanCurve, FanMode};
use legion_core::{HardwareController, PowerProfile, SysRoot};
use legion_hardware::{ControllerOptions, LinuxController, capabilities, doctor, report};
use legion_profiles::CustomProfile;
use legion_profiles::store::ProfileStore;

use backend::Backend;
use cli::{
    BatteryCmd, Cli, Command, ConfigCmd, CpuCmd, CustomCmd, DaemonCmd, FanCmd, GpuCmd, KeyboardCmd,
    PowerLimitCmd,
};
use legion_i18n::tr;

fn init_logging(verbose: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

fn controller(cli: &Cli, wake_gpu: bool) -> LinuxController {
    let mut opts = match &cli.sysroot {
        Some(p) => ControllerOptions::fixture(SysRoot::new(p)),
        None => ControllerOptions::host(),
    };
    opts.wake_gpu = wake_gpu;
    LinuxController::new(opts)
}

fn emit<T: Serialize>(json: bool, value: &T, text: impl FnOnce(&T) -> String) -> Result<()> {
    let mut out = std::io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, value)?;
        writeln!(out)?;
    } else {
        write!(out, "{}", text(value))?;
    }
    Ok(())
}

/// Reports a successful change.
fn done(json: bool, backend: &Backend, what: &str) -> Result<()> {
    emit(
        json,
        &serde_json::json!({ "ok": true, "change": what, "via": backend.describe() }),
        |_| format!("{} (via {})\n", tr(what), tr(backend.describe())),
    )
}

fn read_input(path: &std::path::Path) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        // a user-chosen input file, read with the user's own permissions
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
    }
}

/// Output of `daemon status`.
#[derive(Debug, Serialize)]
pub struct DaemonReport {
    /// Whether the daemon answered.
    pub running: bool,
    /// Daemon state.
    pub info: Option<DaemonInfo>,
}

fn run(cli: Cli) -> Result<ExitCode> {
    let json = cli.json;
    match &cli.command {
        Command::Status => {
            let s = controller(&cli, false).get_status()?;
            emit(json, &s, render::status)?;
        }
        Command::Profile { profile } => {
            if let Some(p) = profile {
                let target: PowerProfile = p.parse()?;
                let b = Backend::select(&cli);
                b.set_profile(target)?;
                done(json, &b, &format!("Power profile set to {target}"))?;
            } else {
                emit(
                    json,
                    &controller(&cli, false).get_power_profile()?,
                    render::profile,
                )?;
            }
        }
        Command::Cpu { cmd } => match cmd {
            CpuCmd::Status => emit(json, &controller(&cli, false).get_cpu_info()?, render::cpu)?,
            CpuCmd::Epp { value } => {
                let b = Backend::select(&cli);
                b.set_epp(value)?;
                done(json, &b, &format!("EPP set to {value}"))?;
            }
            CpuCmd::Governor { value } => {
                let b = Backend::select(&cli);
                b.set_governor(value)?;
                done(json, &b, &format!("Governor set to {value}"))?;
            }
            CpuCmd::Turbo { state } => {
                let b = Backend::select(&cli);
                b.set_turbo(state.enabled())?;
                let s = if state.enabled() {
                    "enabled"
                } else {
                    "disabled"
                };
                done(json, &b, &format!("Turbo {s}"))?;
            }
        },
        Command::Gpu {
            cmd: GpuCmd::Status { wake_gpu },
        } => {
            emit(
                json,
                &controller(&cli, *wake_gpu).get_gpu_info()?,
                render::gpu,
            )?;
        }
        Command::Fan { cmd } => match cmd {
            FanCmd::Status => {
                let info = controller(&cli, false).get_fan_info()?;
                let daemon = if cli.sysroot.is_some() {
                    None
                } else {
                    Backend::select(&cli).daemon_info()
                };
                let v = serde_json::json!({ "fans": info, "daemon": daemon });
                emit(json, &v, |_| render::fans(&info, daemon.as_ref()))?;
            }
            FanCmd::Auto => {
                let b = Backend::select(&cli);
                b.set_fan_mode(&FanMode::Auto)?;
                done(json, &b, "Fan control returned to the firmware")?;
            }
            FanCmd::Fixed { rpm } => {
                let b = Backend::select(&cli);
                b.set_fan_mode(&FanMode::Fixed { rpm: *rpm })?;
                done(json, &b, &format!("Fans fixed at {rpm} RPM"))?;
            }
            FanCmd::Curve { file, example } => {
                if *example {
                    let ex = legion_fan::example_curve();
                    println!("{}", serde_json::to_string_pretty(&ex)?);
                    return Ok(ExitCode::SUCCESS);
                }
                let path = file.as_ref().context("a curve file is required")?;
                let curve: FanCurve =
                    serde_json::from_str(&read_input(path)?).context("invalid fan curve JSON")?;
                legion_fan::validate_curve(&curve)?;
                let b = Backend::select(&cli);
                b.set_fan_mode(&FanMode::Curve(curve))?;
                done(json, &b, "Fan curve activated")?;
            }
        },
        Command::Battery { cmd } => match cmd {
            None | Some(BatteryCmd::Status) => emit(
                json,
                &controller(&cli, false).get_battery_info()?,
                render::battery,
            )?,
            Some(BatteryCmd::Conservation { state }) => {
                let b = Backend::select(&cli);
                b.set_conservation(state.enabled())?;
                let s = if state.enabled() {
                    "enabled"
                } else {
                    "disabled"
                };
                done(json, &b, &format!("Conservation mode {s}"))?;
            }
            Some(BatteryCmd::ChargeType { value }) => {
                let b = Backend::select(&cli);
                b.set_charge_type(value)?;
                done(json, &b, &format!("Charge type set to {value}"))?;
            }
            Some(BatteryCmd::Threshold { value }) => {
                let v = legion_battery::validate_threshold(*value, 1)?;
                let info = controller(&cli, false).get_battery_info()?;
                if info
                    .batteries
                    .iter()
                    .all(|b| b.charge_end_threshold.is_none())
                {
                    let hint = if info.conservation_mode.is_some() {
                        " This machine exposes a fixed conservation mode instead \
                         (`legionctl battery conservation on`)."
                    } else {
                        ""
                    };
                    bail!("Battery charge threshold is not supported on this hardware.{hint}");
                }
                let b = Backend::select(&cli);
                b.set_threshold(v)?;
                done(json, &b, &format!("Charge threshold set to {v}%"))?;
            }
        },
        Command::Keyboard { cmd } => match cmd {
            None | Some(KeyboardCmd::Status) => {
                let info = Backend::select(&cli).keyboard_info()?;
                emit(json, &info, render::keyboard)?;
            }
            Some(KeyboardCmd::Color { color }) => {
                let b = Backend::select(&cli);
                b.set_keyboard_color(*color)?;
                done(json, &b, &format!("Keyboard colour set to {color}"))?;
            }
            Some(KeyboardCmd::Brightness { level }) => {
                let b = Backend::select(&cli);
                b.set_keyboard_brightness(*level)?;
                done(json, &b, &format!("Keyboard brightness set to {level}"))?;
            }
            Some(KeyboardCmd::Restore) => {
                let b = Backend::select(&cli);
                b.restore_keyboard()?;
                done(json, &b, "Keyboard effect restored")?;
            }
        },
        Command::PowerLimit { cmd } => match cmd {
            None | Some(PowerLimitCmd::List) => {
                let pl = Backend::select(&cli).power_limits()?;
                emit(json, &pl, render::power_limits)?;
            }
            Some(PowerLimitCmd::Set { name, value }) => {
                let b = Backend::select(&cli);
                b.set_firmware_attribute(name, *value)?;
                done(json, &b, &format!("{name} set to {value}"))?;
            }
        },
        Command::Custom { cmd } => return custom(&cli, cmd),
        Command::Config { cmd } => match cmd {
            None | Some(ConfigCmd::Show) => {
                let root = match &cli.sysroot {
                    Some(p) => SysRoot::new(p),
                    None => SysRoot::host(),
                };
                let cfg = legion_hardware::Config::load(&root);
                emit(json, &cfg, render::config)?;
            }
            Some(ConfigCmd::Set { name, state }) => {
                let b = Backend::select(&cli);
                b.set_experimental(name, state.enabled())?;
                let s = if state.enabled() {
                    "enabled"
                } else {
                    "disabled"
                };
                done(json, &b, &format!("{name} {s}"))?;
            }
        },
        Command::Thermal => {
            emit(
                json,
                &controller(&cli, false).get_temperatures()?,
                render::thermal,
            )?;
        }
        Command::Hardware => {
            emit(
                json,
                &controller(&cli, false).get_system_info()?,
                render::system,
            )?;
        }
        Command::Capabilities => {
            let caps = capabilities::detect(&controller(&cli, false));
            emit(json, &caps, |c| render::capabilities(c))?;
        }
        Command::HardwareScan { output } => {
            let c = controller(&cli, false);
            let r = report::build(&c);
            let text = serde_json::to_string_pretty(&r)? + "\n";
            if output.as_os_str() == "-" {
                print!("{text}");
            } else {
                std::fs::write(output, text)
                    .with_context(|| format!("writing {}", output.display()))?;
                if json {
                    emit(true, &serde_json::json!({ "report": output }), |_| {
                        String::new()
                    })?;
                } else {
                    print!("{}", render::scan_summary(&r));
                    println!(
                        "\n{}",
                        tr(&format!("Report written to {}", output.display()))
                    );
                }
            }
        }
        Command::Doctor => {
            let r = doctor::run(&controller(&cli, false));
            emit(json, &r, render::doctor)?;
            if r.overall() == CheckStatus::Fail {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Daemon {
            cmd: DaemonCmd::Status,
        } => {
            let info = if cli.sysroot.is_some() {
                None
            } else {
                Backend::select(&cli).daemon_info()
            };
            let r = DaemonReport {
                running: info.is_some(),
                info,
            };
            emit(json, &r, render::daemon)?;
        }
        Command::Version => {
            let v = serde_json::json!({
                "name": "legionctl",
                "version": env!("CARGO_PKG_VERSION"),
            });
            emit(json, &v, |_| {
                format!("legionctl {} (Legion Control)\n", env!("CARGO_PKG_VERSION"))
            })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn custom(cli: &Cli, cmd: &CustomCmd) -> Result<ExitCode> {
    let json = cli.json;
    let store = ProfileStore::user()?;
    match cmd {
        CustomCmd::List => {
            let names = store.list();
            let v = serde_json::json!({ "directory": store.dir(), "profiles": names });
            emit(json, &v, |_| render::custom_list(store.dir(), &names))?;
        }
        CustomCmd::Show { name } => {
            let p = store.load(name)?;
            emit(json, &p, |p| {
                serde_json::to_string_pretty(p).unwrap_or_default() + "\n"
            })?;
        }
        CustomCmd::Save { name, description } => {
            legion_profiles::custom::validate_name(name)?;
            let mut p = Backend::select(cli).capture()?;
            p.description = description.clone();
            let path = store.save(name, &p)?;
            emit(
                json,
                &serde_json::json!({ "saved": path, "profile": p }),
                |_| {
                    format!(
                        "{}\n",
                        tr(&format!("Saved current settings to {}", path.display()))
                    )
                },
            )?;
        }
        CustomCmd::Import { name, file } => {
            let p = CustomProfile::from_json(&read_input(file)?)?;
            let c = controller(cli, false);
            p.validate(&legion_hardware::custom::constraints(&c))
                .context("profile does not match this hardware")?;
            let path = store.save(name, &p)?;
            emit(json, &serde_json::json!({ "saved": path }), |_| {
                format!(
                    "{}\n",
                    tr(&format!("Imported profile to {}", path.display()))
                )
            })?;
        }
        CustomCmd::Apply { name } => {
            let p = store.load(name)?;
            let b = Backend::select(cli);
            let changes = b.apply_profile(&p)?;
            emit(json, &changes, |c| {
                let mut s = format!(
                    "{}\n",
                    tr(&format!("Applied profile {name} (via {}):", b.describe()))
                );
                for l in c {
                    s.push_str(&format!("  {}\n", tr(l)));
                }
                s
            })?;
        }
        CustomCmd::Delete { name } => {
            store.delete(name)?;
            emit(json, &serde_json::json!({ "deleted": name }), |_| {
                format!("{}\n", tr(&format!("Deleted profile {name}")))
            })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match cli.lang.as_deref() {
        None | Some("auto") => legion_i18n::set_lang(legion_i18n::detect()),
        Some(code) => match legion_i18n::Lang::parse(code) {
            Some(l) => legion_i18n::set_lang(l),
            None => {
                eprintln!("legionctl: unsupported language {code:?} (available: en, pt-BR)");
                return ExitCode::from(2);
            }
        },
    }
    let json = cli.json;
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            if json {
                let _ = emit(
                    true,
                    &serde_json::json!({ "error": "failed", "message": format!("{e:#}") }),
                    |_| String::new(),
                );
            } else {
                eprintln!("legionctl: {}", tr(&format!("{e:#}")));
            }
            ExitCode::FAILURE
        }
    }
}
