//! Main window.
//!
//! Every value comes from the daemon ([`Snapshot`]); every action is sent to
//! the D-Bus worker as a [`Cmd`]. Features that do not exist are shown as
//! "Not supported on this hardware." instead of being hidden.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use adw::prelude::*;
use gtk::glib;

use legion_core::PowerProfile;
use legion_core::capabilities::CapabilityState;
use legion_core::model::{CurvePoint, FanCurve, FanMode, GpuVendor, Metric};

use legion_i18n::{t, tr};

use crate::worker::{Cmd, Msg, Snapshot};

pub const NOT_SUPPORTED: &str = "Not supported on this hardware.";
pub const NEEDS_ADMIN: &str = "Administrator permission required.";

const PROFILES: [(PowerProfile, &str); 5] = [
    (PowerProfile::Quiet, "Quiet"),
    (PowerProfile::Balanced, "Balanced"),
    (PowerProfile::Performance, "Performance"),
    (PowerProfile::MaxPower, "Extreme"),
    (PowerProfile::Custom, "Custom"),
];

/// A dashboard card: title + big value + details.
struct Card {
    root: gtk::Box,
    value: gtk::Label,
    details: gtk::Label,
    level: Option<gtk::LevelBar>,
}

fn card(title: &str, icon: &str, with_level: bool) -> Card {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.append(&gtk::Image::from_icon_name(icon));
    let t = gtk::Label::new(Some(&tr(title)));
    t.add_css_class("heading");
    head.append(&t);
    root.append(&head);
    let value = gtk::Label::builder().xalign(0.0).label("—").build();
    value.add_css_class("metric");
    root.append(&value);
    let level = with_level.then(|| {
        let l = gtk::LevelBar::for_interval(0.0, 100.0);
        root.append(&l);
        l
    });
    let details = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(26)
        .width_chars(20)
        .build();
    details.add_css_class("dim-label");
    root.append(&details);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("card");
    frame.append(&root);
    Card {
        root: frame,
        value,
        details,
        level,
    }
}

fn page(title: &str) -> (gtk::ScrolledWindow, gtk::Box) {
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
    let sw = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    sw.set_widget_name(title);
    (sw, content)
}

fn group(title: &str, description: Option<&str>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder().title(tr(title)).build();
    if let Some(d) = description {
        g.set_description(Some(&tr(d)));
    }
    g
}

fn info_row(title: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder()
        .title(tr(title))
        .use_markup(false)
        .build();
    r.add_css_class("property");
    r
}

fn clear(g: &adw::PreferencesGroup, rows: &RefCell<Vec<gtk::Widget>>) {
    for r in rows.borrow_mut().drain(..) {
        g.remove(&r);
    }
}

fn c(v: Option<f64>) -> String {
    v.map(|t| format!("{t:.0} °C"))
        .unwrap_or_else(|| "—".into())
}

/// All widgets that are updated from snapshots.
pub struct Ui {
    pub window: adw::ApplicationWindow,
    stack: adw::ViewStack,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    spinner: adw::Spinner,
    cmd: Sender<Cmd>,
    updating: Cell<bool>,
    // dashboard
    subtitle: adw::WindowTitle,
    profile_group: adw::ToggleGroup,
    cpu: Card,
    gpu: Card,
    mem: Card,
    fans: Card,
    bat: Card,
    temps: Card,
    // battery page
    conservation: adw::SwitchRow,
    charge_type: adw::ComboRow,
    charge_types: RefCell<Vec<String>>,
    threshold: adw::ActionRow,
    bat_rows: BTreeMap<&'static str, adw::ActionRow>,
    // fans page
    fan_group: adw::PreferencesGroup,
    fan_rows: RefCell<Vec<gtk::Widget>>,
    fan_mode: adw::ComboRow,
    fan_fixed: adw::SpinRow,
    fan_curve_rows: Vec<(adw::SpinRow, adw::SpinRow)>,
    fan_apply: gtk::Button,
    fan_note: adw::ActionRow,
    fan_ranges_set: Cell<bool>,
    // performance page
    governor: adw::ComboRow,
    governors: RefCell<Vec<String>>,
    epp: adw::ComboRow,
    epps: RefCell<Vec<String>>,
    turbo: adw::SwitchRow,
    custom: Rc<crate::custom_page::CustomPage>,
    // profiles page
    profiles_group: adw::PreferencesGroup,
    profile_rows: RefCell<Vec<gtk::Widget>>,
    profile_names: RefCell<Option<Vec<String>>>,
    // capabilities page
    caps_group: adw::PreferencesGroup,
    caps_rows: RefCell<Vec<gtk::Widget>>,
    caps_count: Cell<usize>,
}

impl Ui {
    fn send(&self, c: Cmd) {
        if !self.updating.get() {
            let _ = self.cmd.send(c);
        }
    }

    /// Name of the visible page.
    pub fn page(&self) -> Option<String> {
        self.stack.visible_child_name().map(|n| n.to_string())
    }

    /// Shows a toast.
    pub fn toast(&self, text: &str) {
        let t = adw::Toast::new(&glib::markup_escape_text(&tr(text)));
        t.set_timeout(4);
        self.toasts.add_toast(t);
    }

    /// Handles a worker message.
    pub fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Update(s) => {
                self.banner.set_revealed(false);
                self.update(&s);
            }
            Msg::DaemonDown(reason) => {
                self.banner.set_title(&tr(&format!(
                    "Daemon unavailable: {reason}. Start it with: sudo systemctl enable --now legion-control.service"
                )));
                self.banner.set_revealed(true);
            }
            Msg::Done(m) => {
                self.custom.on_done();
                self.toast(&m);
            }
            Msg::Failed(m) => self.toast(&m),
            Msg::Busy(b) => self.spinner.set_visible(b),
        }
    }

    fn update(self: &Rc<Self>, s: &Snapshot) {
        tracing::debug!(caps = s.caps.len(), profiles = s.profiles.len(), "snapshot");
        self.updating.set(true);
        let st = &s.status;
        self.subtitle.set_subtitle(&st.system.display_product());

        // profile toggles
        let pp = st.profile.platform_profile.as_ref();
        for (p, _) in PROFILES {
            if let Some(toggle) = self.profile_group.toggle_by_name(p.as_str()) {
                let available = pp.is_some_and(|pp| pp.mapping.iter().any(|m| m.logical == p))
                    || (st.profile.ppd.is_some()
                        && matches!(
                            p,
                            PowerProfile::Quiet
                                | PowerProfile::Balanced
                                | PowerProfile::Performance
                        ));
                toggle.set_enabled(available);
                toggle.set_tooltip(if available { "" } else { t(NOT_SUPPORTED) });
            }
        }
        self.profile_group
            .set_active_name(Some(st.profile.current.as_str()));

        // CPU
        let cpu = &st.cpu;
        self.cpu.value.set_label(&c(cpu.temperature_c));
        let usage = match &cpu.usage_percent {
            Metric::Ok(u) => format!("{u:.0}%"),
            _ => "—".into(),
        };
        let power = match &cpu.package_power_w {
            Metric::Ok(w) => format!("{w:.1} W"),
            Metric::PermissionRequired => NEEDS_ADMIN.into(),
            _ => "—".into(),
        };
        self.cpu.details.set_label(&tr(&format!(
            "Usage {usage} · {} MHz · {power}\n{}",
            cpu.frequency_mhz
                .map(|f| f.to_string())
                .unwrap_or("—".into()),
            cpu.model.as_deref().unwrap_or("")
        )));

        // GPU
        match st
            .gpu
            .devices
            .iter()
            .find(|d| d.vendor == GpuVendor::Nvidia)
        {
            Some(d) => match &d.metrics {
                Some(m) => {
                    self.gpu.value.set_label(&c(m.temperature_c.map(f64::from)));
                    self.gpu.details.set_label(&tr(&format!(
                        "Usage {}% · {} MHz · {}\nVRAM {} / {} MiB · {}",
                        m.utilization_percent.unwrap_or(0),
                        m.graphics_clock_mhz.unwrap_or(0),
                        m.power_draw_w
                            .map(|w| format!("{w:.1} W"))
                            .unwrap_or("—".into()),
                        m.memory_used_mib.unwrap_or(0),
                        m.memory_total_mib.unwrap_or(0),
                        d.name.as_deref().unwrap_or("")
                    )));
                }
                None => {
                    self.gpu.value.set_label(&tr("Idle"));
                    self.gpu.details.set_label(&tr(&format!(
                        "{} — suspended to save power\nMode: {:?}",
                        d.name.as_deref().unwrap_or("dGPU"),
                        st.gpu.mode
                    )));
                }
            },
            None => {
                self.gpu.value.set_label(&tr("—"));
                self.gpu.details.set_label(t(NOT_SUPPORTED));
            }
        }

        // memory
        let m = &st.memory;
        self.mem
            .value
            .set_label(&tr(&format!("{:.1} GiB", m.used_mib as f64 / 1024.0)));
        if let Some(l) = &self.mem.level {
            if m.total_mib > 0 {
                l.set_value(m.used_mib as f64 * 100.0 / m.total_mib as f64);
            }
        }
        self.mem.details.set_label(&tr(&format!(
            "of {:.1} GiB · swap {:.1} GiB used",
            m.total_mib as f64 / 1024.0,
            (m.swap_total_mib - m.swap_free_mib.min(m.swap_total_mib)) as f64 / 1024.0
        )));

        // fans card
        let fans = &st.fans.fans;
        if fans.is_empty() {
            self.fans.value.set_label(&tr("—"));
            self.fans.details.set_label(t(NOT_SUPPORTED));
        } else {
            self.fans.value.set_label(
                &fans
                    .iter()
                    .map(|f| f.rpm.map(|r| r.to_string()).unwrap_or("—".into()))
                    .collect::<Vec<_>>()
                    .join(" / "),
            );
            self.fans.details.set_label(&tr(&format!(
                "RPM · mode: {}",
                legion_profiles_fan_name(&s.info.fan_mode)
            )));
        }

        // temperatures card
        self.temps.value.set_label(&c(st.thermal.storage_c));
        self.temps.details.set_label(&tr(&format!(
            "SSD · {} sensors · hottest {}",
            st.thermal.sensors.len(),
            c(st.thermal
                .sensors
                .iter()
                .map(|s| s.celsius)
                .reduce(f64::max))
        )));

        // battery card + page
        match st.battery.batteries.first() {
            Some(b) => {
                let pct = b.capacity_percent.unwrap_or(0);
                self.bat.value.set_label(&tr(&format!("{pct}%")));
                if let Some(l) = &self.bat.level {
                    l.set_value(f64::from(pct));
                }
                self.bat.details.set_label(&tr(&format!(
                    "{} · {}",
                    b.status.as_deref().unwrap_or("?"),
                    b.power_w.map(|w| format!("{w:.1} W")).unwrap_or_default()
                )));
                let set = |k: &str, v: String| {
                    if let Some(r) = self.bat_rows.get(k) {
                        r.set_subtitle(&tr(&v));
                    }
                };
                set("status", b.status.clone().unwrap_or_default());
                set("capacity", format!("{pct}%"));
                set(
                    "health",
                    format!(
                        "{:?}{}",
                        b.health,
                        b.health_percent
                            .map(|h| format!(" ({h:.1}% of design)"))
                            .unwrap_or_default()
                    ),
                );
                set(
                    "cycles",
                    b.cycle_count.map(|v| v.to_string()).unwrap_or("—".into()),
                );
                set(
                    "energy",
                    format!(
                        "{:.1} / {:.1} Wh",
                        b.energy_now_wh.unwrap_or(0.0),
                        b.energy_full_wh.unwrap_or(0.0)
                    ),
                );
                set(
                    "model",
                    format!(
                        "{} {}",
                        b.manufacturer.clone().unwrap_or_default(),
                        b.model.clone().unwrap_or_default()
                    ),
                );
                // charge type
                let types = b
                    .charge_types
                    .as_ref()
                    .map(|c| c.available.clone())
                    .unwrap_or_default();
                if *self.charge_types.borrow() != types {
                    let list =
                        gtk::StringList::new(&types.iter().map(String::as_str).collect::<Vec<_>>());
                    self.charge_type.set_model(Some(&list));
                    *self.charge_types.borrow_mut() = types.clone();
                }
                self.charge_type.set_sensitive(!types.is_empty());
                self.charge_type.set_subtitle(if types.is_empty() {
                    t(NOT_SUPPORTED)
                } else {
                    ""
                });
                if let Some(active) = b.charge_types.as_ref().and_then(|c| c.active.as_ref()) {
                    if let Some(i) = types.iter().position(|t| t == active) {
                        self.charge_type.set_selected(i as u32);
                    }
                }
                self.threshold
                    .set_subtitle(&tr(&match b.charge_end_threshold {
                        Some(t) => format!("{t}% (use legionctl battery threshold)"),
                        None => NOT_SUPPORTED.into(),
                    }));
            }
            None => {
                self.bat.value.set_label(&tr("—"));
                self.bat.details.set_label(&tr("No system battery found."));
            }
        }
        match st.battery.conservation_mode {
            Some(on) => {
                self.conservation.set_sensitive(true);
                self.conservation.set_active(on);
                self.conservation
                    .set_subtitle(&tr("Firmware charge limit to extend battery lifespan"));
            }
            None => {
                self.conservation.set_sensitive(false);
                self.conservation.set_subtitle(t(NOT_SUPPORTED));
            }
        }

        self.update_fans(s);
        self.update_performance(s);
        self.custom.update(s);
        self.update_profiles(s);
        self.update_caps(s);
        self.updating.set(false);
    }

    fn update_fans(self: &Rc<Self>, s: &Snapshot) {
        let fans = &s.status.fans.fans;
        clear(&self.fan_group, &self.fan_rows);
        if fans.is_empty() {
            let r = info_row("Fans");
            r.set_subtitle(t(NOT_SUPPORTED));
            self.fan_group.add(&r);
            self.fan_rows.borrow_mut().push(r.upcast());
        }
        for f in fans {
            let r = info_row(&f.label);
            r.set_subtitle(&tr(&format!(
                "{} RPM{}",
                f.rpm.map(|v| v.to_string()).unwrap_or("—".into()),
                match (f.min_rpm, f.max_rpm) {
                    (Some(a), Some(b)) => format!(" · range {a}–{b}"),
                    _ => String::new(),
                }
            )));
            self.fan_group.add(&r);
            self.fan_rows.borrow_mut().push(r.upcast());
        }
        let controllable = s.status.fans.fan_control_supported;
        let enabled = controllable && s.info.fan_control_enabled;
        self.fan_mode.set_sensitive(enabled);
        self.fan_fixed.set_sensitive(enabled);
        self.fan_apply.set_sensitive(enabled);
        for (t, r) in &self.fan_curve_rows {
            t.set_sensitive(enabled);
            r.set_sensitive(enabled);
        }
        self.fan_note.set_subtitle(&tr(&if !controllable {
            s.status.fans.note.clone()
        } else if !s.info.fan_control_enabled {
            "Experimental: enable with \"enable_fan_control\": true in /etc/legion-control/daemon.json".into()
        } else {
            let mut n = "Control returns to the firmware automatically if the CPU or GPU gets too hot.".to_owned();
            if let Some(ev) = &s.info.last_safety_event {
                n = format!("Reverted to automatic: {ev}");
            }
            n
        }));
        if let (false, Some(f)) = (self.fan_ranges_set.get(), fans.first()) {
            if let (Some(min), Some(max)) = (f.min_rpm, f.max_rpm) {
                self.fan_fixed.set_range(f64::from(min), f64::from(max));
                for (_, r) in &self.fan_curve_rows {
                    r.set_range(f64::from(min), f64::from(max));
                }
                self.fan_ranges_set.set(true);
            }
        }
        let idx = match &s.info.fan_mode {
            FanMode::Auto => 0,
            FanMode::Fixed { rpm } => {
                self.fan_fixed.set_value(f64::from(*rpm));
                1
            }
            FanMode::Curve(c) => {
                for (i, (t, r)) in self.fan_curve_rows.iter().enumerate() {
                    if let Some(p) = c.points.get(i) {
                        t.set_value(f64::from(p.temp_c));
                        r.set_value(f64::from(p.rpm));
                    }
                }
                2
            }
        };
        self.fan_mode.set_selected(idx);
    }

    fn update_performance(self: &Rc<Self>, s: &Snapshot) {
        let cpu = &s.status.cpu;
        let sync_combo = |combo: &adw::ComboRow,
                          store: &RefCell<Vec<String>>,
                          avail: &[String],
                          cur: &[String]| {
            if *store.borrow() != avail {
                combo.set_model(Some(&gtk::StringList::new(
                    &avail.iter().map(String::as_str).collect::<Vec<_>>(),
                )));
                *store.borrow_mut() = avail.to_vec();
            }
            combo.set_sensitive(!avail.is_empty());
            combo.set_subtitle(t(if avail.is_empty() {
                NOT_SUPPORTED
            } else if cur.len() > 1 {
                "mixed"
            } else {
                ""
            }));
            if let Some(i) = cur.first().and_then(|c| avail.iter().position(|a| a == c)) {
                combo.set_selected(i as u32);
            }
        };
        sync_combo(
            &self.governor,
            &self.governors,
            &cpu.available_governors,
            &cpu.governors,
        );
        sync_combo(&self.epp, &self.epps, &cpu.available_epp, &cpu.epp);
        match cpu.turbo {
            legion_core::model::TurboState::Unknown => {
                self.turbo.set_sensitive(false);
                self.turbo.set_subtitle(t(NOT_SUPPORTED));
            }
            t => {
                self.turbo.set_sensitive(true);
                self.turbo
                    .set_active(t == legion_core::model::TurboState::Enabled);
            }
        }
    }

    fn update_profiles(self: &Rc<Self>, s: &Snapshot) {
        // `None` until the first render, so an empty list is shown too
        if self.profile_names.borrow().as_ref() == Some(&s.profiles) {
            return;
        }
        clear(&self.profiles_group, &self.profile_rows);
        if s.profiles.is_empty() {
            let r = info_row("No saved profiles");
            r.set_subtitle(&tr("Save the current settings below"));
            self.profiles_group.add(&r);
            self.profile_rows.borrow_mut().push(r.upcast());
        }
        for name in &s.profiles {
            let r = adw::ActionRow::builder()
                .title(name)
                .use_markup(false)
                .build();
            let apply = gtk::Button::with_label(&tr("Apply"));
            apply.set_valign(gtk::Align::Center);
            apply.add_css_class("suggested-action");
            let del = gtk::Button::from_icon_name("user-trash-symbolic");
            del.set_valign(gtk::Align::Center);
            del.add_css_class("flat");
            del.set_tooltip_text(Some(&tr("Delete")));
            for (b, is_apply) in [(&apply, true), (&del, false)] {
                let weak = Rc::downgrade(self);
                let n = name.clone();
                b.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        ui.send(if is_apply {
                            Cmd::ApplyProfile(n.clone())
                        } else {
                            Cmd::DeleteProfile(n.clone())
                        });
                    }
                });
            }
            r.add_suffix(&apply);
            r.add_suffix(&del);
            self.profiles_group.add(&r);
            self.profile_rows.borrow_mut().push(r.upcast());
        }
        *self.profile_names.borrow_mut() = Some(s.profiles.clone());
    }

    fn update_caps(self: &Rc<Self>, s: &Snapshot) {
        if self.caps_count.get() == s.caps.len() && !s.caps.is_empty() {
            return;
        }
        clear(&self.caps_group, &self.caps_rows);
        for cap in &s.caps {
            let r = adw::ActionRow::builder()
                .title(tr(&cap.id.replace('_', " ")))
                .subtitle(tr(&cap.note))
                .use_markup(false)
                .build();
            let label = gtk::Label::new(Some(t(match cap.state {
                CapabilityState::Supported => "Supported",
                CapabilityState::ReadOnly => "Monitoring",
                CapabilityState::Detected => "Available",
                CapabilityState::Unsupported => "Not supported",
            })));
            label.add_css_class(match cap.state {
                CapabilityState::Supported => "success",
                CapabilityState::ReadOnly => "accent",
                CapabilityState::Detected => "warning",
                CapabilityState::Unsupported => "dim-label",
            });
            r.add_suffix(&label);
            self.caps_group.add(&r);
            self.caps_rows.borrow_mut().push(r.upcast());
        }
        self.caps_count.set(s.caps.len());
    }
}

fn legion_profiles_fan_name(m: &FanMode) -> String {
    match m {
        FanMode::Auto => "auto".into(),
        FanMode::Fixed { rpm } => format!("fixed {rpm}"),
        FanMode::Curve(_) => "curve".into(),
    }
}

/// Builds the window. `page` selects the initial page (development aid).
pub fn build(app: &adw::Application, cmd: Sender<Cmd>, page_name: Option<&str>) -> Rc<Ui> {
    // ---------- dashboard ----------
    let (dash, dash_box) = page("dashboard");
    let profile_group = adw::ToggleGroup::new();
    profile_group.add_css_class("round");
    profile_group.set_homogeneous(true);
    for (p, label) in PROFILES {
        profile_group.add(
            adw::Toggle::builder()
                .name(p.as_str())
                .label(t(label))
                .build(),
        );
    }
    let pg = group("Power profile", None);
    pg.add(&profile_group);
    dash_box.append(&pg);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(3)
        .column_spacing(12)
        .row_spacing(12)
        .build();
    let cpu = card("CPU", "computer-symbolic", false);
    let gpu = card("GPU", "video-display-symbolic", false);
    let mem = card("Memory", "drive-harddisk-solidstate-symbolic", true);
    let fans = card("Fans", "weather-windy-symbolic", false);
    let temps = card("Storage", "temperature-symbolic", false);
    let bat = card("Battery", "battery-symbolic", true);
    for c in [&cpu, &gpu, &mem, &fans, &temps, &bat] {
        flow.insert(&c.root, -1);
    }
    dash_box.append(&flow);

    // ---------- battery ----------
    let (bat_page, bat_box) = page("battery");
    let charging = group("Charging", None);
    let conservation = adw::SwitchRow::builder()
        .title(tr("Conservation mode"))
        .build();
    let charge_type = adw::ComboRow::builder().title(tr("Charge type")).build();
    let threshold = info_row("Charge threshold");
    charging.add(&conservation);
    charging.add(&charge_type);
    charging.add(&threshold);
    bat_box.append(&charging);
    let bstatus = group("Status", None);
    let mut bat_rows = BTreeMap::new();
    for (k, t) in [
        ("status", "Status"),
        ("capacity", "Charge"),
        ("health", "Health"),
        ("cycles", "Cycle count"),
        ("energy", "Energy"),
        ("model", "Model"),
    ] {
        let r = info_row(t);
        bstatus.add(&r);
        bat_rows.insert(k, r);
    }
    bat_box.append(&bstatus);

    // ---------- fans ----------
    let (fan_page, fan_box) = page("fans");
    let fan_group = group("Fans", None);
    fan_box.append(&fan_group);
    let control = group("Control", None);
    let fan_note = info_row("Manual control");
    let fan_mode = adw::ComboRow::builder()
        .title(tr("Mode"))
        .model(&gtk::StringList::new(&[
            t("Automatic (firmware)"),
            t("Fixed speed"),
            t("Curve"),
        ]))
        .build();
    let fan_fixed = adw::SpinRow::with_range(1800.0, 4600.0, 100.0);
    fan_fixed.set_title(&tr("Fixed speed (RPM)"));
    control.add(&fan_note);
    control.add(&fan_mode);
    control.add(&fan_fixed);
    let curve_group = group(
        "Curve",
        Some(
            "Hottest of CPU/GPU → fan speed. Below the first point the firmware controls the fans.",
        ),
    );
    let example = legion_fan::example_curve();
    let fan_curve_rows: Vec<(adw::SpinRow, adw::SpinRow)> = example
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let t = adw::SpinRow::with_range(20.0, 100.0, 1.0);
            t.set_title(&tr(&format!("Point {} — temperature (°C)", i + 1)));
            t.set_value(f64::from(p.temp_c));
            let r = adw::SpinRow::with_range(1800.0, 4600.0, 100.0);
            r.set_use_markup(false);
            r.set_title(&tr(&format!("Point {} — speed (RPM)", i + 1)));
            r.set_value(f64::from(p.rpm));
            curve_group.add(&t);
            curve_group.add(&r);
            (t, r)
        })
        .collect();
    let fan_apply = gtk::Button::with_label(&tr("Apply fan settings"));
    fan_apply.add_css_class("suggested-action");
    fan_apply.add_css_class("pill");
    fan_apply.set_halign(gtk::Align::Center);
    fan_box.append(&control);
    fan_box.append(&curve_group);
    fan_box.append(&fan_apply);

    // ---------- performance ----------
    let (perf_page, perf_box) = page("performance");
    let cpu_group = group(
        "CPU",
        Some("power-profiles-daemon may override the EPP when the profile changes."),
    );
    let governor = adw::ComboRow::builder().title(tr("Governor")).build();
    let epp = adw::ComboRow::builder()
        .title(tr("Energy performance preference"))
        .build();
    let turbo = adw::SwitchRow::builder().title(tr("Turbo boost")).build();
    cpu_group.add(&governor);
    cpu_group.add(&epp);
    cpu_group.add(&turbo);
    perf_box.append(&cpu_group);

    // ---------- profiles ----------
    let (prof_page, prof_box) = page("profiles");
    let profiles_group = group(
        "Saved profiles",
        Some(
            "Stored in ~/.config/legion-control/profiles. Applying validates every value against the hardware.",
        ),
    );
    let save_group = group("", None);
    let save_row = adw::EntryRow::builder()
        .title(tr("Save current settings as…"))
        .show_apply_button(true)
        .build();
    save_group.add(&save_row);
    prof_box.append(&profiles_group);
    prof_box.append(&save_group);

    // ---------- hardware ----------
    let (caps_page, caps_box) = page("hardware");
    let caps_group = group(
        "Capabilities",
        Some("Detected from the hardware at runtime."),
    );
    caps_box.append(&caps_group);

    // ---------- chrome ----------
    let stack = adw::ViewStack::new();
    let custom = crate::custom_page::CustomPage::new(cmd.clone(), {
        let stack = stack.clone();
        move || stack.set_visible_child_name("fans")
    });
    for (w, name, title, icon) in [
        (&dash, "dashboard", "Dashboard", "speedometer-symbolic"),
        (&bat_page, "battery", "Battery", "battery-symbolic"),
        (&fan_page, "fans", "Fans", "weather-windy-symbolic"),
        (
            &perf_page,
            "performance",
            "Performance",
            "power-profile-performance-symbolic",
        ),
        (
            &custom.root,
            "custom",
            "Custom",
            "preferences-other-symbolic",
        ),
        (&prof_page, "profiles", "Profiles", "document-save-symbolic"),
        (&caps_page, "hardware", "Hardware", "computer-symbolic"),
    ] {
        stack.add_titled_with_icon(w, Some(name), t(title), icon);
    }
    if let Some(p) = page_name {
        stack.set_visible_child_name(p);
    }
    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Narrow)
        .build();
    let subtitle = adw::WindowTitle::new("Legion Control", "");
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    let spinner = adw::Spinner::new();
    spinner.set_visible(false);
    header.pack_start(&spinner);
    let menu = gtk::gio::Menu::new();
    let languages = gtk::gio::Menu::new();
    languages.append(Some(t("Automatic")), Some("app.language::auto"));
    for l in legion_i18n::Lang::ALL {
        languages.append(
            Some(l.native_name()),
            Some(&format!("app.language::{}", l.code())),
        );
    }
    menu.append_submenu(Some(t("Language")), &languages);
    menu.append(Some(&tr("Start at Login")), Some("app.autostart"));
    menu.append(Some(&tr("About Legion Control")), Some("app.about"));
    menu.append(Some(&tr("Quit")), Some("app.quit"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .build(),
    );
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
    let banner = adw::Banner::new("");
    banner.set_use_markup(false);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&stack);
    stack.set_vexpand(true);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&content));
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&toasts));
    tv.add_bottom_bar(&switcher_bar);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(tr("Legion Control"))
        .default_width(980)
        .default_height(720)
        .width_request(360)
        .height_request(400)
        .content(&tv)
        .build();
    // responsive: switcher moves to the bottom on narrow windows
    let bp = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 700sp").expect("valid condition"),
    );
    bp.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
    bp.add_setter(&header, "title-widget", Some(&subtitle.to_value()));
    window.add_breakpoint(bp);

    let ui = Rc::new(Ui {
        window,
        stack,
        toasts,
        banner,
        spinner,
        cmd,
        updating: Cell::new(false),
        subtitle,
        profile_group,
        cpu,
        gpu,
        mem,
        fans,
        bat,
        temps,
        conservation,
        charge_type,
        charge_types: RefCell::new(vec![]),
        threshold,
        bat_rows,
        fan_group,
        fan_rows: RefCell::new(vec![]),
        fan_mode,
        fan_fixed,
        fan_curve_rows,
        fan_apply,
        fan_note,
        fan_ranges_set: Cell::new(false),
        governor,
        governors: RefCell::new(vec![]),
        epp,
        epps: RefCell::new(vec![]),
        turbo,
        custom,
        profiles_group,
        profile_rows: RefCell::new(vec![]),
        profile_names: RefCell::new(None),
        caps_group,
        caps_rows: RefCell::new(vec![]),
        caps_count: Cell::new(0),
    });
    connect(&ui, &save_row);
    ui
}

fn connect(ui: &Rc<Ui>, save_row: &adw::EntryRow) {
    let w = Rc::downgrade(ui);
    ui.profile_group.connect_active_name_notify(move |g| {
        if let (Some(ui), Some(name)) = (w.upgrade(), g.active_name()) {
            if let Ok(p) = name.parse::<PowerProfile>() {
                ui.send(Cmd::Profile(p));
            }
        }
    });
    let w = Rc::downgrade(ui);
    ui.conservation.connect_active_notify(move |s| {
        if let Some(ui) = w.upgrade() {
            ui.send(Cmd::Conservation(s.is_active()));
        }
    });
    let w = Rc::downgrade(ui);
    ui.charge_type.connect_selected_notify(move |c| {
        if let Some(ui) = w.upgrade() {
            if let Some(t) = ui.charge_types.borrow().get(c.selected() as usize) {
                ui.send(Cmd::ChargeType(t.clone()));
            }
        }
    });
    let w = Rc::downgrade(ui);
    ui.governor.connect_selected_notify(move |c| {
        if let Some(ui) = w.upgrade() {
            if let Some(t) = ui.governors.borrow().get(c.selected() as usize) {
                ui.send(Cmd::Governor(t.clone()));
            }
        }
    });
    let w = Rc::downgrade(ui);
    ui.epp.connect_selected_notify(move |c| {
        if let Some(ui) = w.upgrade() {
            if let Some(t) = ui.epps.borrow().get(c.selected() as usize) {
                ui.send(Cmd::Epp(t.clone()));
            }
        }
    });
    let w = Rc::downgrade(ui);
    ui.turbo.connect_active_notify(move |s| {
        if let Some(ui) = w.upgrade() {
            ui.send(Cmd::Turbo(s.is_active()));
        }
    });
    let w = Rc::downgrade(ui);
    ui.fan_apply.connect_clicked(move |_| {
        let Some(ui) = w.upgrade() else { return };
        let mode = match ui.fan_mode.selected() {
            0 => FanMode::Auto,
            1 => FanMode::Fixed {
                rpm: ui.fan_fixed.value() as u32,
            },
            _ => FanMode::Curve(FanCurve {
                points: ui
                    .fan_curve_rows
                    .iter()
                    .map(|(t, r)| CurvePoint {
                        temp_c: t.value() as u8,
                        rpm: r.value() as u32,
                    })
                    .collect(),
                hysteresis_c: 4,
            }),
        };
        if let FanMode::Curve(c) = &mode {
            if let Err(e) = legion_fan::validate_curve(c) {
                ui.toast(&e.to_string());
                return;
            }
        }
        let _ = ui.cmd.send(Cmd::Fan(mode));
    });
    let w = Rc::downgrade(ui);
    save_row.connect_apply(move |row| {
        let Some(ui) = w.upgrade() else { return };
        let name = row.text().trim().to_owned();
        match legion_profiles::custom::validate_name(&name) {
            Ok(()) => {
                let _ = ui.cmd.send(Cmd::SaveProfile(name));
                row.set_text("");
            }
            Err(e) => ui.toast(&e.to_string()),
        }
    });
}
