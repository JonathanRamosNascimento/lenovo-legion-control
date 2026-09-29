//! Core building blocks for Legion Control.
//!
//! This crate contains:
//!
//! * the data model shared by every other crate ([`model`]),
//! * the [`HardwareController`] abstraction,
//! * capability reporting ([`capabilities`]),
//! * the per-model support matrix ([`support`]),
//! * a **read-only** filesystem access layer rooted at a configurable path
//!   ([`sysfs::SysRoot`]) so that every parser can be tested against a fake
//!   `/sys` + `/proc` tree without touching real hardware,
//! * generic hwmon enumeration ([`hwmon`]).
//!
//! Nothing in this crate writes to the filesystem (the optional `testing`
//! feature only writes into temporary directories created by tests).

pub mod capabilities;
pub mod controller;
pub mod doctor;
pub mod error;
pub mod hwmon;
pub mod model;
pub mod profile;
pub mod support;
pub mod sysfs;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use capabilities::{Capability, CapabilityState, HardwareCapabilities};
pub use controller::HardwareController;
pub use error::{HwError, Result};
pub use model::Metric;
pub use profile::PowerProfile;
pub use sysfs::SysRoot;
