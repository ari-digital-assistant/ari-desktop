//! The Preferences dialog.

use gettextrs::gettext;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::warn;

use crate::{autostart, hyprland};

pub fn present(app: &adw::Application, settings: &gio::Settings) {
    let dialog = adw::PreferencesDialog::new();

    let start = adw::SwitchRow::builder()
        .title(gettext("Start at Login"))
        .subtitle(gettext(
            "Ari always runs in the background. This also starts it when you log in.",
        ))
        .build();
    settings.bind(autostart::KEY, &start, "active").build();

    let group = adw::PreferencesGroup::new();
    group.add(&start);
    if hyprland::running() {
        group.add(&hyprland_row(&dialog, settings));
    }

    let page = adw::PreferencesPage::builder()
        .title(gettext("General"))
        .icon_name("preferences-system-symbolic")
        .build();
    page.add(&group);
    dialog.add(&page);
    dialog.present(app.active_window().as_ref());
}

/// The offer to add Ari to Hyprland's config, which is what starts it at
/// login there. Only while "Start at Login" is on, which also takes it out
/// again when switched off (`autostart`).
fn hyprland_row(dialog: &adw::PreferencesDialog, settings: &gio::Settings) -> adw::ActionRow {
    let button = gtk4::Button::builder().valign(gtk4::Align::Center).build();
    let row = adw::ActionRow::builder()
        .title(gettext("Start with Hyprland"))
        .build();
    row.add_suffix(&button);
    settings
        .bind(autostart::KEY, &row, "sensitive")
        .get()
        .build();
    show_hyprland_state(&row, &button);

    button.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        row,
        move |button| toggle_hyprland(&dialog, &row, button)
    ));

    // Switching "Start at Login" off takes Ari out of the config; show that
    // once the handler doing it has run.
    let changed = settings.connect_changed(
        Some(autostart::KEY),
        glib::clone!(
            #[weak]
            row,
            #[weak]
            button,
            move |_, _| {
                glib::idle_add_local_once(move || show_hyprland_state(&row, &button));
            }
        ),
    );
    let settings = settings.clone();
    let changed = std::cell::Cell::new(Some(changed));
    dialog.connect_closed(move |_| {
        if let Some(id) = changed.take() {
            settings.disconnect(id);
        }
    });

    row
}

// Kept out of the `glib::clone!` above: xgettext doesn't look for strings
// inside macro calls.
fn toggle_hyprland(dialog: &adw::PreferencesDialog, row: &adw::ActionRow, button: &gtk4::Button) {
    if let Some(config) = hyprland::Config::find() {
        let result = match config.has_ari() {
            Ok(true) => config.remove_ari().map(|_| ()),
            Ok(false) => config.add_ari(),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            warn!(%error, path = %config.path().display(), "couldn't change the Hyprland config");
            let message = gettext("Couldn't change your Hyprland config: {}")
                .replace("{}", &error.to_string());
            dialog.add_toast(adw::Toast::new(&message));
        }
    }
    show_hyprland_state(row, button);
}

fn show_hyprland_state(row: &adw::ActionRow, button: &gtk4::Button) {
    let intro = gettext("Hyprland doesn't start apps at login by itself.");
    let Some(config) = hyprland::Config::find() else {
        row.set_subtitle(&format!(
            "{intro} {}",
            gettext("Ari couldn't find your Hyprland config to add itself to.")
        ));
        button.set_visible(false);
        return;
    };
    button.set_visible(true);
    match config.has_ari() {
        Ok(true) => {
            row.set_subtitle(&gettext(
                "Ari is in your Hyprland config, so it starts with Hyprland.",
            ));
            button.set_label(&gettext("Remove"));
        }
        Ok(false) => {
            row.set_subtitle(&format!(
                "{intro} {}",
                gettext("Ari can add itself to your Hyprland config.")
            ));
            button.set_label(&gettext("Add"));
        }
        Err(error) => {
            warn!(%error, path = %config.path().display(), "couldn't read the Hyprland config");
            row.set_subtitle(
                &gettext("Couldn't read your Hyprland config: {}")
                    .replace("{}", &error.to_string()),
            );
            button.set_visible(false);
        }
    }
}
