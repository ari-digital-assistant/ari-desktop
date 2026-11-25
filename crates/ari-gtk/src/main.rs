//! Ari for Linux.
//!
//! ```text
//! ari-gtk [--background]
//! ```
//!
//! One process, for as long as the session lasts. A launch with no arguments
//! opens the window, or brings back the one already open; `--background`
//! starts Ari without a window, which is how it starts at login. Closing the
//! window never quits it.

mod autostart;
mod hyprland;
mod i18n;
mod preferences;
mod window;

use clap::Parser;
use gtk4::gio::prelude::ApplicationCommandLineExt;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use tracing::{error, warn};

const APP_ID: &str = "dev.heyari.Ari";

#[derive(Debug, Parser)]
#[command(
    name = "ari-gtk",
    version,
    about = "Ari, the open-source digital assistant."
)]
struct Args {
    /// Start without opening the window, as Ari does at login.
    #[arg(long)]
    background: bool,
}

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Parsed here so that `--help`, `--version` and a mistyped flag are
    // answered on the terminal that ran this, before anything starts. The
    // instance that is already running parses them again in
    // `connect_command_line`, since a launch is forwarded to it.
    Args::parse();

    i18n::init();

    // ashpd talks D-Bus through zbus on Tokio, and zbus spawns its tasks into
    // whichever runtime is current. Everything of ours runs on GLib's main
    // loop; this runtime only gives zbus somewhere to live.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(%error, "the D-Bus runtime could not be started");
            return glib::ExitCode::FAILURE;
        }
    };
    let _runtime = runtime.enter();

    // A missing schema makes GSettings abort the process, so say why first.
    if gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(APP_ID, true))
        .is_none()
    {
        error!(
            schema = APP_ID,
            "the GSettings schema isn't installed; run Ari with `just run`, or install it with `just install-dev`"
        );
        return glib::ExitCode::FAILURE;
    }
    let settings = gio::Settings::new(APP_ID);

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup(move |app| {
        // Ari runs until the session ends, window or no window, so the hold
        // is never released.
        std::mem::forget(app.hold());
        autostart::follow(&settings);
        install_actions(app, &settings);
    });

    app.connect_command_line(|app, command_line| {
        let args = match Args::try_parse_from(command_line.arguments()) {
            Ok(args) => args,
            // The launching process already reported this on its terminal.
            Err(error) => {
                warn!(%error, "ignoring a launch whose arguments don't parse");
                return glib::ExitCode::FAILURE;
            }
        };
        if !args.background {
            app.activate();
        }
        glib::ExitCode::SUCCESS
    });

    app.connect_activate(window::present);

    app.run()
}

fn install_actions(app: &adw::Application, settings: &gio::Settings) {
    let preferences = gio::SimpleAction::new("preferences", None);
    let settings = settings.clone();
    preferences.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| preferences::present(&app, &settings)
    ));
    app.add_action(&preferences);
    app.set_accels_for_action("app.preferences", &["<Control>comma"]);
}
