//! The alarms Ari keeps and rings itself, as Linux has no alarm clock that
//! every desktop shares. Kept on disk with the time each rings next.

use std::path::PathBuf;

use gtk4::prelude::*;
use gtk4::{gio, glib};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::store;

/// The days an alarm can repeat on, as skills name them, Monday first.
pub const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alarm {
    pub id: u64,
    pub hour: i32,
    pub minute: i32,
    pub message: Option<String>,
    /// The days it repeats on, from [`DAYS`]. Empty: it rings once.
    pub days: Vec<String>,
    /// When it rings next, Unix ms.
    pub next_ms: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    alarms: Vec<Alarm>,
    next_id: u64,
}

pub struct Alarms {
    path: PathBuf,
    kept: Kept,
}

impl Alarms {
    pub fn open(path: PathBuf) -> Self {
        let kept = store::load(&path);
        Self { path, kept }
    }

    /// Soonest first.
    pub fn all(&self) -> &[Alarm] {
        &self.kept.alarms
    }

    /// Set an alarm for `hour`:`minute` after `now`. None when that isn't a
    /// time of day.
    pub fn add(
        &mut self,
        now: &glib::DateTime,
        hour: i32,
        minute: i32,
        message: Option<String>,
        days: Vec<String>,
    ) -> Option<&Alarm> {
        let days: Vec<String> = days
            .into_iter()
            .map(|day| day.to_lowercase())
            .filter(|day| DAYS.contains(&day.as_str()))
            .collect();
        let next_ms = next_after(now, hour, minute, &days)?;
        self.kept.next_id += 1;
        let alarm = Alarm {
            id: self.kept.next_id,
            hour,
            minute,
            message,
            days,
            next_ms,
        };
        self.kept.alarms.push(alarm);
        self.sort();
        let id = self.kept.next_id;
        self.kept.alarms.iter().find(|a| a.id == id)
    }

    pub fn remove(&mut self, id: u64) -> Option<Alarm> {
        let at = self.kept.alarms.iter().position(|a| a.id == id)?;
        Some(self.kept.alarms.remove(at))
    }

    /// Take out the alarms due by `now`, setting each repeating one for its
    /// next day. What they were when due.
    pub fn due(&mut self, now: &glib::DateTime) -> Vec<Alarm> {
        let now_ms = now.to_unix() * 1000;
        let (due, mut kept): (Vec<Alarm>, Vec<Alarm>) = std::mem::take(&mut self.kept.alarms)
            .into_iter()
            .partition(|a| a.next_ms <= now_ms);
        for alarm in &due {
            if alarm.days.is_empty() {
                continue;
            }
            if let Some(next_ms) = next_after(now, alarm.hour, alarm.minute, &alarm.days) {
                kept.push(Alarm {
                    next_ms,
                    ..alarm.clone()
                });
            }
        }
        self.kept.alarms = kept;
        self.sort();
        due
    }

    fn sort(&mut self) {
        self.kept.alarms.sort_by_key(|a| a.next_ms);
    }

    pub fn save(&self) {
        if let Err(error) = store::save(&self.path, &self.kept) {
            warn!(%error, path = %self.path.display(), "couldn't save the alarms");
        }
    }
}

/// The first `hour`:`minute` after `now`, in `now`'s time zone, on one of
/// `days` when there are any, as Unix ms. None when it isn't a time of day.
pub fn next_after(now: &glib::DateTime, hour: i32, minute: i32, days: &[String]) -> Option<i64> {
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    let zone = now.timezone();
    (0..=7).find_map(|offset| {
        let day = now.add_days(offset).ok()?;
        let at = glib::DateTime::new(
            &zone,
            day.year(),
            day.month(),
            day.day_of_month(),
            hour,
            minute,
            0.0,
        )
        .ok()?;
        let weekday = DAYS[(at.day_of_week() - 1) as usize];
        let on_day = days.is_empty() || days.iter().any(|d| d == weekday);
        (at > *now && on_day).then(|| at.to_unix() * 1000)
    })
}

/// `hour`:`minute` as the desktop shows the time: 12-hour where GNOME's
/// clock is set to it, else 24-hour.
pub fn clock(hour: i32, minute: i32) -> String {
    let twelve = gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup("org.gnome.desktop.interface", true))
        .is_some_and(|schema| {
            schema.has_key("clock-format")
                && gio::Settings::new("org.gnome.desktop.interface").string("clock-format") == "12h"
        });
    let format = if twelve { "%-l:%M %p" } else { "%H:%M" };
    glib::DateTime::from_local(2000, 1, 1, hour, minute, 0.0)
        .and_then(|time| time.format(format))
        .map(|time| time.to_string())
        .unwrap_or_else(|_| format!("{hour:02}:{minute:02}"))
}

/// `days` by their short names in the user's language, Monday first.
pub fn day_names(days: &[String]) -> String {
    DAYS.iter()
        .enumerate()
        .filter(|(_, day)| days.iter().any(|d| d == *day))
        .filter_map(|(index, _)| {
            // 1 January 2024 was a Monday.
            let date = glib::DateTime::from_local(2024, 1, 1 + index as i32, 12, 0, 0.0).ok()?;
            Some(date.format("%a").ok()?.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday 25 November 2026, `hour`:`minute`, UTC.
    fn wednesday(hour: i32, minute: i32) -> glib::DateTime {
        glib::DateTime::new(&glib::TimeZone::utc(), 2026, 11, 25, hour, minute, 0.0).unwrap()
    }

    fn at(day: i32, hour: i32, minute: i32) -> i64 {
        glib::DateTime::new(&glib::TimeZone::utc(), 2026, 11, day, hour, minute, 0.0)
            .unwrap()
            .to_unix()
            * 1000
    }

    fn days(list: &[&str]) -> Vec<String> {
        list.iter().map(|d| d.to_string()).collect()
    }

    #[test]
    fn a_one_off_rings_later_today_or_else_tomorrow() {
        assert_eq!(next_after(&wednesday(6, 0), 7, 0, &[]), Some(at(25, 7, 0)));
        assert_eq!(next_after(&wednesday(8, 0), 7, 0, &[]), Some(at(26, 7, 0)));
        // At the very minute, it's tomorrow's.
        assert_eq!(next_after(&wednesday(7, 0), 7, 0, &[]), Some(at(26, 7, 0)));
    }

    #[test]
    fn a_repeating_alarm_rings_on_its_next_day() {
        assert_eq!(
            next_after(&wednesday(8, 0), 7, 0, &days(&["mon", "fri"])),
            Some(at(27, 7, 0))
        );
        assert_eq!(
            next_after(&wednesday(6, 0), 7, 0, &days(&["wed"])),
            Some(at(25, 7, 0))
        );
        // Wednesday's has gone, so it's next week's.
        assert_eq!(
            next_after(&wednesday(8, 0), 7, 0, &days(&["wed"])),
            Some(at(25, 7, 0) + 7 * 24 * 3_600_000)
        );
    }

    #[test]
    fn a_time_that_isn_t_one_sets_nothing() {
        assert_eq!(next_after(&wednesday(6, 0), 24, 0, &[]), None);
        assert_eq!(next_after(&wednesday(6, 0), 7, 60, &[]), None);
        assert_eq!(next_after(&wednesday(6, 0), -1, 0, &[]), None);
    }

    #[test]
    fn due_alarms_go_unless_they_repeat_and_are_kept_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alarms.json");
        let mut alarms = Alarms::open(path.clone());
        let once = alarms
            .add(&wednesday(6, 0), 7, 0, Some("Wake up".into()), vec![])
            .unwrap()
            .id;
        let weekdays = alarms
            .add(
                &wednesday(6, 0),
                7,
                30,
                None,
                days(&["MON", "wed", "someday"]),
            )
            .unwrap()
            .clone();
        assert_eq!(weekdays.days, ["mon", "wed"]);
        alarms.add(&wednesday(6, 0), 9, 0, None, vec![]);
        alarms.save();

        let mut alarms = Alarms::open(path);
        let due = alarms.due(&wednesday(7, 45));
        assert_eq!(
            due.iter().map(|a| a.id).collect::<Vec<_>>(),
            [once, weekdays.id]
        );
        let left: Vec<(i32, i32, i64)> = alarms
            .all()
            .iter()
            .map(|a| (a.hour, a.minute, a.next_ms))
            .collect();
        assert_eq!(left, [(9, 0, at(25, 9, 0)), (7, 30, at(30, 7, 30))]);
    }

    #[test]
    fn a_removed_alarm_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let mut alarms = Alarms::open(dir.path().join("alarms.json"));
        let id = alarms.add(&wednesday(6, 0), 7, 0, None, vec![]).unwrap().id;
        assert_eq!(alarms.remove(id).map(|a| a.hour), Some(7));
        assert!(alarms.remove(id).is_none());
        assert!(alarms.all().is_empty());
    }
}
