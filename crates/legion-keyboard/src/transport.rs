//! Feature-report transport.
//!
//! [`FeatureTransport`] is the only way bytes reach a keyboard. The real
//! implementation ([`HidrawOpener`]) uses the `hidraw` character device with
//! `HIDIOCSFEATURE` / `HIDIOCGFEATURE` through the `hidapi` crate, which keeps
//! this workspace free of `unsafe` code. Tests and `--sysroot` fixtures use
//! [`crate::sim::SimOpener`].

use std::ffi::CString;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use legion_core::{HwError, Result};

use crate::discover::HidNode;

/// Sends and receives HID *feature* reports of one device.
pub trait FeatureTransport {
    /// `GET_REPORT(Feature)`. `len` is the expected length **including** the
    /// report id; the reply also starts with the report id.
    fn get_feature(&mut self, report_id: u8, len: usize) -> Result<Vec<u8>>;

    /// `SET_REPORT(Feature)`. `report` starts with the report id.
    fn set_feature(&mut self, report: &[u8]) -> Result<()>;
}

impl<T: FeatureTransport + ?Sized> FeatureTransport for Box<T> {
    fn get_feature(&mut self, report_id: u8, len: usize) -> Result<Vec<u8>> {
        (**self).get_feature(report_id, len)
    }

    fn set_feature(&mut self, report: &[u8]) -> Result<()> {
        (**self).set_feature(report)
    }
}

/// Opens the transport of a HID node.
pub trait Opener: Send + Sync + fmt::Debug {
    /// Opens `node` for feature reports.
    fn open(&self, node: &HidNode) -> Result<Box<dyn FeatureTransport>>;
}

/// Opens the real `/dev/hidrawN` nodes.
#[derive(Debug, Clone, Copy, Default)]
pub struct HidrawOpener;

fn backend(e: impl fmt::Display) -> HwError {
    HwError::Backend {
        backend: "hidraw",
        message: e.to_string(),
    }
}

/// Process-wide `hidapi` context (creating one enumerates every HID device,
/// so it is done once).
fn api() -> Result<&'static Mutex<hidapi::HidApi>> {
    static API: OnceLock<std::result::Result<Mutex<hidapi::HidApi>, String>> = OnceLock::new();
    API.get_or_init(|| {
        hidapi::HidApi::new()
            .map(Mutex::new)
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(backend)
}

impl Opener for HidrawOpener {
    fn open(&self, node: &HidNode) -> Result<Box<dyn FeatureTransport>> {
        let path = node.dev_path();
        // `hidapi` reports failures as text only; probe first so that a
        // permission problem maps to the usual "administrator permission".
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| HwError::from_io(&path, e))?;
        let c_path = CString::new(path.clone()).map_err(backend)?;
        let api = api()?
            .lock()
            .map_err(|_| backend("hidapi context poisoned"))?;
        let dev = api.open_path(&c_path).map_err(backend)?;
        tracing::debug!(path, "hidraw opened");
        Ok(Box::new(Hidraw { dev }))
    }
}

struct Hidraw {
    dev: hidapi::HidDevice,
}

impl FeatureTransport for Hidraw {
    fn get_feature(&mut self, report_id: u8, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        buf[0] = report_id;
        let n = self.dev.get_feature_report(&mut buf).map_err(backend)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn set_feature(&mut self, report: &[u8]) -> Result<()> {
        self.dev.send_feature_report(report).map_err(backend)
    }
}
