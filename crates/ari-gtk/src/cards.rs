//! The cards skills put in the conversation, kept on disk so a timer's card
//! and its deadline outlast a restart, and drawn as widgets.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use ari_core::envelope::{
    Accent, ButtonStyle, Card, CardAction, IconText, ListCard, PresentationEnvelope, Stat,
};
use gtk4::glib;
use gtk4::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::warn;

use crate::store;

/// A card as its skill sent it, which is how it's kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Kept {
    skill_id: String,
    card: Value,
    /// Its countdown has reached its deadline, which has been dealt with.
    #[serde(default)]
    done: bool,
}

pub struct Cards {
    path: PathBuf,
    /// Oldest first, each with what it was read from.
    cards: Vec<(Card, Kept)>,
}

impl Cards {
    pub fn open(path: PathBuf) -> Self {
        let kept: Vec<Kept> = store::load(&path);
        let cards = kept
            .into_iter()
            .filter_map(|kept| Some((read(&kept)?, kept)))
            .collect();
        Self { path, cards }
    }

    pub fn get(&self, id: &str) -> Option<&Card> {
        self.cards.iter().map(|(card, _)| card).find(|c| c.id == id)
    }

    /// Every card, oldest first.
    pub fn all(&self) -> impl Iterator<Item = &Card> {
        self.cards.iter().map(|(card, _)| card)
    }

    /// The cards in skill `skill_id`'s envelope `json`, each replacing the
    /// one with its id. What they are, in the envelope's order.
    pub fn put(&mut self, json: &Value, skill_id: &str) -> Vec<Card> {
        let raw = json.get("cards").and_then(Value::as_array);
        let mut put = Vec::new();
        for card in raw.into_iter().flatten() {
            let kept = Kept {
                skill_id: skill_id.to_string(),
                card: card.clone(),
                done: false,
            };
            let Some(card) = read(&kept) else { continue };
            match self.cards.iter_mut().find(|(c, _)| c.id == card.id) {
                Some(slot) => *slot = (card.clone(), kept),
                None => self.cards.push((card.clone(), kept)),
            }
            put.push(card);
        }
        put
    }

    /// The cards whose countdown has something to do at its deadline, which
    /// hasn't come yet or hasn't been dealt with.
    pub fn pending(&self) -> impl Iterator<Item = &Card> {
        self.cards
            .iter()
            .filter(|(card, kept)| {
                !kept.done && card.countdown_to_ts_ms.is_some() && card.on_complete.is_some()
            })
            .map(|(card, _)| card)
    }

    /// Card `id`'s deadline has been dealt with.
    pub fn done(&mut self, id: &str) {
        if let Some((_, kept)) = self.cards.iter_mut().find(|(c, _)| c.id == id) {
            kept.done = true;
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<Card> {
        let at = self.cards.iter().position(|(c, _)| c.id == id)?;
        Some(self.cards.remove(at).0)
    }

    pub fn save(&self) {
        let kept: Vec<&Kept> = self.cards.iter().map(|(_, kept)| kept).collect();
        if let Err(error) = store::save(&self.path, &kept) {
            warn!(%error, path = %self.path.display(), "couldn't save the cards");
        }
    }
}

/// The card `kept` holds, read the way its envelope was.
fn read(kept: &Kept) -> Option<Card> {
    let envelope = json!({ "v": 1, "cards": [kept.card] });
    PresentationEnvelope::from_value(&envelope, &kept.skill_id)?
        .cards
        .into_iter()
        .next()
}

/// What a card's widget needs from Ari.
pub struct Look<'a> {
    /// The file an `asset:` reference names.
    pub asset: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// The label of the Stop button standing in for the card's buttons while
    /// its alert rings, or None when it isn't ringing.
    pub ringing: Option<String>,
}

/// `card` drawn. `press` gets the id of the button pressed.
pub fn widget(card: &Card, look: &Look, press: impl Fn(&str) + 'static) -> gtk4::Widget {
    let column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(10)
        .css_classes(["card", "skill-card"])
        .build();
    match card.accent {
        Accent::Default => {}
        Accent::Warning => column.add_css_class("accent-warning"),
        Accent::Success => column.add_css_class("accent-success"),
        Accent::Critical => column.add_css_class("accent-critical"),
    }

    let header = gtk4::Box::builder().spacing(12).build();
    if let Some(image) = card.icon.as_deref().and_then(|icon| image(look, icon, 32)) {
        header.append(&image);
    }
    let titles = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    titles.append(&text(&card.title, &["heading"]));
    if let Some(subtitle) = &card.subtitle {
        titles.append(&text(subtitle, &["dim-label", "caption"]));
    }
    header.append(&titles);
    column.append(&header);

    if let Some(body) = &card.body {
        column.append(&text(body, &[]));
    }
    if let Some(deadline) = card.countdown_to_ts_ms {
        column.append(&countdown(deadline, card.started_at_ts_ms));
    } else if let Some(progress) = card.progress {
        column.append(
            &gtk4::ProgressBar::builder()
                .fraction(f64::from(progress).clamp(0.0, 1.0))
                .build(),
        );
    }
    if let Some(stat) = &card.stat {
        column.append(&stat_widget(stat, look));
    }
    if let Some(list) = &card.list {
        column.append(&list_widget(list, look));
    }

    let press = Rc::new(press);
    let buttons: Vec<CardAction> = match &look.ringing {
        Some(stop) => vec![CardAction {
            id: "stop_alert".into(),
            label: stop.clone(),
            utterance: None,
            speak: None,
            style: ButtonStyle::Primary,
        }],
        None => card.actions.clone(),
    };
    if !buttons.is_empty() {
        let row = gtk4::Box::builder()
            .spacing(8)
            .halign(gtk4::Align::End)
            .build();
        for action in buttons {
            let button = gtk4::Button::builder()
                .label(&action.label)
                .css_classes(["pill"])
                .build();
            match action.style {
                ButtonStyle::Default => {}
                ButtonStyle::Primary => button.add_css_class("suggested-action"),
                ButtonStyle::Destructive => button.add_css_class("destructive-action"),
            }
            let press = press.clone();
            button.connect_clicked(move |_| press(&action.id));
            row.append(&button);
        }
        column.append(&row);
    }
    column.upcast()
}

fn text(label: &str, classes: &[&str]) -> gtk4::Label {
    gtk4::Label::builder()
        .label(label)
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .xalign(0.0)
        .css_classes(classes.to_vec())
        .build()
}

fn image(look: &Look, reference: &str, size: i32) -> Option<gtk4::Image> {
    let path = (look.asset)(reference)?;
    let image = gtk4::Image::from_file(path);
    image.set_pixel_size(size);
    image.set_valign(gtk4::Align::Center);
    Some(image)
}

/// An icon and its words.
fn icon_text(item: &IconText, look: &Look, classes: &[&str]) -> gtk4::Box {
    let row = gtk4::Box::builder().spacing(6).build();
    if let Some(image) = item.icon.as_deref().and_then(|icon| image(look, icon, 16)) {
        row.append(&image);
    }
    row.append(&text(&item.text, classes));
    row
}

/// The time left until `deadline` (Unix ms), counting down as it goes, with
/// a bar of what's left when the card says when it started.
fn countdown(deadline: i64, started: Option<i64>) -> gtk4::Box {
    let column = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    let label = gtk4::Label::builder()
        .xalign(0.0)
        .css_classes(["title-1", "numeric"])
        .build();
    column.append(&label);
    let bar = started
        .filter(|&started| started < deadline)
        .map(|started| (gtk4::ProgressBar::new(), started));
    if let Some((bar, _)) = &bar {
        column.append(bar);
    }
    let show = move |label: &gtk4::Label, bar: &Option<(gtk4::ProgressBar, i64)>| {
        let now = now_ms();
        label.set_label(&left(deadline - now));
        if let Some((bar, started)) = bar {
            let total = (deadline - started) as f64;
            bar.set_fraction(((deadline - now) as f64 / total).clamp(0.0, 1.0));
        }
        now < deadline
    };
    if show(&label, &bar) {
        let weak = label.downgrade();
        glib::timeout_add_local(Duration::from_millis(250), move || match weak.upgrade() {
            Some(label) if show(&label, &bar) => glib::ControlFlow::Continue,
            _ => glib::ControlFlow::Break,
        });
    }
    column
}

/// `ms` left as minutes and seconds, or hours too, never below zero.
pub fn left(ms: i64) -> String {
    // Up to the second, so a fresh one-minute timer shows 1:00, not 0:59.
    let seconds = (ms.max(0) + 999) / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn now_ms() -> i64 {
    glib::real_time() / 1000
}

fn stat_widget(stat: &Stat, look: &Look) -> gtk4::Widget {
    let column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .build();
    column.append(&text(&stat.headline, &["title-1"]));
    if let Some(caption) = &stat.caption {
        column.append(&text(caption, &["dim-label"]));
    }
    if let Some(pill) = &stat.pill {
        let pill = icon_text(pill, look, &[]);
        pill.add_css_class("stat-pill");
        pill.set_halign(gtk4::Align::Start);
        column.append(&pill);
    }
    if !stat.metrics.is_empty() {
        let metrics = gtk4::FlowBox::builder()
            .selection_mode(gtk4::SelectionMode::None)
            .column_spacing(18)
            .row_spacing(6)
            .max_children_per_line(4)
            .build();
        for metric in &stat.metrics {
            metrics.insert(&icon_text(metric, look, &[]), -1);
        }
        column.append(&metrics);
    }
    if let Some(footer) = &stat.footer {
        column.append(&icon_text(footer, look, &["dim-label", "caption"]));
    }
    let background = stat
        .background
        .as_deref()
        .and_then(|reference| (look.asset)(reference));
    let Some(background) = background else {
        return column.upcast();
    };
    // The picture behind the figures, as on the phone.
    column.add_css_class("stat-over-picture");
    let picture = gtk4::Picture::builder()
        .content_fit(gtk4::ContentFit::Cover)
        .can_shrink(true)
        .css_classes(["stat-picture"])
        .build();
    picture.set_filename(Some(background.as_path()));
    let overlay = gtk4::Overlay::builder().child(&picture).build();
    overlay.add_overlay(&column);
    overlay.set_measure_overlay(&column, true);
    overlay.upcast()
}

fn list_widget(list: &ListCard, look: &Look) -> gtk4::Box {
    let column = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    if let Some(summary) = &list.summary {
        column.append(&icon_text(summary, look, &["heading"]));
    }
    let grid = gtk4::Grid::builder()
        .column_spacing(12)
        .row_spacing(6)
        .build();
    for (y, row) in list.rows.iter().enumerate() {
        let y = y as i32;
        grid.attach(&text(&row.leading, &["numeric"]), 0, y, 1, 1);
        let middle = gtk4::Box::builder().spacing(6).hexpand(true).build();
        if let Some(image) = row.icon.as_deref().and_then(|icon| image(look, icon, 20)) {
            middle.append(&image);
        }
        if let Some(words) = &row.text {
            middle.append(&text(words, &[]));
        }
        grid.attach(&middle, 1, y, 1, 1);
        if let Some(trailing) = &row.trailing {
            grid.attach(&text(trailing, &["dim-label"]), 2, y, 1, 1);
        }
        if let Some(badge) = &row.badge {
            let badge = icon_text(badge, look, &["caption"]);
            badge.add_css_class("stat-pill");
            grid.attach(&badge, 3, y, 1, 1);
        }
    }
    column.append(&grid);
    if let Some(footer) = &list.footer {
        column.append(&icon_text(footer, look, &["dim-label", "caption"]));
    }
    column
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(id: &str, title: &str) -> Value {
        json!({
            "id": id,
            "title": title,
            "countdown_to_ts_ms": 1_744_648_920_000_i64,
            "actions": [{"id": "cancel", "label": "Cancel", "utterance": "aricanceltimer t1"}],
            "on_complete": {"alert": {"id": "alert_t1", "title": "Pasta timer done"}},
        })
    }

    #[test]
    fn a_card_is_put_kept_and_read_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cards.json");
        let mut cards = Cards::open(path.clone());
        let put = cards.put(
            &json!({"v": 1, "cards": [timer("card_t1", "Pasta")]}),
            "dev.heyari.timer",
        );
        assert_eq!(put.len(), 1);
        cards.save();
        assert!(path.is_file());

        let reopened = Cards::open(path);
        let card = reopened.get("card_t1").unwrap();
        assert_eq!(card.title, "Pasta");
        assert_eq!(card.skill_id, "dev.heyari.timer");
        assert_eq!(card.countdown_to_ts_ms, Some(1_744_648_920_000));
        let alert = card.on_complete.as_ref().unwrap().alert.as_ref().unwrap();
        assert_eq!(alert.id, "alert_t1");
    }

    #[test]
    fn a_card_sent_again_replaces_the_one_with_its_id_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut cards = Cards::open(dir.path().join("cards.json"));
        let envelope = json!({"v": 1, "cards": [timer("a", "First"), timer("b", "Second")]});
        cards.put(&envelope, "dev.heyari.timer");
        cards.put(
            &json!({"v": 1, "cards": [timer("a", "First, moved")]}),
            "dev.heyari.timer",
        );
        let titles: Vec<&str> = cards.all().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["First, moved", "Second"]);
    }

    #[test]
    fn a_card_the_engine_would_not_read_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut cards = Cards::open(dir.path().join("cards.json"));
        let put = cards.put(
            &json!({"v": 1, "cards": [{"id": "no_title"}, timer("ok", "Fine")]}),
            "dev.heyari.timer",
        );
        assert_eq!(
            put.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["ok"]
        );
    }

    #[test]
    fn a_deadline_dealt_with_stays_dealt_with_until_the_card_is_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cards.json");
        let mut cards = Cards::open(path.clone());
        let envelope = json!({"v": 1, "cards": [
            timer("t1", "Pasta"),
            {"id": "plain", "title": "No countdown"},
            {"id": "ticking", "title": "Countdown only", "countdown_to_ts_ms": 5},
        ]});
        cards.put(&envelope, "dev.heyari.timer");
        let pending = |cards: &Cards| cards.pending().map(|c| c.id.clone()).collect::<Vec<_>>();
        assert_eq!(pending(&cards), ["t1"]);

        cards.done("t1");
        cards.save();
        let mut cards = Cards::open(path);
        assert!(pending(&cards).is_empty());
        assert!(cards.get("t1").is_some());

        cards.put(
            &json!({"v": 1, "cards": [timer("t1", "Pasta, longer")]}),
            "dev.heyari.timer",
        );
        assert_eq!(pending(&cards), ["t1"]);
    }

    #[test]
    fn removing_a_card_returns_it_and_it_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let mut cards = Cards::open(dir.path().join("cards.json"));
        cards.put(&json!({"v": 1, "cards": [timer("a", "Pasta")]}), "s");
        assert_eq!(cards.remove("a").map(|c| c.title), Some("Pasta".into()));
        assert!(cards.remove("a").is_none());
        assert_eq!(cards.all().count(), 0);
    }

    #[test]
    fn time_left_reads_as_minutes_and_seconds_rounding_up() {
        assert_eq!(left(60_000), "1:00");
        assert_eq!(left(59_001), "1:00");
        assert_eq!(left(59_000), "0:59");
        assert_eq!(left(600_000), "10:00");
        assert_eq!(left(3_600_000), "1:00:00");
        assert_eq!(left(3_725_000), "1:02:05");
        assert_eq!(left(0), "0:00");
        assert_eq!(left(-5_000), "0:00");
    }
}
