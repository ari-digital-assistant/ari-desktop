//! A skill's settings as rows to edit, for the skill's page and the
//! assistant's. Text, secrets and choices so far; the field types that need
//! sign-in or the user's calendar come with stage 8 of dev.md.

use std::rc::Rc;

use ari_runtime::settings::SettingField;
use gettextrs::gettext;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::ari::Ari;

/// What a row does once its value is kept: show what went wrong, or redraw
/// the fields, since a choice can show or hide another.
pub struct Changed {
    pub failed: Box<dyn Fn(String)>,
    pub saved: Box<dyn Fn()>,
}

/// The rows for `fields` of skill `skill`, leaving out any whose `show_when`
/// doesn't hold.
pub fn rows(
    ari: &Rc<Ari>,
    skill: &str,
    fields: &[SettingField],
    changed: Rc<Changed>,
) -> Vec<gtk4::Widget> {
    fields
        .iter()
        .filter(|field| shown(field, fields))
        .map(|field| row(ari, skill, field, changed.clone()))
        .collect()
}

fn shown(field: &SettingField, fields: &[SettingField]) -> bool {
    let Some(key) = &field.show_when_key else {
        return true;
    };
    let value = fields
        .iter()
        .find(|f| &f.key == key)
        .and_then(|f| f.current_value.clone().or_else(|| f.default_value.clone()));
    value.is_some_and(|v| field.show_when_equals.contains(&v))
}

fn row(ari: &Rc<Ari>, skill: &str, field: &SettingField, changed: Rc<Changed>) -> gtk4::Widget {
    let keep = {
        let (ari, skill, key) = (ari.clone(), skill.to_string(), field.key.clone());
        let secret = field.field_type == "secret";
        move |value: String| {
            let (ari, skill, key, changed) =
                (ari.clone(), skill.clone(), key.clone(), changed.clone());
            glib::spawn_future_local(async move {
                match ari.engine.settings.set(&skill, &key, &value, secret).await {
                    Ok(()) => (changed.saved)(),
                    Err(error) => (changed.failed)(error),
                }
            });
        }
    };
    match field.field_type.as_str() {
        "text" => {
            let entry = adw::EntryRow::builder()
                .title(&field.label)
                .text(field.current_value.as_deref().unwrap_or_default())
                .show_apply_button(true)
                .build();
            entry.connect_apply(move |entry| keep(entry.text().to_string()));
            with_help(entry.upcast(), field)
        }
        "secret" => {
            let entry = adw::PasswordEntryRow::builder().title(&field.label).build();
            // A saved secret is never handed out: the engine gives bullets in
            // its place, which show as dots here.
            let saved = field.current_value.clone().unwrap_or_default();
            entry.set_text(&saved);
            // After the text is set, so the dots don't count as an edit.
            entry.set_show_apply_button(true);
            if !saved.is_empty() {
                // Typing or pasting into the dots replaces them.
                let saved = saved.clone();
                entry.connect_changed(move |entry| {
                    let text = entry.text();
                    if text != saved && text.contains(saved.as_str()) {
                        entry.set_text(&text.replace(saved.as_str(), ""));
                        entry.set_position(-1);
                    }
                });
            }
            entry.connect_apply(move |entry| {
                let text = entry.text();
                if text != saved {
                    keep(text.to_string());
                }
            });
            with_help(entry.upcast(), field)
        }
        "select" => {
            let labels: Vec<&str> = field.options.iter().map(|o| o.label.as_str()).collect();
            let chosen = field
                .current_value
                .as_ref()
                .or(field.default_value.as_ref())
                .and_then(|value| field.options.iter().position(|o| &o.value == value));
            let combo = adw::ComboRow::builder()
                .title(&field.label)
                .model(&gtk4::StringList::new(&labels))
                .build();
            if let Some(subtitle) = &field.help_text {
                combo.set_subtitle(subtitle);
            }
            combo.set_selected(chosen.map_or(gtk4::INVALID_LIST_POSITION, |i| i as u32));
            let values: Vec<String> = field.options.iter().map(|o| o.value.clone()).collect();
            combo.connect_selected_notify(move |combo| {
                if let Some(value) = values.get(combo.selected() as usize) {
                    keep(value.clone());
                }
            });
            combo.upcast()
        }
        _ => adw::ActionRow::builder()
            .title(&field.label)
            .subtitle(gettext("Ari for Linux can't set this up yet."))
            .build()
            .upcast(),
    }
}

/// An entry row has no subtitle, so its help goes in a tooltip.
fn with_help(row: gtk4::Widget, field: &SettingField) -> gtk4::Widget {
    if let Some(help) = &field.help_text {
        row.set_tooltip_text(Some(help));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, current: Option<&str>, default: Option<&str>) -> SettingField {
        SettingField {
            key: key.into(),
            label: key.into(),
            field_type: "select".into(),
            required: false,
            default_value: default.map(Into::into),
            current_value: current.map(Into::into),
            options: Vec::new(),
            show_when_key: None,
            show_when_equals: Vec::new(),
            depends_on: Vec::new(),
            validate: false,
            help_text: None,
            collapsed_group: None,
            keyboard: None,
        }
    }

    fn gated(on: &str, equals: &[&str]) -> SettingField {
        SettingField {
            show_when_key: Some(on.into()),
            show_when_equals: equals.iter().map(|e| e.to_string()).collect(),
            ..field("gated", None, None)
        }
    }

    #[test]
    fn a_field_with_no_condition_is_shown() {
        let plain = field("tier", None, None);
        assert!(shown(&plain, std::slice::from_ref(&plain)));
    }

    #[test]
    fn a_condition_reads_the_current_value_first() {
        let gate = gated("mode", &["cloud"]);
        let fields = [field("mode", Some("cloud"), Some("local")), gate.clone()];
        assert!(shown(&gate, &fields));
        let fields = [field("mode", Some("local"), Some("cloud")), gate.clone()];
        assert!(!shown(&gate, &fields));
    }

    #[test]
    fn a_condition_falls_back_to_the_default() {
        let gate = gated("mode", &["cloud", "both"]);
        assert!(shown(
            &gate,
            &[field("mode", None, Some("both")), gate.clone()]
        ));
        assert!(!shown(&gate, &[field("mode", None, None), gate.clone()]));
    }
}
