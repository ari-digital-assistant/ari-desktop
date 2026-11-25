//! What the engine reads from the desktop: the user's language, the local
//! time for skills, and somewhere for skills' log lines to go.

use std::sync::RwLock;

use ari_skill_loader::{LocalClock, LocalTimeComponents, LocaleProvider, LogLevel, LogSink};
use gtk4::glib;
use tracing::{debug, info, warn};

/// The languages Ari speaks, as the engine's locale codes.
pub const LANGUAGES: [&str; 2] = ["en", "it"];

/// A language by its code, in its own words, so it's found whatever the
/// UI is in. A code Ari doesn't speak shows as itself.
pub fn language_name(code: &str) -> String {
    match code {
        "en" => "English".to_string(),
        "it" => "Italiano".to_string(),
        other => other.to_uppercase(),
    }
}

/// The language Ari answers in, changed from Preferences while it runs.
pub struct Locale(RwLock<String>);

impl Locale {
    pub fn new(code: String) -> Self {
        Self(RwLock::new(code))
    }

    pub fn set(&self, code: String) {
        *self.0.write().expect("locale lock poisoned") = code;
    }
}

impl LocaleProvider for Locale {
    fn current_locale(&self) -> String {
        self.0.read().expect("locale lock poisoned").clone()
    }
}

/// The language for the `language` setting: the chosen one, or the system's
/// first language Ari speaks, or English.
pub fn language(setting: &str) -> String {
    let system = glib::language_names();
    let system = system.iter().map(|name| name.as_str());
    std::iter::once(setting)
        .chain(system)
        .find_map(|name| {
            let code = name.split(['_', '.', '@']).next()?;
            LANGUAGES.contains(&code).then(|| code.to_string())
        })
        .unwrap_or_else(|| "en".to_string())
}

pub struct Clock;

impl LocalClock for Clock {
    fn now_components(&self) -> LocalTimeComponents {
        components(&glib::DateTime::now_local().expect("the local time"))
    }

    fn timezone_id(&self) -> String {
        glib::TimeZone::local().identifier().to_string()
    }
}

fn components(time: &glib::DateTime) -> LocalTimeComponents {
    LocalTimeComponents {
        year: time.year(),
        month: time.month() as u8,
        day: time.day_of_month() as u8,
        hour: time.hour() as u8,
        minute: time.minute() as u8,
        second: time.second() as u8,
        // GLib counts Monday as 1, skills as 0.
        weekday: (time.day_of_week() - 1) as u8,
        tz_id: time.timezone().identifier().to_string(),
    }
}

pub struct SkillLog;

impl LogSink for SkillLog {
    fn log(&self, skill_id: &str, level: LogLevel, message: &str) {
        match level {
            LogLevel::Trace | LogLevel::Debug => debug!(target: "skill", skill_id, "{message}"),
            LogLevel::Info => info!(target: "skill", skill_id, "{message}"),
            LogLevel::Warn | LogLevel::Error => warn!(target: "skill", skill_id, "{message}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_language_wins() {
        assert_eq!(language("it"), "it");
        assert_eq!(language("en"), "en");
    }

    #[test]
    fn a_language_ari_lacks_falls_back_to_one_it_has() {
        assert!(LANGUAGES.contains(&language("xx").as_str()));
    }

    #[test]
    fn a_time_s_components_count_monday_as_0() {
        let london = glib::TimeZone::new(Some("Europe/London"));
        let monday = glib::DateTime::new(&london, 2026, 11, 23, 9, 30, 15.0).unwrap();
        assert_eq!(
            components(&monday),
            LocalTimeComponents {
                year: 2026,
                month: 11,
                day: 23,
                hour: 9,
                minute: 30,
                second: 15,
                weekday: 0,
                tz_id: "Europe/London".into(),
            }
        );
        let sunday = glib::DateTime::new(&london, 2026, 11, 29, 23, 59, 0.0).unwrap();
        assert_eq!(components(&sunday).weekday, 6);
    }
}
