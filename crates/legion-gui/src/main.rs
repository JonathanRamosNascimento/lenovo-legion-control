//! Legion Control — GTK4 / libadwaita interface.
//!
//! The GUI never reads or writes `/sys`: it only talks to
//! `legion-control-daemon` over D-Bus (see [`worker`]).
//!
//! Options:
//! * `--hidden`  start in the tray (used by the autostart entry);
//! * `--session` talk to a daemon on the session bus (development);
//! * `--page NAME` open on a given page (development).

mod autostart;
mod custom_page;
mod settings;
#[cfg(feature = "tray")]
mod tray;
mod ui;
mod worker;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use legion_dbus::Bus;
use legion_i18n::t;

const APP_ID: &str = "com.legion.Control.Gui";

const CSS: &str = "
.metric { font-size: 1.8em; font-weight: 800; }
toggle-group.round { margin: 6px 0; }
";

/// Events from the tray (or other sources) into the GTK thread.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Switch profile.
    Profile(legion_core::PowerProfile),
    /// Show the window.
    Open,
    /// Quit.
    Quit,
}

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().collect();
    let hidden = args.iter().any(|a| a == "--hidden");
    let page = args
        .iter()
        .position(|a| a == "--page")
        .and_then(|i| args.get(i + 1).cloned());
    let bus = if args.iter().any(|a| a == "--session") {
        Bus::Session
    } else {
        Bus::System
    };

    legion_i18n::set_lang(settings::Settings::load().effective_lang());

    let app = adw::Application::builder().application_id(APP_ID).build();
    let state: Rc<RefCell<Option<Rc<ui::Ui>>>> = Rc::new(RefCell::new(None));
    let first = Rc::new(std::cell::Cell::new(true));

    {
        let state = state.clone();
        app.connect_startup(move |app| {
            let provider = gtk::CssProvider::new();
            provider.load_from_string(CSS);
            gtk::style_context_add_provider_for_display(
                &gtk::gdk::Display::default().expect("display"),
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );

            let (tx, rx) = async_channel::unbounded();
            let cmd = worker::spawn(bus, tx);
            let ui = ui::build(app, cmd.clone(), page.as_deref());
            *state.borrow_mut() = Some(ui.clone());

            // worker → current UI (the UI may be rebuilt, e.g. on language change)
            let st = state.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = rx.recv().await {
                    let current = st.borrow().clone();
                    if let Some(u) = current {
                        u.handle(msg);
                    }
                }
            });

            // tray → UI
            let (etx, erx) = async_channel::unbounded::<AppEvent>();
            #[cfg(feature = "tray")]
            let tray_ok = tray::spawn(etx.clone()).is_ok();
            #[cfg(not(feature = "tray"))]
            let tray_ok = false;
            drop(etx);
            if tray_ok {
                // keep running in the tray when the window is closed
                ui.window.set_hide_on_close(true);
                std::mem::forget(app.hold());
            }
            let app2 = app.clone();
            let st = state.clone();
            let tray_cmd = cmd.clone();
            glib::spawn_future_local(async move {
                while let Ok(ev) = erx.recv().await {
                    match ev {
                        AppEvent::Profile(p) => {
                            let _ = tray_cmd.send(worker::Cmd::Profile(p));
                        }
                        AppEvent::Open => {
                            let current = st.borrow().clone();
                            if let Some(u) = current {
                                u.window.present();
                            }
                        }
                        AppEvent::Quit => app2.quit(),
                    }
                }
            });

            // actions
            let quit = gio::ActionEntry::builder("quit")
                .activate(|app: &adw::Application, _, _| app.quit())
                .build();
            let about = gio::ActionEntry::builder("about")
                .activate(|app: &adw::Application, _, _| {
                    let d = adw::AboutDialog::builder()
                        .application_name("Legion Control")
                        .application_icon("com.legion.Control")
                        .version(env!("CARGO_PKG_VERSION"))
                        .license_type(gtk::License::MitX11)
                        .comments(t("Monitor and control Lenovo Legion laptops on Linux using documented kernel interfaces."))
                        .build();
                    d.present(app.active_window().as_ref());
                })
                .build();
            let autostart = gio::ActionEntry::builder("autostart")
                .state(autostart::is_enabled().to_variant())
                .activate(|_: &adw::Application, action, _| {
                    let want = !action.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
                    match autostart::set(want) {
                        Ok(()) => action.set_state(&want.to_variant()),
                        Err(e) => tracing::warn!(error = %e, "autostart"),
                    }
                })
                .build();
            let initial = settings::Settings::load()
                .language
                .unwrap_or_else(|| "auto".into());
            let st = state.clone();
            let language = gio::ActionEntry::builder("language")
                .parameter_type(Some(glib::VariantTy::STRING))
                .state(initial.to_variant())
                .activate(move |app: &adw::Application, action, param| {
                    let Some(code) = param.and_then(|p| p.get::<String>()) else {
                        return;
                    };
                    let mut prefs = settings::Settings::load();
                    prefs.language = (code != "auto").then(|| code.clone());
                    if let Err(e) = prefs.save() {
                        tracing::warn!(error = %e, "saving preferences");
                    }
                    action.set_state(&code.to_variant());
                    legion_i18n::set_lang(prefs.effective_lang());
                    rebuild(app, &st, &cmd, tray_ok);
                })
                .build();
            app.add_action_entries([quit, about, autostart, language]);
            app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
            if !tray_ok && hidden {
                tracing::info!("no tray available; showing the window instead of starting hidden");
            }
            // remember whether the window must stay hidden on first activation
            set_tray_available(tray_ok);
        });
    }

    {
        let state = state.clone();
        app.connect_activate(move |_| {
            let Some(ui) = state.borrow().clone() else {
                return;
            };
            let stay_hidden = first.replace(false) && hidden && tray_available();
            if !stay_hidden {
                ui.window.present();
            }
        });
    }

    // GTK must not see our own flags
    app.run_with_args(&args[..1])
}

/// Rebuilds the window in the current language, keeping page, size and
/// visibility. The D-Bus worker keeps running; the next snapshot (≤ 1 s)
/// fills the new window.
fn rebuild(
    app: &adw::Application,
    state: &Rc<RefCell<Option<Rc<ui::Ui>>>>,
    cmd: &std::sync::mpsc::Sender<worker::Cmd>,
    tray_ok: bool,
) {
    let Some(old) = state.borrow().clone() else {
        return;
    };
    let new = ui::build(app, cmd.clone(), old.page().as_deref());
    new.window
        .set_default_size(old.window.width(), old.window.height());
    new.window.set_hide_on_close(tray_ok);
    let visible = old.window.is_visible();
    *state.borrow_mut() = Some(new.clone());
    if visible {
        new.window.present();
    }
    old.window.destroy();
}

thread_local! {
    static TRAY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn set_tray_available(v: bool) {
    TRAY.with(|t| t.set(v));
}

fn tray_available() -> bool {
    TRAY.with(|t| t.get())
}
