//! What skills send, carried out: their actions, cards, alerts and
//! notifications, Ari's own alarms, and what a card's buttons do.

use std::rc::Rc;
use std::time::Duration;

use ari_core::envelope::{
    AlarmAction, AlertAction, AlertSpec, ButtonStyle, CardAction, CardPress, Importance,
    MediaAction, NotificationSpec, PresentationEnvelope, Urgency,
};
use ari_runtime::Reply;
use ari_voice::session::{AlertTarget, CardButton, CardTarget, InterceptTargets};
use gettextrs::gettext;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use serde_json::Value;
use tracing::warn;

use super::Ari;
use crate::alarms::{self, Alarm};
use crate::alerts::{self, Sounding};
use crate::cards::{self, Look, now_ms};
use crate::conversation::Attachment;
use crate::links::{MAPS, SEARCH_ENGINES, link};
use crate::mpris::{self, Command, Outcome};

/// How late a deadline can be noticed and still ring. Later than this, the
/// computer was asleep or off when it came, so it's reported missed.
const LATE_MS: i64 = 60_000;

/// How long an alarm rings for unless it's stopped.
const ALARM_RINGS_MS: i64 = 5 * 60_000;

/// What running an envelope came to.
pub(super) struct Ran {
    /// What Ari says.
    pub text: String,
    /// What the conversation shows instead, when the skill words it apart.
    pub display: Option<String>,
    pub attachments: Vec<Attachment>,
    /// Its `run_utterance`, for a card's cancel to send on to the engine.
    pub then: Option<String>,
}

impl Ran {
    fn said(text: String) -> Self {
        Self {
            text,
            display: None,
            attachments: Vec::new(),
            then: None,
        }
    }
}

/// What pressing a card's button came to.
pub(super) enum Pressed {
    Silent,
    Spoken {
        text: String,
        attachments: Vec<Attachment>,
    },
}

impl Pressed {
    fn saying(speak: Option<String>) -> Self {
        match speak.filter(|s| !s.trim().is_empty()) {
            Some(text) => Self::Spoken {
                text,
                attachments: Vec::new(),
            },
            None => Self::Silent,
        }
    }
}

impl Ari {
    /// Carry out skill `skill_id`'s envelope `json`, which comes as Rust's
    /// `ari_core::envelope` reads it. The skill's `speak` wins over what Ari
    /// would say about an action, but never stops the action happening.
    pub(super) async fn run_envelope(self: &Rc<Self>, json: &str, skill_id: &str) -> Ran {
        let value: Value = serde_json::from_str(json).unwrap_or(Value::Null);
        let Some(envelope) = PresentationEnvelope::from_value(&value, skill_id) else {
            warn!(skill_id, json, "an envelope Ari can't read");
            return Ran::said(gettext("I couldn't understand that action."));
        };
        let mut said = None;
        if envelope.launch_app.is_some() {
            said = Some(gettext("I can't open apps on this computer."));
        }
        if let Some(query) = &envelope.search {
            said = Some(if query.trim().is_empty() {
                gettext("What would you like me to search for?")
            } else {
                let engine = self.settings.string("search-engine");
                self.open(
                    link(&SEARCH_ENGINES, &engine, query),
                    gettext("I couldn't open the search."),
                );
                gettext("Searching for {}.").replace("{}", query)
            });
        }
        if let Some(url) = &envelope.open_url {
            self.open(url.clone(), gettext("I couldn't open that link."));
            said = Some(gettext("Opening that link."));
        }
        if let Some(media) = &envelope.media {
            said = Some(self.media(media).await);
        }
        if let Some(navigate) = &envelope.navigate {
            let maps = self.settings.string("maps");
            self.open(
                link(&MAPS, &maps, &navigate.destination),
                gettext("I couldn't open the map."),
            );
        }
        if envelope.message.is_some() || envelope.reply.is_some() {
            warn!(skill_id, "Ari for Linux can't send messages");
        }
        if let Some(text) = &envelope.clipboard_text
            && let Some(display) = gdk::Display::default()
        {
            display.clipboard().set_text(text);
        }
        if let Some(alarm) = &envelope.alarm
            && let Err(failed) = self.alarm(alarm)
        {
            return Ran::said(failed);
        }
        let attachments = if envelope.has_presentation() {
            self.apply(&envelope, &value, skill_id)
        } else {
            Vec::new()
        };
        Ran {
            text: envelope.speak.or(said).unwrap_or_default(),
            display: envelope.display,
            attachments,
            then: envelope.run_utterance,
        }
    }

    /// Open `url` in the user's browser, saying `failed` in the conversation
    /// if it can't be.
    fn open(self: &Rc<Self>, url: String, failed: String) {
        let parent = self.window().map(|window| window.gtk().clone());
        let launcher = gtk4::UriLauncher::new(&url);
        launcher.launch(
            parent.as_ref(),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = ari)]
                self,
                move |result| {
                    let Err(error) = result else { return };
                    // The user closed the portal's "Open with" dialog.
                    if error.matches(gtk4::DialogError::Dismissed) {
                        return;
                    }
                    warn!(%error, url, "couldn't open a link");
                    ari.add(false, failed, None);
                    ari.log.borrow().save();
                }
            ),
        );
    }

    async fn media(&self, media: &MediaAction) -> String {
        if media.action == "play" {
            return match media.query {
                None => gettext("What would you like me to play?"),
                Some(_) => gettext("I can't play music by name on this computer."),
            };
        }
        let Some(command) = mpris::command(media) else {
            warn!(action = media.action, "a media action Ari can't carry out");
            return String::new();
        };
        match mpris::control(command, &self.muted).await {
            Outcome::Done(Command::Method("Pause")) => gettext("Paused."),
            Outcome::Done(Command::Method("Play")) => gettext("Resumed."),
            Outcome::Done(Command::Method("Next")) => gettext("Skipped."),
            Outcome::Done(Command::Method("Previous")) => gettext("Going back."),
            Outcome::Done(Command::Method("Stop")) => gettext("Stopped."),
            Outcome::Done(Command::Mute) => gettext("Muted."),
            Outcome::Done(Command::Unmute) => gettext("Unmuted."),
            Outcome::Done(_) => String::new(),
            Outcome::VolumeAt(level) => {
                gettext("Volume set to {}%.").replace("{}", &level.to_string())
            }
            Outcome::NothingPlaying => gettext("Nothing's playing right now."),
            Outcome::Failed(error) => {
                warn!(error, "the media player didn't answer");
                gettext("The media player didn't answer.")
            }
        }
    }

    /// Set an alarm, or show them. What to say instead when it can't be set.
    fn alarm(self: &Rc<Self>, alarm: &AlarmAction) -> Result<(), String> {
        match alarm.op.as_str() {
            "set" => {
                let now = glib::DateTime::now_local()
                    .map_err(|_| gettext("I couldn't set that alarm."))?;
                let (Some(hour), Some(minute)) = (alarm.hour, alarm.minute) else {
                    return Err(gettext("I couldn't set that alarm."));
                };
                let set = self
                    .alarms
                    .borrow_mut()
                    .add(
                        &now,
                        hour as i32,
                        minute as i32,
                        alarm.message.clone(),
                        alarm.days.clone(),
                    )
                    .is_some();
                if !set {
                    return Err(gettext("I couldn't set that alarm."));
                }
                self.alarms.borrow().save();
                self.schedule();
                self.show_timers();
            }
            "show" => self.present_timers(),
            op => warn!(op, "an alarm action Ari doesn't know"),
        }
        Ok(())
    }

    /// Apply the envelope's dismissals, cards, notifications and alerts.
    /// What shows under the answer: the first card, and a skill suggested.
    fn apply(
        self: &Rc<Self>,
        envelope: &PresentationEnvelope,
        value: &Value,
        skill_id: &str,
    ) -> Vec<Attachment> {
        for id in &envelope.dismiss_cards {
            self.remove_card(id);
        }
        for id in &envelope.dismiss_notifications {
            withdraw(&skill_notification(id));
        }
        for id in &envelope.dismiss_alerts {
            self.stop_alert(id);
        }
        let put = self.cards.borrow_mut().put(value, skill_id);
        self.cards.borrow().save();
        for card in &put {
            self.refresh_card(&card.id);
        }
        for notification in &envelope.notifications {
            notify(notification);
        }
        for alert in &envelope.alerts {
            self.sound(alert.clone());
        }
        self.schedule();
        let card = put.first().map(|card| Attachment::Card {
            id: card.id.clone(),
        });
        let suggestion = envelope
            .suggest_skill
            .as_ref()
            .map(|s| Attachment::Suggestion {
                id: s.id.clone(),
                name: s.name.clone(),
            });
        card.into_iter().chain(suggestion).collect()
    }

    /// Card `id` drawn as it is now, or None once it has gone.
    pub fn card_widget(self: &Rc<Self>, id: &str) -> Option<gtk4::Widget> {
        let card = self.cards.borrow().get(id).cloned()?;
        let ringing = card
            .on_complete
            .as_ref()
            .and_then(|complete| complete.alert.as_ref())
            .filter(|alert| self.sounding.borrow().contains_key(&alert.id))
            .map(stop_label);
        let asset = |reference: &str| self.engine.asset(&card.skill_id, reference);
        let look = Look {
            asset: &asset,
            ringing,
        };
        let card_id = card.id.clone();
        let weak = Rc::downgrade(self);
        Some(cards::widget(&card, &look, move |button| {
            if let Some(ari) = weak.upgrade() {
                ari.tap(&card_id, button);
            }
        }))
    }

    /// Button `button` on card `card` was clicked: what it answers shows in
    /// the conversation.
    pub fn tap(self: &Rc<Self>, card: &str, button: &str) {
        let (ari, card, button) = (self.clone(), card.to_string(), button.to_string());
        glib::spawn_future_local(async move {
            let Some(Pressed::Spoken { text, attachments }) = ari.press(&card, &button).await
            else {
                return;
            };
            if !text.trim().is_empty() || !attachments.is_empty() {
                let id = ari.add(false, text, None);
                ari.attach(id, attachments);
                ari.log.borrow().save();
            }
        });
    }

    /// Press button `button_id` on card `card_id`, by click or by the words
    /// of a turn. What the press means is the engine's call
    /// (`Card::press`). None when the card or button has gone.
    pub(super) async fn press(self: &Rc<Self>, card_id: &str, button_id: &str) -> Option<Pressed> {
        let card = self.cards.borrow().get(card_id).cloned()?;
        let action = match card.actions.iter().find(|a| a.id == button_id) {
            Some(action) => action.clone(),
            // The Stop standing in for the buttons while the card's alert rings.
            None if button_id == "stop_alert" => CardAction {
                id: button_id.to_string(),
                label: String::new(),
                utterance: None,
                speak: None,
                style: ButtonStyle::Primary,
            },
            None => return None,
        };
        Some(match card.press(&action) {
            CardPress::StopAlert { alert_id } => {
                self.stop_alert(&alert_id);
                Pressed::Silent
            }
            CardPress::NoAlert => Pressed::Silent,
            CardPress::Cancel { on_cancel, speak } => {
                // The cancel envelope often sends a word back to its skill,
                // whose answer ("OK, cancelled that.") says it worked.
                let ran = self.run_envelope(&on_cancel, &card.skill_id).await;
                let answer = match ran.then.filter(|u| !u.trim().is_empty()) {
                    Some(utterance) => Some(self.respond(utterance).await.0),
                    None => None,
                };
                self.remove_card(card_id);
                Pressed::saying(answer.filter(|a| !a.trim().is_empty()).or(speak))
            }
            CardPress::Run { utterance } => {
                self.remove_card(card_id);
                let (text, attachments) = self.respond(utterance).await;
                Pressed::Spoken { text, attachments }
            }
            CardPress::Dismiss { speak } => {
                self.remove_card(card_id);
                Pressed::saying(speak)
            }
        })
    }

    /// The engine's answer to `utterance`, outside the conversation's turns,
    /// with what shows under it.
    pub(super) async fn respond(self: &Rc<Self>, utterance: String) -> (String, Vec<Attachment>) {
        match self.engine.respond(utterance).await {
            Some(Reply::Text { body, .. } | Reply::NotUnderstood { body }) => (body, Vec::new()),
            Some(Reply::Action { json, skill_id, .. }) => {
                let ran = self.run_envelope(&json, &skill_id).await;
                (ran.text, ran.attachments)
            }
            Some(Reply::Binary { mime, data }) => (
                format!("[Binary: {mime}, {} bytes]", data.len()),
                Vec::new(),
            ),
            None => {
                warn!("the engine's answer to a button panicked");
                (String::new(), Vec::new())
            }
        }
    }

    /// The sounding alerts and the cards, for the words of a turn to answer.
    pub(super) fn targets(&self) -> InterceptTargets {
        InterceptTargets {
            alerts: self
                .sounding
                .borrow()
                .values()
                .map(|sounding| AlertTarget {
                    id: sounding.spec.id.clone(),
                    stop_label: stop_action(&sounding.spec).map(|a| a.label.clone()),
                })
                .collect(),
            cards: self
                .cards
                .borrow()
                .all()
                .map(|card| CardTarget {
                    id: card.id.clone(),
                    buttons: card
                        .actions
                        .iter()
                        .map(|a| CardButton {
                            id: a.id.clone(),
                            label: a.label.clone(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn remove_card(self: &Rc<Self>, id: &str) {
        if self.cards.borrow_mut().remove(id).is_some() {
            self.cards.borrow().save();
            self.refresh_card(id);
            self.schedule();
        }
    }

    /// Draw card `id` again wherever it shows.
    fn refresh_card(&self, id: &str) {
        if let Some(window) = self.window() {
            window.chat.refresh_card(id);
        }
        self.show_timers();
    }

    /// Start sounding `spec`, in place of any alert with its id.
    fn sound(self: &Rc<Self>, spec: AlertSpec) {
        self.stop_alert(&spec.id);
        let sound = alerts::sound_file(&spec.sound, |reference| {
            self.engine.asset(&spec.skill_id, reference)
        });
        if let Some(app) = gio::Application::default() {
            let notification = gio::Notification::new(&spec.title);
            if let Some(body) = &spec.body {
                notification.set_body(Some(body));
            }
            notification.set_priority(gio::NotificationPriority::Urgent);
            let target = spec.id.to_variant();
            if stop_action(&spec).is_none() {
                notification.add_button_with_target_value(
                    &gettext("Stop"),
                    "app.stop-alert",
                    Some(&target),
                );
            }
            for action in &spec.actions {
                match (&action.utterance, action.id.as_str()) {
                    (_, "stop_alert") => notification.add_button_with_target_value(
                        &action.label,
                        "app.stop-alert",
                        Some(&target),
                    ),
                    (Some(utterance), _) => notification.add_button_with_target_value(
                        &action.label,
                        "app.send",
                        Some(&utterance.to_variant()),
                    ),
                    (None, _) => {}
                }
            }
            app.send_notification(Some(&alert_notification(&spec.id)), &notification);
        }
        let id = spec.id.clone();
        let weak = Rc::downgrade(self);
        let sounding = Sounding::start(spec, sound.as_deref(), move || {
            if let Some(ari) = weak.upgrade() {
                ari.stop_alert(&id);
            }
        });
        let id = sounding.spec.id.clone();
        self.sounding.borrow_mut().insert(id.clone(), sounding);
        for card in self.cards_waiting_on(&id) {
            self.refresh_card(&card.0);
        }
    }

    /// Stop alert `id`, and remove the cards that waited on it if their
    /// skill asked. False when it wasn't sounding.
    pub fn stop_alert(self: &Rc<Self>, id: &str) -> bool {
        let Some(sounding) = self.sounding.borrow_mut().remove(id) else {
            return false;
        };
        sounding.stop();
        withdraw(&alert_notification(id));
        for (card, dismiss) in self.cards_waiting_on(id) {
            if dismiss {
                self.remove_card(&card);
            } else {
                self.refresh_card(&card);
            }
        }
        true
    }

    /// Stop every sounding alert. False when none was.
    pub fn stop_alerts(self: &Rc<Self>) -> bool {
        let ids: Vec<String> = self.sounding.borrow().keys().cloned().collect();
        for id in &ids {
            self.stop_alert(id);
        }
        !ids.is_empty()
    }

    /// The cards whose deadline sounds alert `id`, each with whether it goes
    /// once the alert ends.
    fn cards_waiting_on(&self, id: &str) -> Vec<(String, bool)> {
        self.cards
            .borrow()
            .all()
            .filter_map(|card| {
                let complete = card.on_complete.as_ref()?;
                (complete.alert.as_ref()?.id == id)
                    .then(|| (card.id.clone(), complete.dismiss_card))
            })
            .collect()
    }

    /// Wake for the soonest card deadline or alarm. A wait is never longer
    /// than half a minute, as the main loop's clock stops while the computer
    /// sleeps and the deadlines are on the wall clock.
    pub(super) fn schedule(self: &Rc<Self>) {
        if let Some(id) = self.deadline.take() {
            id.remove();
        }
        let cards: Vec<i64> = self
            .cards
            .borrow()
            .pending()
            .filter_map(|c| c.countdown_to_ts_ms)
            .collect();
        let alarms = self
            .alarms
            .borrow()
            .all()
            .iter()
            .map(|a| a.next_ms)
            .collect::<Vec<_>>();
        let Some(soonest) = cards.into_iter().chain(alarms).min() else {
            return;
        };
        let wait = (soonest - now_ms()).clamp(0, 30_000) as u64;
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(wait), move || {
            if let Some(ari) = weak.upgrade() {
                // It has run, so there's nothing left to remove.
                ari.deadline.take();
                ari.due();
            }
        });
        *self.deadline.borrow_mut() = Some(id);
    }

    /// Deal with the card deadlines and alarms that have come: ring them, or
    /// report them missed if they came while Ari couldn't ring.
    pub(super) fn due(self: &Rc<Self>) {
        let now = now_ms();
        let due: Vec<_> = self
            .cards
            .borrow()
            .pending()
            .filter(|card| card.countdown_to_ts_ms.is_some_and(|at| at <= now))
            .cloned()
            .collect();
        for card in due {
            self.cards.borrow_mut().done(&card.id);
            let (Some(complete), Some(at)) = (&card.on_complete, card.countdown_to_ts_ms) else {
                continue;
            };
            for id in &complete.dismiss_notifications {
                withdraw(&skill_notification(id));
            }
            let late = now - at > LATE_MS;
            match &complete.alert {
                Some(alert) if late => missed(&alert.title),
                Some(alert) => self.sound(alert.clone()),
                None => {}
            }
            // A card whose alert rings stays, for its Stop, until it ends.
            if complete.dismiss_card && (complete.alert.is_none() || late) {
                self.remove_card(&card.id);
            } else {
                self.refresh_card(&card.id);
            }
        }
        self.cards.borrow().save();

        if let Ok(local) = glib::DateTime::now_local() {
            let rung = self.alarms.borrow_mut().due(&local);
            if !rung.is_empty() {
                self.alarms.borrow().save();
                for alarm in rung {
                    if now - alarm.next_ms > LATE_MS {
                        missed(&alarm_title(&alarm));
                    } else {
                        self.sound(alarm_alert(&alarm));
                    }
                }
                self.show_timers();
            }
        }
        self.schedule();
    }

    pub fn delete_alarm(self: &Rc<Self>, id: u64) {
        if self.alarms.borrow_mut().remove(id).is_some() {
            self.alarms.borrow().save();
            self.schedule();
            self.show_timers();
        }
    }
}

fn stop_action(spec: &AlertSpec) -> Option<&AlertAction> {
    spec.actions.iter().find(|a| a.id == "stop_alert")
}

/// The label of the Stop button for `alert`: its own, in its skill's words.
fn stop_label(alert: &AlertSpec) -> String {
    stop_action(alert).map_or_else(|| gettext("Stop"), |a| a.label.clone())
}

fn alarm_title(alarm: &Alarm) -> String {
    alarm.message.clone().unwrap_or_else(|| gettext("Alarm"))
}

fn alarm_alert(alarm: &Alarm) -> AlertSpec {
    AlertSpec {
        id: format!("alarm-{}", alarm.id),
        skill_id: String::new(),
        title: alarm_title(alarm),
        body: Some(alarms::clock(alarm.hour, alarm.minute)),
        urgency: Urgency::Critical,
        sound: "system.alarm".into(),
        speech_loop: None,
        auto_stop_ms: ALARM_RINGS_MS,
        // The ringing time is the limit, not how often the sound plays.
        max_cycles: i64::MAX,
        full_takeover: false,
        icon: None,
        actions: vec![AlertAction {
            id: "stop_alert".into(),
            label: gettext("Stop"),
            utterance: None,
            style: ButtonStyle::Primary,
        }],
    }
}

fn alert_notification(id: &str) -> String {
    format!("alert-{id}")
}

fn skill_notification(id: &str) -> String {
    format!("skill-{id}")
}

fn withdraw(id: &str) {
    if let Some(app) = gio::Application::default() {
        app.withdraw_notification(id);
    }
}

/// Post a skill's notification. One that stays on, such as a running
/// timer's, goes quietly into the list rather than popping up.
fn notify(spec: &NotificationSpec) {
    let Some(app) = gio::Application::default() else {
        return;
    };
    let notification = gio::Notification::new(&spec.title);
    if let Some(body) = &spec.body {
        notification.set_body(Some(body));
    }
    notification.set_priority(match spec.importance {
        _ if spec.sticky => gio::NotificationPriority::Low,
        Importance::Min | Importance::Low => gio::NotificationPriority::Low,
        Importance::Default => gio::NotificationPriority::Normal,
        Importance::High => gio::NotificationPriority::High,
    });
    for action in &spec.actions {
        match (&action.utterance, action.id.as_str()) {
            (_, "dismiss_notification") => notification.add_button_with_target_value(
                &action.label,
                "app.dismiss-notification",
                Some(&spec.id.to_variant()),
            ),
            (Some(utterance), _) => notification.add_button_with_target_value(
                &action.label,
                "app.send",
                Some(&utterance.to_variant()),
            ),
            (None, _) => {}
        }
    }
    app.send_notification(Some(&skill_notification(&spec.id)), &notification);
}

/// Skill notification `id`, which its Dismiss button clears.
pub fn dismiss_notification(id: &str) {
    withdraw(&skill_notification(id));
}

/// Say that `title` came and went while Ari couldn't ring it.
fn missed(title: &str) {
    let Some(app) = gio::Application::default() else {
        return;
    };
    let notification = gio::Notification::new(title);
    notification.set_body(Some(&gettext(
        "Missed while your computer was off or asleep.",
    )));
    app.send_notification(None, &notification);
}
