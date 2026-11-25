//! The Preferences dialog.

use gettextrs::gettext;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

use tracing::warn;

use crate::ari::Ari;
use crate::fields::{self, Changed};
use crate::providers::{self, LANGUAGES, language_name};
use crate::skills::saving_failed;
use crate::{autostart, hyprland};

pub fn present(app: &adw::Application, settings: &gio::Settings, ari: &Rc<Ari>) {
    let dialog = adw::PreferencesDialog::new();

    let start = adw::SwitchRow::builder()
        .title(gettext("Start at Login"))
        .subtitle(gettext(
            "Ari always runs in the background. This also starts it when you log in.",
        ))
        .build();
    settings.bind(autostart::KEY, &start, "active").build();

    let group = adw::PreferencesGroup::builder()
        .title(gettext("Startup"))
        .build();
    group.add(&start);
    if hyprland::running() {
        group.add(&hyprland_row(&dialog, settings));
    }

    let page = adw::PreferencesPage::builder()
        .title(gettext("General"))
        .icon_name("preferences-system-symbolic")
        .build();
    page.add(&group);
    page.add(&language_group(settings));
    page.add(&conversation_group(settings));
    dialog.add(&page);
    dialog.add(&assistant_page(settings, ari, &dialog));
    dialog.present(app.active_window().as_ref());
}

fn language_group(settings: &gio::Settings) -> adw::PreferencesGroup {
    let names: Vec<String> = LANGUAGES.iter().map(|code| language_name(code)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let combo = adw::ComboRow::builder()
        .title(gettext("Language"))
        .subtitle(gettext("The language Ari understands and answers in."))
        .model(&gtk4::StringList::new(&names))
        .build();
    let current = providers::language(&settings.string("language"));
    if let Some(index) = LANGUAGES.iter().position(|code| *code == current) {
        combo.set_selected(index as u32);
    }
    combo.connect_selected_notify(glib::clone!(
        #[weak]
        settings,
        move |combo| {
            if let Some(code) = LANGUAGES.get(combo.selected() as usize) {
                let _ = settings.set_string("language", code);
            }
        }
    ));
    let group = adw::PreferencesGroup::new();
    group.add(&combo);
    group
}

const LENGTHS: [&str; 3] = ["short", "medium", "long"];

fn conversation_group(settings: &gio::Settings) -> adw::PreferencesGroup {
    let memory = adw::SwitchRow::builder()
        .title(gettext("Remember Conversations"))
        .subtitle(gettext(
            "Ari keeps the last few things said, so it can answer follow-up questions in context.",
        ))
        .build();
    settings
        .bind("conversation-memory", &memory, "active")
        .build();

    let labels = [gettext("Short"), gettext("Medium"), gettext("Long")];
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let length = adw::ComboRow::builder()
        .title(gettext("How Long Ari Remembers"))
        .model(&gtk4::StringList::new(&labels))
        .build();
    settings
        .bind("conversation-memory", &length, "sensitive")
        .get()
        .build();
    let current = settings.string("conversation-length");
    length.set_selected(LENGTHS.iter().position(|l| *l == current).unwrap_or(0) as u32);
    describe_length(&length);
    length.connect_selected_notify(glib::clone!(
        #[weak]
        settings,
        move |length| {
            if let Some(name) = LENGTHS.get(length.selected() as usize) {
                let _ = settings.set_string("conversation-length", name);
            }
            describe_length(length);
        }
    ));

    let group = adw::PreferencesGroup::builder()
        .title(gettext("Conversation"))
        .build();
    group.add(&memory);
    group.add(&length);
    group
}

fn describe_length(row: &adw::ComboRow) {
    let blurb = match row.selected() {
        1 => gettext("Ten exchanges, forgotten after 5 minutes of quiet."),
        2 => gettext("Twenty exchanges, forgotten after 15 minutes of quiet."),
        _ => gettext("Five exchanges, forgotten after 90 seconds of quiet."),
    };
    row.set_subtitle(&blurb);
}

/// Who answers when no skill does: nobody, or an installed assistant, with
/// the chosen one's settings below. The on-device assistant joins with
/// stage 9 of dev.md.
fn assistant_page(
    settings: &gio::Settings,
    ari: &Rc<Ari>,
    dialog: &adw::PreferencesDialog,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(gettext("Assistant"))
        .icon_name("assistant-symbolic")
        .build();
    let choices = adw::PreferencesGroup::builder()
        .title(gettext("When No Skill Matches"))
        .build();
    let config = adw::PreferencesGroup::new();
    let config_rows = gtk4::ListBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    config.add(&config_rows);
    page.add(&choices);
    page.add(&config);

    let assistants: Vec<_> = ari
        .engine
        .assistants
        .list_assistants()
        .into_iter()
        .filter(|a| a.provider == "api")
        .collect();
    if assistants.is_empty() {
        choices.set_description(Some(&gettext(
            "Install an assistant such as ChatGPT from the Skills page to have it answer.",
        )));
    }
    let active = settings.string("assistant").to_string();

    let none = choice_row(
        &gettext("None"),
        &gettext("Ari answers only what a skill does."),
        None,
    );
    choices.add(&none.0);
    let mut buttons = vec![(none.1.clone(), String::new())];
    for assistant in &assistants {
        let subtitle = if assistant.privacy == "cloud" {
            gettext("Your questions are sent to {}.").replace("{}", &assistant.name)
        } else {
            assistant.description.clone()
        };
        let (row, button) = choice_row(&assistant.name, &subtitle, Some(&none.1));
        choices.add(&row);
        buttons.push((button, assistant.id.clone()));
    }
    for (button, id) in &buttons {
        button.set_active(*id == active);
    }
    show_assistant_config(ari, &config, &config_rows, &active, dialog);

    for (button, id) in buttons {
        button.connect_toggled(glib::clone!(
            #[weak]
            settings,
            #[weak]
            ari,
            #[weak]
            config,
            #[weak]
            config_rows,
            #[weak]
            dialog,
            move |button| {
                if button.is_active() {
                    let _ = settings.set_string("assistant", &id);
                    show_assistant_config(&ari, &config, &config_rows, &id, &dialog);
                }
            }
        ));
    }
    page
}

fn choice_row(
    title: &str,
    subtitle: &str,
    group: Option<&gtk4::CheckButton>,
) -> (adw::ActionRow, gtk4::CheckButton) {
    let button = gtk4::CheckButton::builder()
        .valign(gtk4::Align::Center)
        .build();
    button.set_group(group);
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .use_markup(false)
        .activatable_widget(&button)
        .build();
    row.add_prefix(&button);
    (row, button)
}

/// The chosen assistant's settings, such as its API key, drawn again after
/// each change.
fn show_assistant_config(
    ari: &Rc<Ari>,
    group: &adw::PreferencesGroup,
    list: &gtk4::ListBox,
    id: &str,
    dialog: &adw::PreferencesDialog,
) {
    list.remove_all();
    let fields = if id.is_empty() {
        Vec::new()
    } else {
        ari.engine.assistants.get_assistant_config(id)
    };
    group.set_visible(!fields.is_empty());
    let changed = Rc::new(Changed {
        failed: Box::new(glib::clone!(
            #[weak]
            dialog,
            move |error| dialog.add_toast(adw::Toast::new(&saving_failed(&error)))
        )),
        saved: Box::new(glib::clone!(
            #[weak]
            ari,
            #[weak]
            group,
            #[weak]
            list,
            #[weak]
            dialog,
            #[to_owned]
            id,
            move || show_assistant_config(&ari, &group, &list, &id, &dialog)
        )),
    });
    for row in fields::rows(ari, id, &fields, changed) {
        list.append(&row);
    }
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
