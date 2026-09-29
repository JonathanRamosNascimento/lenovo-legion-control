//! Linux implementation of the Legion Control hardware layer.
//!
//! * [`system`] — DMI / OS / kernel / desktop detection;
//! * [`memory`] — `/proc/meminfo`;
//! * [`LinuxController`] — the [`HardwareController`] implementation that
//!   composes the per-subsystem crates;
//! * [`capabilities`] — feature detection with evidence;
//! * [`doctor`] — `legionctl doctor`;
//! * [`report`] — `legionctl hardware-scan` JSON report.
//!
//! All hardware writes are validated here and executed through
//! `legion_core::sysfs::write_attr`.

pub mod capabilities;
pub mod config;
pub mod controller;
pub mod custom;
pub mod doctor;
pub mod memory;
pub mod report;
pub mod system;

pub use config::Config;
pub use controller::{ControllerOptions, LinuxController};
pub use legion_core::HardwareController;

#[cfg(any(test, feature = "testing"))]
pub mod fixtures;
