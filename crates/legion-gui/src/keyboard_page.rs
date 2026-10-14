//! "Keyboard" page: backlight colour and brightness of the built-in keyboard.
//!
//! Experimental: the colour is stored in the keyboard controller through a
//! vendor protocol, so the page starts with the switch that enables it
//! (`enable_keyboard_rgb`, administrator password) and explains that the
//! original effect is saved and can be restored. Nothing is queried from the
//! controller while the page is not visible or the control is disabled.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};

use legion_core::keyboard::{KeyboardInfo, Rgb};
use legion_i18n::{t, tr};

use crate::worker::Cmd;

/// Delay before a brightness change is sent (one write per user pause).
const BRIGHTNESS_DEBOUNCE: Duration = Duration::from_millis(500);

/// What the worker last reported.
pub type Report = Result<KeyboardInfo, String>;

/// The page.
pub struct KeyboardPage {
    /// Root widget (added to the view stack).
    pub root: gtk::ScrolledWindow,
    cmd: Sender<Cmd>,
    updating: Cell<bool>,
    enable: adw::SwitchRow,
    controller: adw::ActionRow,
    effect: adw::ActionRow,
    colour_group: adw::PreferencesGroup,
    colour_button: gtk::ColorDialogButton,
    brightness: adw::SpinRow,
    brightness_timer: RefCell<Option<glib::SourceId>>,
    restore: gtk::Button,
}

fn to_rgba(c: Rgb) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
        1.0,
    )
}

fn from_rgba(c: &gdk::RGBA) -> Rgb {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb::new(b(c.red()), b(c.green()), b(c.blue()))
}

impl KeyboardPage {
    /// Builds the page.
    pub fn new(cmd: Sender<Cmd>) -> Rc<Self> {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(12)
            .margin_end(12)
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(960)
            .child(&content)
            .build();
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        root.set_widget_name("keyboard");

        // ---- experimental switch + status
        let top = adw::PreferencesGroup::builder()
            .title(tr("Keyboard backlight"))
            .description(tr(
                "Experimental. The colour is stored in the keyboard controller through a vendor \
                 protocol that Lenovo does not document. The effect the keyboard had is saved \
                 first and can be restored.",
            ))
            .build();
        let enable = adw::SwitchRow::builder()
            .title(tr("Enable keyboard control (experimental)"))
            .subtitle(tr("Asks for the administrator password"))
            .build();
        let controller = info_row("Controller");
        let effect = info_row("Effect");
        top.add(&enable);
        top.add(&controller);
        top.add(&effect);
        content.append(&top);

        // ---- colour + brightness
        let colour_group = adw::PreferencesGroup::builder()
            .title(tr("Colour"))
            .description(tr("One steady colour on the whole keyboard."))
            .build();
        let dialog = gtk::ColorDialog::builder()
            .title(tr("Keyboard colour"))
            .with_alpha(false)
            .modal(true)
            .build();
        let colour_button = gtk::ColorDialogButton::new(Some(dialog));
        colour_button.set_valign(gtk::Align::Center);
        colour_button.set_rgba(&to_rgba(Rgb::new(255, 255, 255)));
        let picker = adw::ActionRow::builder()
            .title(tr("Custom colour"))
            .use_markup(false)
            .build();
        picker.add_suffix(&colour_button);
        let swatches = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .valign(gtk::Align::Center)
            .build();
        let presets = adw::ActionRow::builder()
            .title(tr("Presets"))
            .use_markup(false)
            .build();
        presets.add_suffix(&swatches);
        colour_group.add(&picker);
        colour_group.add(&presets);
        let brightness = adw::SpinRow::with_range(0.0, 9.0, 1.0);
        brightness.set_title(&tr("Brightness"));
        colour_group.add(&brightness);
        let restore = gtk::Button::with_label(&tr("Restore the original effect"));
        restore.add_css_class("pill");
        restore.set_halign(gtk::Align::Center);
        restore.set_sensitive(false);
        content.append(&colour_group);
        content.append(&restore);

        let page = Rc::new(Self {
            root,
            cmd,
            updating: Cell::new(false),
            enable,
            controller,
            effect,
            colour_group,
            colour_button,
            brightness,
            brightness_timer: RefCell::new(None),
            restore,
        });

        for (name, colour) in Rgb::NAMED {
            let b = gtk::Button::builder()
                .child(&swatch(colour))
                .css_classes(["flat", "circular"])
                .tooltip_text(tr(name))
                .build();
            let w = Rc::downgrade(&page);
            b.connect_clicked(move |_| {
                if let Some(p) = w.upgrade() {
                    // move the picker without triggering its own handler
                    p.updating.set(true);
                    p.colour_button.set_rgba(&to_rgba(colour));
                    p.updating.set(false);
                    p.send(Cmd::KeyboardColor(colour));
                }
            });
            swatches.append(&b);
        }
        page.connect();
        page.update(&Err(tr("Waiting for the daemon…")));
        page
    }

    fn send(&self, c: Cmd) {
        if !self.updating.get() {
            let _ = self.cmd.send(c);
        }
    }

    fn connect(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.enable.connect_active_notify(move |s| {
            if let Some(p) = w.upgrade() {
                p.send(Cmd::SetExperimental(
                    "enable_keyboard_rgb".into(),
                    s.is_active(),
                ));
            }
        });
        let w = Rc::downgrade(self);
        self.colour_button.connect_rgba_notify(move |b| {
            if let Some(p) = w.upgrade() {
                p.send(Cmd::KeyboardColor(from_rgba(&b.rgba())));
            }
        });
        let w = Rc::downgrade(self);
        self.brightness.connect_value_notify(move |s| {
            let Some(p) = w.upgrade() else { return };
            if p.updating.get() {
                return;
            }
            // send once the user paused
            if let Some(old) = p.brightness_timer.borrow_mut().take() {
                old.remove();
            }
            let level = s.value().round() as u8;
            let w2 = Rc::downgrade(&p);
            let id = glib::timeout_add_local_once(BRIGHTNESS_DEBOUNCE, move || {
                if let Some(p) = w2.upgrade() {
                    *p.brightness_timer.borrow_mut() = None;
                    p.send(Cmd::KeyboardBrightness(level));
                }
            });
            *p.brightness_timer.borrow_mut() = Some(id);
        });
        let w = Rc::downgrade(self);
        self.restore.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.send(Cmd::KeyboardRestore);
            }
        });
        // only talk to the keyboard while the page is on screen
        let tx = self.cmd.clone();
        self.root.connect_map(move |_| {
            let _ = tx.send(Cmd::KeyboardVisible(true));
        });
        let tx = self.cmd.clone();
        self.root.connect_unmap(move |_| {
            let _ = tx.send(Cmd::KeyboardVisible(false));
        });
    }

    /// Shows what the worker reported.
    pub fn update(&self, report: &Report) {
        self.updating.set(true);
        match report {
            Err(why) => {
                self.enable.set_sensitive(false);
                self.enable.set_active(false);
                self.controller.set_subtitle(&tr(why));
                self.effect.set_subtitle("—");
                self.set_controls(false, None);
            }
            Ok(k) => {
                self.enable.set_sensitive(true);
                self.enable.set_active(k.enabled);
                self.controller
                    .set_subtitle(&format!("{} ({}, {})", k.device, k.usb_id, k.node));
                match &k.state {
                    Some(s) => {
                        self.effect.set_subtitle(&match s.color {
                            Some(c) => {
                                format!("{} {c} · {} {}", s.effect, tr("profile"), s.profile)
                            }
                            None => format!("{} · {} {}", s.effect, tr("profile"), s.profile),
                        });
                        if let Some(c) = s.color {
                            self.colour_button.set_rgba(&to_rgba(c));
                        }
                        self.brightness.set_range(0.0, f64::from(k.max_brightness));
                        self.brightness.set_value(f64::from(s.brightness));
                        self.set_controls(true, Some(s.backup));
                    }
                    None => {
                        self.effect
                            .set_subtitle(t("Enable the experimental control above to use it."));
                        self.set_controls(false, None);
                    }
                }
            }
        }
        self.updating.set(false);
    }

    fn set_controls(&self, on: bool, backup: Option<bool>) {
        self.colour_group.set_sensitive(on);
        self.restore.set_sensitive(backup.unwrap_or(false));
    }
}

/// A filled circle in `colour` (drawn, so it does not depend on the theme).
fn swatch(colour: Rgb) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(22)
        .content_height(22)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let (r, g, b) = (
            f64::from(colour.r) / 255.0,
            f64::from(colour.g) / 255.0,
            f64::from(colour.b) / 255.0,
        );
        let radius = f64::from(w.min(h)) / 2.0 - 1.0;
        cr.arc(
            f64::from(w) / 2.0,
            f64::from(h) / 2.0,
            radius,
            0.0,
            std::f64::consts::TAU,
        );
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill_preserve();
        // thin outline so that dark/white swatches stay visible on any theme
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.7);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    area
}

fn info_row(title: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder()
        .title(tr(title))
        .use_markup(false)
        .build();
    r.add_css_class("property");
    r
}
