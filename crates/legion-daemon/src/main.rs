//! `legion-control-daemon` — privileged D-Bus service of Legion Control.
//!
//! * Owns `com.legion.Control` on the system bus and serves
//!   `com.legion.Control.Manager` at `/com/legion/Control`.
//! * Monitors hardware (CPU/temperatures 1 s, GPU 2 s, battery 10 s) and
//!   emits `ProfileChanged`, `TemperatureChanged`, `BatteryChanged` and
//!   `HardwareChanged`.
//! * Authorises every change with polkit and validates every value against
//!   the hardware before writing.
//! * Runs manual fan control / curves (experimental, disabled by default)
//!   and gives control back to the firmware on overheating or exit.
//!
//! Logs go to stderr (journald under systemd). Every hardware change is
//! logged under the `legion_audit` target.

mod manager;
mod polkit;
mod worker;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;

use legion_core::SysRoot;
use legion_dbus::{BUS_NAME, OBJECT_PATH};
use legion_hardware::{ControllerOptions, LinuxController};

use manager::Manager;
use polkit::Policy;
use worker::{Hw, Worker};

/// Legion Control daemon.
#[derive(Debug, Parser)]
#[command(name = "legion-control-daemon", version, about)]
struct Args {
    /// Use the session bus (development only).
    #[arg(long)]
    session: bool,
    /// Skip polkit checks (only allowed together with --session).
    #[arg(long, requires = "session")]
    no_polkit: bool,
    /// Serve a fixture tree instead of the real /sys (development only;
    /// requires --session).
    #[arg(long, requires = "session", value_name = "DIR")]
    sysroot: Option<PathBuf>,
    /// Monitoring interval in milliseconds.
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(250..=10_000))]
    interval_ms: u64,
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,zbus=warn,tracing=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .without_time() // journald timestamps
        .try_init();
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    init_logging();
    let args = Args::parse();

    let euid = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|v| v.split_whitespace().nth(1)?.parse::<u32>().ok())
        });
    if !args.session && euid != Some(0) {
        bail!("must run as root on the system bus (use --session for development)");
    }

    let opts = match &args.sysroot {
        Some(dir) => ControllerOptions::fixture(SysRoot::new(dir)),
        None => {
            let mut o = ControllerOptions::host();
            o.sample_interval = Duration::ZERO;
            o.desktop = None;
            o
        }
    };
    let config = opts.config.clone();
    let ctrl = LinuxController::new(opts);
    let policy = if args.no_polkit {
        tracing::warn!("polkit disabled (development mode)");
        Policy::AllowAll
    } else {
        Policy::Polkit
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        fan_control = config.enable_fan_control,
        power_limits = config.enable_power_limits,
        "starting"
    );

    let hw = Hw::spawn(Worker::new(ctrl, policy == Policy::Polkit))
        .context("starting hardware thread")?;
    hw.run(|w| {
        w.tick(true);
    })
    .await;

    let builder = if args.session {
        zbus::connection::Builder::session()?
    } else {
        zbus::connection::Builder::system()?
    };
    let conn = builder
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, Manager::new(hw.clone(), policy))?
        .build()
        .await
        .context("connecting to D-Bus / acquiring com.legion.Control")?;
    tracing::info!("serving {BUS_NAME} at {OBJECT_PATH}");

    let iface = conn
        .object_server()
        .interface::<_, Manager>(OBJECT_PATH)
        .await?;
    let monitor = {
        let hw = hw.clone();
        let iface = iface.clone();
        async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(args.interval_ms));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let ev = hw.run(|w| w.tick(true)).await;
                Manager::emit(iface.signal_emitter(), &ev).await;
            }
        }
    };

    tokio::select! {
        _ = monitor => {}
        _ = shutdown_signal() => tracing::info!("stopping"),
    }
    hw.run(|w| w.shutdown()).await;
    Ok(())
}
