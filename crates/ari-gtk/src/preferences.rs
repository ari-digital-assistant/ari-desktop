//! The Preferences dialog.

use gettextrs::gettext;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

use tracing::warn;

use crate::ari::{Ari, Download, SPEECH_KEY};
use crate::fields::{self, Changed};
use crate::links::{MAPS, SEARCH_ENGINES};
use crate::providers::{self, LANGUAGES, language_name};
use crate::skills::saving_failed;
use crate::speech_models;
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
    page.add(&web_group(settings));
    dialog.add(&page);
    dialog.add(&speech_page(settings, ari, &dialog));
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

/// Where a search and directions Ari opens go.
fn web_group(settings: &gio::Settings) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Searches and Directions"))
        .build();
    group.add(&choice_combo(
        settings,
        "search-engine",
        &gettext("Search With"),
        &SEARCH_ENGINES,
    ));
    group.add(&choice_combo(
        settings,
        "maps",
        &gettext("Directions From"),
        &MAPS,
    ));
    group
}

/// A row choosing setting `key` from `choices`: each its value, its name and
/// its address.
fn choice_combo(
    settings: &gio::Settings,
    key: &'static str,
    title: &str,
    choices: &'static [(&'static str, &'static str, &'static str)],
) -> adw::ComboRow {
    let names: Vec<&str> = choices.iter().map(|(_, name, _)| *name).collect();
    let combo = adw::ComboRow::builder()
        .title(title)
        .model(&gtk4::StringList::new(&names))
        .build();
    let current = settings.string(key);
    let at = choices.iter().position(|(value, _, _)| *value == current);
    combo.set_selected(at.unwrap_or(0) as u32);
    combo.connect_selected_notify(glib::clone!(
        #[weak]
        settings,
        move |combo| {
            if let Some((value, _, _)) = choices.get(combo.selected() as usize) {
                let _ = settings.set_string(key, value);
            }
        }
    ));
    combo
}

/// The `speech-recognition` setting's values, in the order offered.
const RECOGNITION: [&str; 3] = ["on-device", "openai", "self-hosted"];

/// How Ari turns speech into words: on this computer, with a model for
/// Ari's language to download, or by a cloud service, with its address and
/// key.
fn speech_page(
    settings: &gio::Settings,
    ari: &Rc<Ari>,
    dialog: &adw::PreferencesDialog,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(gettext("Speech"))
        .icon_name("audio-input-microphone-symbolic")
        .build();

    let labels = [
        gettext("On This Computer"),
        gettext("OpenAI"),
        gettext("Your Own Server"),
    ];
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let recognition = adw::ComboRow::builder()
        .title(gettext("Recognise Speech"))
        .model(&gtk4::StringList::new(&labels))
        .build();
    let current = settings.string("speech-recognition");
    recognition.set_selected(RECOGNITION.iter().position(|r| *r == current).unwrap_or(0) as u32);
    let how = adw::PreferencesGroup::new();
    how.add(&recognition);
    page.add(&how);

    let models = model_group(settings, ari);
    let openai = adw::PreferencesGroup::builder()
        .title(gettext("OpenAI"))
        .description(gettext(
            "Paste your OpenAI API key. Nothing else to set up.",
        ))
        .build();
    openai.add(&speech_key(ari, dialog));
    let server = adw::PreferencesGroup::builder()
        .title(gettext("Your Own Server"))
        .description(gettext(
            "Any OpenAI-compatible transcription endpoint: whisper.cpp, faster-whisper, or Home Assistant's Whisper add-on.",
        ))
        .build();
    for (key, title) in [
        ("speech-server", gettext("Address")),
        ("speech-server-model", gettext("Model")),
    ] {
        let row = adw::EntryRow::builder()
            .title(title)
            .text(settings.string(key).as_str())
            .show_apply_button(true)
            .build();
        row.connect_apply(glib::clone!(
            #[weak]
            settings,
            move |row| {
                let _ = settings.set_string(key, row.text().trim());
            }
        ));
        server.add(&row);
    }
    server.add(&speech_key(ari, dialog));
    page.add(&models);
    page.add(&openai);
    page.add(&server);

    let show = move |recognition: &adw::ComboRow| {
        let at = recognition.selected() as usize;
        recognition.set_subtitle(&match at {
            1 => gettext("Most accurate. Needs an API key and a connection."),
            2 => gettext("Accurate, and your audio stays on your network."),
            _ => gettext("Transcribes on your computer. Works offline."),
        });
        models.set_visible(at == 0);
        openai.set_visible(at == 1);
        server.set_visible(at == 2);
    };
    show(&recognition);
    recognition.connect_selected_notify(glib::clone!(
        #[weak]
        settings,
        move |recognition| {
            if let Some(value) = RECOGNITION.get(recognition.selected() as usize) {
                let _ = settings.set_string("speech-recognition", value);
            }
            show(recognition);
        }
    ));
    page
}

/// The models for Ari's language, recommended first, each to pick and
/// download.
fn model_group(settings: &gio::Settings, ari: &Rc<Ari>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Speech Model"))
        .description(gettext(
            "Speech models for Ari's language. Each downloads once, then works offline.",
        ))
        .build();
    let choices = ari_voice::stt_models::on_device_choices(&ari.engine.language());
    let picked = settings.string("speech-model");
    let chosen = ari_voice::stt_models::on_device_for(
        &ari.engine.language(),
        Some(picked.as_str()).filter(|p| !p.is_empty()),
    );
    let mut first: Option<gtk4::CheckButton> = None;
    for (index, model) in choices.into_iter().enumerate() {
        let (name, description) = speech_models::describe(&model);
        let title = if index == 0 {
            gettext("{} (recommended)").replace("{}", &name)
        } else {
            name
        };
        let check = gtk4::CheckButton::builder()
            .active(model == chosen)
            .valign(gtk4::Align::Center)
            .build();
        if let Some(first) = &first {
            check.set_group(Some(first));
        } else {
            first = Some(check.clone());
        }
        check.connect_toggled(glib::clone!(
            #[weak]
            settings,
            move |check| {
                if check.is_active() {
                    let _ = settings.set_string("speech-model", model.id);
                }
            }
        ));
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(description)
            .activatable_widget(&check)
            .build();
        row.add_prefix(&check);
        let state = gtk4::Stack::builder().valign(gtk4::Align::Center).build();
        let download = gtk4::Button::builder()
            .label(gettext("Download"))
            .css_classes(["pill"])
            .build();
        let progress = gtk4::ProgressBar::builder().width_request(120).build();
        let downloaded = gtk4::Label::builder()
            .label(gettext("Downloaded"))
            .css_classes(["dim-label"])
            .build();
        let failed = gtk4::Button::builder()
            .label(gettext("Try Again"))
            .css_classes(["pill"])
            .build();
        state.add_named(&download, Some("download"));
        state.add_named(&progress, Some("progress"));
        state.add_named(&downloaded, Some("downloaded"));
        state.add_named(&failed, Some("failed"));
        row.add_suffix(&state);
        state.set_visible_child_name(
            if speech_models::downloaded(&model, &speech_models::dir(&model)) {
                "downloaded"
            } else {
                "download"
            },
        );
        let follow = Rc::new(glib::clone!(
            #[weak]
            state,
            #[weak]
            progress,
            #[weak]
            failed,
            move |mut changes: tokio::sync::watch::Receiver<Download>| {
                glib::spawn_future_local(glib::clone!(
                    #[weak]
                    state,
                    #[weak]
                    progress,
                    #[weak]
                    failed,
                    async move {
                        loop {
                            let now = changes.borrow_and_update().clone();
                            match now {
                                Download::Running(done, total) => {
                                    state.set_visible_child_name("progress");
                                    progress.set_fraction(done as f64 / total.max(1) as f64);
                                }
                                Download::Done => {
                                    state.set_visible_child_name("downloaded");
                                    return;
                                }
                                Download::Failed(error) => {
                                    state.set_visible_child_name("failed");
                                    failed.set_tooltip_text(Some(&error));
                                    return;
                                }
                            }
                            if changes.changed().await.is_err() {
                                state.set_visible_child_name("failed");
                                return;
                            }
                        }
                    }
                ));
            }
        ));
        if let Some(running) = ari.speech_download(&model) {
            follow(running);
        }
        for button in [&download, &failed] {
            button.connect_clicked(glib::clone!(
                #[weak]
                ari,
                #[strong]
                follow,
                move |_| follow(ari.download_speech(model))
            ));
        }
        group.add(&row);
    }
    group
}

/// The cloud transcription key, kept in the keyring. A saved key shows as
/// dots, which typing replaces.
fn speech_key(ari: &Rc<Ari>, dialog: &adw::PreferencesDialog) -> adw::PasswordEntryRow {
    const SAVED: &str = "••••••••";
    let (skill, key) = SPEECH_KEY;
    let entry = adw::PasswordEntryRow::builder()
        .title(gettext("API Key"))
        .build();
    let saved = ari
        .engine
        .settings
        .get(skill, key)
        .is_some_and(|k| !k.is_empty());
    if saved {
        entry.set_text(SAVED);
        entry.connect_changed(|entry| {
            let text = entry.text();
            if text != SAVED && text.contains(SAVED) {
                entry.set_text(&text.replace(SAVED, ""));
                entry.set_position(-1);
            }
        });
    }
    // After the text is set, so the dots don't count as an edit.
    entry.set_show_apply_button(true);
    entry.connect_apply(glib::clone!(
        #[weak]
        ari,
        #[weak]
        dialog,
        move |entry| {
            let text = entry.text().trim().to_string();
            if text == SAVED {
                return;
            }
            glib::spawn_future_local(async move {
                match ari.engine.settings.set(skill, key, &text, true).await {
                    Ok(()) => ari.follow_speech(),
                    Err(error) => {
                        dialog.add_toast(adw::Toast::new(&saving_failed(&error)));
                    }
                }
            });
        }
    ));
    entry
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
