//! Backlight colour of the built-in keyboard through the Lenovo **Spectrum**
//! vendor protocol of the ITE 8258 controller (experimental).
//!
//! * [`hid`] parses the device's report descriptor;
//! * [`spectrum`] implements the protocol and refuses to talk to a device
//!   whose descriptor does not match it (read its module docs for the
//!   provenance of the protocol);
//! * [`keyboard`] is the high-level API (snapshot, colour, brightness, restore);
//! * [`discover`] finds the built-in controller in sysfs (no privileges
//!   needed);
//! * [`transport`] sends feature reports through `hidraw` (root only);
//! * [`sim`] is a simulated controller for tests and fixture roots.

pub mod descriptors;
pub mod discover;
pub mod hid;
pub mod keyboard;
pub mod sim;
pub mod spectrum;
pub mod transport;

pub use discover::{Detection, HidNode, NodeSummary, detect, summarize};
pub use keyboard::Keyboard;
pub use spectrum::{Description, Effect, Spectrum};
pub use transport::{FeatureTransport, HidrawOpener, Opener};
