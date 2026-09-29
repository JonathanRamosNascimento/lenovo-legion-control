//! StatusNotifierItem tray icon (optional; the app works without it).
//!
//! On GNOME this requires the "AppIndicator and KStatusNotifierItem
//! Support" extension; if no tray host is available, spawning fails and the
//! window is used instead.

use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};

use legion_core::PowerProfile;

use crate::AppEvent;

struct Tray {
    tx: async_channel::Sender<AppEvent>,
}

impl Tray {
    fn item(&self, label: &str, ev: AppEvent) -> MenuItem<Self> {
        StandardItem {
            label: legion_i18n::tr(label),
            activate: Box::new(move |t: &mut Self| {
                let _ = t.tx.send_blocking(ev.clone());
            }),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "legion-control".into()
    }

    fn title(&self) -> String {
        "Legion Control".into()
    }

    fn icon_name(&self) -> String {
        "com.legion.Control".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send_blocking(AppEvent::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Legion Control".into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            self.item("Quiet", AppEvent::Profile(PowerProfile::Quiet)),
            self.item("Balanced", AppEvent::Profile(PowerProfile::Balanced)),
            self.item("Performance", AppEvent::Profile(PowerProfile::Performance)),
            MenuItem::Separator,
            self.item("Open", AppEvent::Open),
            self.item("Quit", AppEvent::Quit),
        ]
    }
}

/// Starts the tray icon.
pub fn spawn(tx: async_channel::Sender<AppEvent>) -> Result<(), ksni::Error> {
    let handle = Tray { tx }.spawn()?;
    // keep the service alive for the whole process lifetime
    std::mem::forget(handle);
    Ok(())
}
