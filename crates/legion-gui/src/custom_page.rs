//! "Custom mode" page, modelled on the Legion Space / Vantage custom mode:
//! one slider + number field per firmware tunable (CPU PL1/PL2/cross-load/
//! temperature, GPU cTGP/Dynamic Boost/offset/temperature…), applied
//! together with a single authorisation.
//!
//! Ranges, steps and defaults come from the driver at runtime
//! (`lenovo-wmi-other` capability data), never from hard-coded values.
//! Attributes for which the firmware reports no range are shown read-only.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::mpsc::Sender;

use adw::prelude::*;

use legion_core::PowerProfile;
use legion_core::model::{FirmwareAttribute, PowerLimits};
use legion_i18n::{t, tr};

use crate::worker::{Cmd, Snapshot};

/// Friendly label, unit and help for known attributes
/// (`Documentation/wmi/devices/lenovo-wmi-other.rst`).
fn meta(name: &str) -> (&'static str, &'static str, &'static str) {
    match name {
        "ppt_pl1_spl" => (
            "CPU long-term power limit (PL1)",
            "W",
            "Sustained CPU power.",
        ),
        "ppt_pl2_sppt" => (
            "CPU short-term power limit (PL2)",
            "W",
            "Power allowed during short bursts.",
        ),
        "ppt_pl3_fppt" => ("CPU fast power limit (PL3)", "W", "Very short power peaks."),
        "ppt_pl4_ipl" => (
            "CPU instantaneous power limit (PL4)",
            "W",
            "Absolute instantaneous peak.",
        ),
        "ppt_pl1_tau" => (
            "PL1 time window (Tau)",
            "s",
            "How long PL2 can be sustained before falling back to PL1.",
        ),
        "ppt_cpu_cl" => (
            "CPU cross-loading power limit",
            "W",
            "CPU power while the GPU is also under load.",
        ),
        "ppt_pl1_spl_cl" => (
            "PL1 while cross-loading",
            "W",
            "Long-term CPU limit when CPU and GPU are both loaded.",
        ),
        "ppt_pl2_sppt_cl" => (
            "PL2 while cross-loading",
            "W",
            "Short-term CPU limit when CPU and GPU are both loaded.",
        ),
        "ppt_pl1_apu_spl" => ("APU long-term power limit", "W", ""),
        "cpu_temp" => (
            "CPU temperature limit",
            "°C",
            "Temperature at which the CPU is throttled.",
        ),
        "gpu_nv_ctgp" => ("GPU configurable TGP (cTGP)", "W", "Base graphics power."),
        "gpu_nv_ppab" => (
            "GPU Dynamic Boost (PPAB)",
            "W",
            "Extra GPU power when the CPU has headroom.",
        ),
        "gpu_nv_cpu_boost" => (
            "Dynamic Boost GPU → CPU",
            "W",
            "Power the GPU can lend to the CPU.",
        ),
        "gpu_nv_ac_offset" => (
            "GPU AC power offset (TPP)",
            "W",
            "Total processing power baseline offset on AC.",
        ),
        "gpu_nv_bpl" => ("GPU base power limit", "W", ""),
        "gpu_temp" => (
            "GPU temperature limit",
            "°C",
            "Temperature at which the GPU is throttled.",
        ),
        "dgpu_boost_clk" => ("GPU boost clock", "MHz", ""),
        _ => ("", "", ""),
    }
}

struct LimitRow {
    row: adw::ActionRow,
    adj: Option<gtk::Adjustment>,
    attr: FirmwareAttribute,
}

/// The page.
pub struct CustomPage {
    /// Root widget (added to the view stack).
    pub root: gtk::ScrolledWindow,
    cmd: Sender<Cmd>,
    updating: Cell<bool>,
    allow: adw::SwitchRow,
    profile_row: adw::ActionRow,
    profile_btn: gtk::Button,
    cpu_group: adw::PreferencesGroup,
    gpu_group: adw::PreferencesGroup,
    rows: RefCell<BTreeMap<String, LimitRow>>,
    layout_key: RefCell<String>,
    dirty: RefCell<BTreeSet<String>>,
    editable: Cell<bool>,
    warning: gtk::Label,
    apply: gtk::Button,
    discard: gtk::Button,
    defaults: gtk::Button,
}

fn is_gpu(name: &str) -> bool {
    name.starts_with("gpu_") || name.starts_with("dgpu_")
}

impl CustomPage {
    /// Builds the page. `open_fans` switches to the fan page.
    pub fn new(cmd: Sender<Cmd>, open_fans: impl Fn() + 'static) -> Rc<Self> {
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

        let state = adw::PreferencesGroup::builder()
            .title(t("Custom mode"))
            .description(t("Set CPU and GPU power and temperature limits, like the custom mode of Legion Space. Values come from the firmware and are validated before being applied."))
            .build();
        let allow = adw::SwitchRow::builder()
            .title(t("Allow power adjustments (experimental)"))
            .subtitle(t(
                "Requires the administrator password. Saved in /etc/legion-control/daemon.json.",
            ))
            .build();
        let profile_row = adw::ActionRow::builder().title(t("Custom profile")).build();
        let profile_btn = gtk::Button::with_label(t("Activate Custom mode"));
        profile_btn.set_valign(gtk::Align::Center);
        profile_btn.add_css_class("suggested-action");
        profile_row.add_suffix(&profile_btn);
        let fans_row = adw::ActionRow::builder()
            .title(t("Fan curve"))
            .subtitle(t("Fixed speed or temperature curve for the fans."))
            .activatable(true)
            .build();
        fans_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        fans_row.connect_activated(move |_| open_fans());
        state.add(&allow);
        state.add(&profile_row);
        state.add(&fans_row);
        content.append(&state);

        let cpu_group = adw::PreferencesGroup::builder().title(t("CPU")).build();
        let gpu_group = adw::PreferencesGroup::builder().title(t("GPU")).build();
        content.append(&cpu_group);
        content.append(&gpu_group);

        let warning = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .visible(false)
            .build();
        warning.add_css_class("error");
        content.append(&warning);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::End)
            .build();
        let defaults = gtk::Button::with_label(t("Restore defaults"));
        let discard = gtk::Button::with_label(t("Discard"));
        let apply = gtk::Button::with_label(t("Apply"));
        apply.add_css_class("suggested-action");
        for b in [&defaults, &discard, &apply] {
            b.add_css_class("pill");
            buttons.append(b);
        }
        content.append(&buttons);

        let page = Rc::new(Self {
            root,
            cmd,
            updating: Cell::new(false),
            allow,
            profile_row,
            profile_btn,
            cpu_group,
            gpu_group,
            rows: RefCell::new(BTreeMap::new()),
            layout_key: RefCell::new(String::new()),
            dirty: RefCell::new(BTreeSet::new()),
            editable: Cell::new(false),
            warning,
            apply,
            discard,
            defaults,
        });
        page.connect();
        page.refresh_buttons();
        page
    }

    fn connect(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.allow.connect_active_notify(move |s| {
            if let Some(p) = w.upgrade() {
                if !p.updating.get() {
                    let _ = p.cmd.send(Cmd::SetExperimental(
                        "enable_power_limits".into(),
                        s.is_active(),
                    ));
                }
            }
        });
        let w = Rc::downgrade(self);
        self.profile_btn.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                let _ = p.cmd.send(Cmd::Profile(PowerProfile::Custom));
            }
        });
        let w = Rc::downgrade(self);
        self.apply.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                let values = p.pending();
                if !values.is_empty() {
                    let _ = p.cmd.send(Cmd::ApplyLimits(values));
                }
            }
        });
        let w = Rc::downgrade(self);
        self.discard.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.dirty.borrow_mut().clear();
                p.load_current();
            }
        });
        let w = Rc::downgrade(self);
        self.defaults.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                let rows = p.rows.borrow();
                for (name, r) in rows.iter() {
                    let (Some(adj), Some(def)) = (
                        &r.adj,
                        r.attr
                            .default_value
                            .as_deref()
                            .and_then(|v| v.parse::<f64>().ok()),
                    ) else {
                        continue;
                    };
                    if (adj.value() - def).abs() > f64::EPSILON {
                        adj.set_value(def);
                        p.dirty.borrow_mut().insert(name.clone());
                    }
                }
                drop(rows);
                p.refresh_buttons();
            }
        });
    }

    /// Values edited by the user.
    fn pending(&self) -> BTreeMap<String, i64> {
        let rows = self.rows.borrow();
        self.dirty
            .borrow()
            .iter()
            .filter_map(|n| {
                let adj = rows.get(n)?.adj.as_ref()?;
                Some((n.clone(), adj.value().round() as i64))
            })
            .collect()
    }

    fn value_of(&self, name: &str) -> Option<i64> {
        let rows = self.rows.borrow();
        let r = rows.get(name)?;
        match &r.adj {
            Some(a) => Some(a.value().round() as i64),
            None => r.attr.current(),
        }
    }

    fn refresh_buttons(&self) {
        let dirty = !self.dirty.borrow().is_empty();
        let bad = matches!(
            (self.value_of("ppt_pl1_spl"), self.value_of("ppt_pl2_sppt")),
            (Some(pl1), Some(pl2)) if pl1 > pl2
        );
        self.warning.set_visible(bad);
        if bad {
            self.warning
                .set_label(t("PL1 (long-term) cannot be higher than PL2 (short-term)."));
        }
        let editable = self.editable.get();
        self.apply.set_sensitive(editable && dirty && !bad);
        self.discard.set_sensitive(dirty);
        self.defaults
            .set_sensitive(editable && !self.rows.borrow().is_empty());
        for (name, r) in self.rows.borrow().iter() {
            if self.dirty.borrow().contains(name) {
                r.row.add_css_class("accent");
            } else {
                r.row.remove_css_class("accent");
            }
        }
    }

    /// Loads the firmware values into non-edited rows.
    fn load_current(&self) {
        self.updating.set(true);
        for (name, r) in self.rows.borrow().iter() {
            if self.dirty.borrow().contains(name) {
                continue;
            }
            if let (Some(adj), Some(v)) = (&r.adj, r.attr.current()) {
                adj.set_value(v as f64);
            }
        }
        self.updating.set(false);
        self.refresh_buttons();
    }

    /// Called after a successful change: forget local edits.
    pub fn on_done(&self) {
        self.dirty.borrow_mut().clear();
        self.refresh_buttons();
    }

    fn rebuild(self: &Rc<Self>, limits: &PowerLimits) {
        for r in std::mem::take(&mut *self.rows.borrow_mut()).into_values() {
            let group = if is_gpu(&r.attr.name) {
                &self.gpu_group
            } else {
                &self.cpu_group
            };
            group.remove(&r.row);
        }
        self.dirty.borrow_mut().clear();
        let mut rows = BTreeMap::new();
        for a in &limits.attributes {
            let (label, unit, help) = meta(&a.name);
            let title = if label.is_empty() {
                a.display_name.clone().unwrap_or_else(|| a.name.clone())
            } else {
                t(label).to_owned()
            };
            let row = adw::ActionRow::builder()
                .title(title)
                .use_markup(false)
                .build();
            let adj = match a.range() {
                Some((min, max, step)) => {
                    let adj = gtk::Adjustment::new(
                        a.current().unwrap_or(min) as f64,
                        min as f64,
                        max as f64,
                        step as f64,
                        (step * 5) as f64,
                        0.0,
                    );
                    let mut sub = format!("{min}–{max} {unit}");
                    if let Some(default) = &a.default_value {
                        sub = format!("{sub} · {} {default} {unit}", t("default"));
                    }
                    if !help.is_empty() {
                        sub = format!("{} · {sub}", t(help));
                    }
                    row.set_subtitle(&sub);
                    let scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&adj));
                    scale.set_draw_value(false);
                    scale.set_hexpand(false);
                    scale.set_width_request(240);
                    scale.set_valign(gtk::Align::Center);
                    let spin = gtk::SpinButton::new(Some(&adj), step as f64, 0);
                    spin.set_valign(gtk::Align::Center);
                    spin.set_width_chars(4);
                    let unit_l = gtk::Label::new(Some(unit));
                    unit_l.add_css_class("dim-label");
                    row.add_suffix(&scale);
                    row.add_suffix(&spin);
                    row.add_suffix(&unit_l);
                    let w = Rc::downgrade(self);
                    let name = a.name.clone();
                    adj.connect_value_changed(move |_| {
                        if let Some(p) = w.upgrade() {
                            if !p.updating.get() {
                                p.dirty.borrow_mut().insert(name.clone());
                                p.refresh_buttons();
                            }
                        }
                    });
                    Some(adj)
                }
                None => {
                    row.set_subtitle(&tr(&format!(
                        "{} = {} (fixed by firmware)",
                        a.name,
                        a.current_value.as_deref().unwrap_or("?")
                    )));
                    let v = gtk::Label::new(Some(&format!(
                        "{} {unit}",
                        a.current_value.as_deref().unwrap_or("?")
                    )));
                    v.add_css_class("dim-label");
                    row.add_suffix(&v);
                    None
                }
            };
            let group = if is_gpu(&a.name) {
                &self.gpu_group
            } else {
                &self.cpu_group
            };
            group.add(&row);
            rows.insert(
                a.name.clone(),
                LimitRow {
                    row,
                    adj,
                    attr: a.clone(),
                },
            );
        }
        *self.rows.borrow_mut() = rows;
    }

    /// Updates from a daemon snapshot.
    pub fn update(self: &Rc<Self>, s: &Snapshot) {
        let lim = &s.limits;
        self.updating.set(true);
        self.allow.set_active(s.info.power_limits_enabled);
        self.updating.set(false);
        let custom = s.status.profile.current == PowerProfile::Custom || lim.custom_profile_active;
        self.profile_btn.set_visible(!custom);
        self.profile_row.set_subtitle(t(if custom {
            "Custom mode is active: applied values take effect immediately. Selecting another profile restores the firmware values."
        } else {
            "The values only take effect in Custom mode."
        }));

        // rebuild when the set of attributes or their ranges change
        let key: String = lim
            .attributes
            .iter()
            .map(|a| format!("{}:{:?};", a.name, a.range()))
            .collect();
        if *self.layout_key.borrow() != key {
            self.rebuild(lim);
            *self.layout_key.borrow_mut() = key;
        } else {
            // keep the latest firmware values for "Discard"/current reads
            let mut rows = self.rows.borrow_mut();
            for a in &lim.attributes {
                if let Some(r) = rows.get_mut(&a.name) {
                    r.attr = a.clone();
                }
            }
        }
        if lim.attributes.is_empty() {
            self.cpu_group
                .set_description(Some(t("Not supported on this hardware.")));
        } else {
            self.cpu_group.set_description(None);
        }
        self.gpu_group
            .set_visible(lim.attributes.iter().any(|a| is_gpu(&a.name)));

        let editable = s.info.power_limits_enabled && custom;
        self.editable.set(editable);
        for r in self.rows.borrow().values() {
            r.row.set_sensitive(editable || r.adj.is_none());
        }
        self.load_current();
    }
}
