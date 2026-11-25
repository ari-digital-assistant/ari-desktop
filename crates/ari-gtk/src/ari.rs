//! Ari's state in this process, window or no window: the engine, the
//! conversation log, the remembered facts, and what skills keep on screen and
//! ringing. The window shows them while it's open.

mod presentation;

pub use presentation::dismiss_notification;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use ari_runtime::skills::SkillRegistryError;
use ari_voice::session::{ActionOutcome, CardPressed};
use gettextrs::{gettext, ngettext};
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use tokio::runtime::Handle;
use tracing::{info, warn};

use crate::alarms::Alarms;
use crate::alerts::Sounding;
use crate::cards::Cards;
use crate::conversation::{Attachment, Log};
use crate::engine::{Engine, Event, Setup, conversation_length};
use crate::window::{Page, Window};
use crate::{providers, store};
use presentation::Pressed;

pub struct Ari {
    pub engine: Engine,
    pub settings: gio::Settings,
    log: RefCell<Log>,
    facts: RefCell<Vec<String>>,
    facts_path: PathBuf,
    /// Typed turns whose answer is shown while it's written: turn → message.
    answers: RefCell<HashMap<u64, u64>>,
    /// Typed turns gone to the engine that haven't answered yet.
    thinking: RefCell<HashSet<u64>>,
    window: RefCell<Option<Rc<Window>>>,
    cards: RefCell<Cards>,
    alarms: RefCell<Alarms>,
    /// The alerts sounding, by id.
    sounding: RefCell<HashMap<String, Sounding>>,
    /// What shows under the answer to the turn being answered: the cards and
    /// suggestions of its action, or of the card button it pressed.
    held: RefCell<Vec<Attachment>>,
    /// Wakes Ari for the soonest card deadline or alarm.
    deadline: RefCell<Option<glib::SourceId>>,
    /// The volume a mute of the media player left behind.
    muted: Cell<Option<f64>>,
}

impl Ari {
    /// Start the engine as `settings` say. `tokio` runs the keyring's calls.
    pub fn start(settings: &gio::Settings, tokio: Handle) -> Result<Rc<Self>, SkillRegistryError> {
        let dir = store::dir();
        let facts_path = dir.join("facts.json");
        let facts: Vec<String> = store::load(&facts_path);
        let (engine, mut events) = Engine::start(Setup {
            dir: &dir,
            facts: facts.clone(),
            language: providers::language(&settings.string("language")),
            conversation_memory: settings.boolean("conversation-memory"),
            conversation_length: conversation_length(&settings.string("conversation-length")),
            tokio,
        })?;
        engine
            .assistants
            .set_active_assistant(assistant(settings, "assistant"));
        let ari = Rc::new(Self {
            engine,
            settings: settings.clone(),
            log: RefCell::new(Log::open(dir.join("conversation.json"))),
            facts: RefCell::new(facts),
            facts_path,
            answers: RefCell::default(),
            thinking: RefCell::default(),
            window: RefCell::default(),
            cards: RefCell::new(Cards::open(dir.join("cards.json"))),
            alarms: RefCell::new(Alarms::open(dir.join("alarms.json"))),
            sounding: RefCell::default(),
            held: RefCell::default(),
            deadline: RefCell::default(),
            muted: Cell::default(),
        });
        let weak = Rc::downgrade(&ari);
        glib::spawn_future_local(async move {
            while let Some(event) = events.recv().await {
                let Some(ari) = weak.upgrade() else { break };
                ari.handle(event);
            }
        });
        // The installed skills, once their API keys are in: an assistant
        // loaded before its key would answer that it has none.
        glib::spawn_future_local(glib::clone!(
            #[weak]
            ari,
            async move {
                ari.engine.settings.load_secrets().await;
                if ari.engine.reload_skills().await.is_err() {
                    warn!("loading the installed skills panicked");
                }
            }
        ));
        ari.follow(settings);
        // What came while Ari wasn't running is reported missed.
        ari.due();
        ari.watch_updates();
        Ok(ari)
    }

    /// Look for skill updates a minute after starting, then daily. Fetching
    /// the registry's list also keeps the store suggestions current.
    fn watch_updates(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let check = move || {
            if let Some(ari) = weak.upgrade() {
                ari.check_updates();
            }
        };
        glib::timeout_add_seconds_local_once(60, check.clone());
        glib::timeout_add_seconds_local(24 * 60 * 60, move || {
            check();
            glib::ControlFlow::Continue
        });
    }

    fn check_updates(&self) {
        let skills = self.engine.skills.clone();
        glib::spawn_future_local(async move {
            let updates = match gio::spawn_blocking(move || skills.check_for_updates()).await {
                Ok(Ok(updates)) => updates,
                Ok(Err(error)) => {
                    info!(%error, "couldn't check for skill updates");
                    return;
                }
                Err(_) => {
                    warn!("checking for skill updates panicked");
                    return;
                }
            };
            let Some(app) = gio::Application::default().filter(|_| !updates.is_empty()) else {
                return;
            };
            let count = updates.len();
            let notification = gio::Notification::new(&gettext("Skill Updates"));
            notification.set_body(Some(
                &ngettext(
                    "{} skill can be updated.",
                    "{} skills can be updated.",
                    count as u32,
                )
                .replace("{}", &count.to_string()),
            ));
            notification.set_default_action("app.show-skills");
            app.send_notification(Some("skill-updates"), &notification);
        });
    }

    /// Hand the engine each setting as Preferences changes it.
    fn follow(self: &Rc<Self>, settings: &gio::Settings) {
        settings.connect_changed(
            None,
            glib::clone!(
                #[weak(rename_to = ari)]
                self,
                move |settings, key| {
                    let engine = &ari.engine;
                    match key {
                        "language" => {
                            engine.set_language(providers::language(&settings.string(key)))
                        }
                        "conversation-memory" => {
                            engine.set_conversation_memory(settings.boolean(key))
                        }
                        "conversation-length" => engine
                            .set_conversation_length(conversation_length(&settings.string(key))),
                        "assistant" => engine.set_assistant(assistant(settings, key)),
                        _ => {}
                    }
                }
            ),
        );
    }

    /// Show the window, building it the first time.
    pub fn present(self: &Rc<Self>, app: &adw::Application) -> Rc<Window> {
        let window = self.window.borrow().clone();
        let window = window.unwrap_or_else(|| {
            let window = Window::new(app, self);
            window.chat.show(self.log.borrow().messages());
            window.chat.set_thinking(!self.thinking.borrow().is_empty());
            window.show_facts(&self.facts.borrow());
            *self.window.borrow_mut() = Some(window.clone());
            self.show_timers();
            window
        });
        window.present();
        window
    }

    /// Show skill `id` from the store, as a suggestion's button does.
    pub fn show_skill(self: &Rc<Self>, id: &str) {
        if let Some(app) = app() {
            self.present(&app).show_skill(id);
        }
    }

    /// Show the window on page `page`.
    pub fn present_page(self: &Rc<Self>, page: Page) {
        if let Some(app) = app() {
            self.present(&app).select(page);
        }
    }

    fn present_timers(self: &Rc<Self>) {
        self.present_page(Page::Timers);
    }

    /// Show what's counting down and the alarms on the Timers page.
    fn show_timers(&self) {
        if let Some(window) = self.window() {
            window
                .timers
                .show(&self.cards.borrow(), self.alarms.borrow().all());
        }
    }

    /// Say `utterance` to the engine, as a notification's button does: its
    /// answer shows in the conversation.
    pub fn send(self: &Rc<Self>, utterance: String) {
        let ari = self.clone();
        glib::spawn_future_local(async move {
            let (text, attachments) = ari.respond(utterance).await;
            if !text.trim().is_empty() || !attachments.is_empty() {
                let id = ari.add(false, text, None);
                ari.attach(id, attachments);
                ari.log.borrow().save();
            }
        });
    }

    fn window(&self) -> Option<Rc<Window>> {
        self.window.borrow().clone()
    }

    pub fn submit(&self, text: String) {
        let message = self.log.borrow_mut().add(true, text.clone(), None).clone();
        self.log.borrow().save();
        if let Some(window) = self.window() {
            window.chat.add(&message);
        }
        self.engine.submit(text);
    }

    pub fn new_conversation(&self) {
        self.engine.new_conversation();
        self.answers.borrow_mut().clear();
        self.thinking.borrow_mut().clear();
        self.clear_log();
        self.log.borrow().save();
        self.show_thinking();
    }

    pub fn delete_fact(&self, fact: &str) {
        let mut facts = self.facts.borrow().clone();
        facts.retain(|f| f != fact);
        self.engine.set_facts(facts.clone());
        self.keep_facts(facts);
    }

    fn handle(self: Rc<Self>, event: Event) {
        match event {
            Event::Thinking(turn) => {
                self.thinking.borrow_mut().insert(turn);
                self.show_thinking();
            }
            Event::Answer {
                turn,
                text,
                markdown,
            } => {
                self.thinking.borrow_mut().remove(&turn);
                let shown = self.answers.borrow().get(&turn).copied();
                match shown {
                    Some(id) => self.replace(id, text, markdown),
                    None => {
                        let id = self.add(false, text, markdown);
                        self.answers.borrow_mut().insert(turn, id);
                    }
                }
                self.show_thinking();
            }
            Event::Finished { turn, finish } => {
                self.thinking.borrow_mut().remove(&turn);
                let shown = self.answers.borrow_mut().remove(&turn);
                let held = self.held.take();
                // "Clear history": the engine has forgotten the conversation,
                // so the log goes too, request included, before the reply.
                if finish.clear {
                    self.clear_log();
                }
                if let Some(reply) = finish.reply {
                    let id = match shown {
                        Some(id) if !finish.clear => {
                            self.replace(id, reply.text, reply.markdown);
                            id
                        }
                        _ => self.add(false, reply.text, reply.markdown),
                    };
                    self.attach(id, held);
                }
                self.log.borrow().save();
                self.show_thinking();
            }
            Event::Logged(turn) => {
                let held = self.held.take();
                if turn.clear {
                    self.clear_log();
                } else {
                    self.add(true, turn.heard, None);
                }
                if let Some(reply) = turn.reply {
                    let id = self.add(false, reply.text, reply.markdown);
                    self.attach(id, held);
                }
                self.log.borrow().save();
            }
            Event::FactsChanged => {
                glib::spawn_future_local(async move {
                    let facts = self.engine.facts().await;
                    self.keep_facts(facts);
                });
            }
            Event::SpeechDone(id) => self.engine.speech_finished(id),
            Event::Action {
                json,
                skill_id,
                done,
            } => {
                glib::spawn_future_local(async move {
                    let ran = self.run_envelope(&json, &skill_id).await;
                    let (cards, attachments) = self.hold(ran.attachments);
                    // Only fails once the session has gone, as Ari quits.
                    let _ = done.send(ActionOutcome {
                        text: ran.text,
                        display_text: ran.display,
                        cards,
                        attachments,
                    });
                });
            }
            Event::Targets(done) => {
                let _ = done.send(self.targets());
            }
            Event::Press { card, button, done } => {
                glib::spawn_future_local(async move {
                    let pressed = match self.press(&card, &button).await {
                        None => CardPressed::Gone,
                        Some(Pressed::Silent) => CardPressed::Silent,
                        Some(Pressed::Spoken { text, attachments }) => {
                            let (cards, attachments) = self.hold(attachments);
                            CardPressed::Spoken {
                                text,
                                cards,
                                attachments,
                            }
                        }
                    };
                    let _ = done.send(pressed);
                });
            }
            Event::StopAlert(id) => {
                self.stop_alert(&id);
            }
        }
    }

    /// Keep `attachments` for the answer to the turn. Whether they have a
    /// card, and any at all, as the session asks.
    fn hold(&self, attachments: Vec<Attachment>) -> (bool, bool) {
        let cards = attachments
            .iter()
            .any(|a| matches!(a, Attachment::Card { .. }));
        let any = !attachments.is_empty();
        *self.held.borrow_mut() = attachments;
        (cards, any)
    }

    fn add(&self, from_user: bool, text: String, markdown: Option<String>) -> u64 {
        let message = self.log.borrow_mut().add(from_user, text, markdown).clone();
        if let Some(window) = self.window() {
            window.chat.add(&message);
        }
        message.id
    }

    /// Show `attachments` under message `id`.
    fn attach(&self, id: u64, attachments: Vec<Attachment>) {
        if attachments.is_empty() {
            return;
        }
        let attached = self.log.borrow_mut().attach(id, attachments).cloned();
        if let Some(message) = attached
            && let Some(window) = self.window()
        {
            window.chat.replace(&message);
        }
    }

    fn replace(&self, id: u64, text: String, markdown: Option<String>) {
        let replaced = self.log.borrow_mut().replace(id, text, markdown).cloned();
        if let Some(message) = replaced
            && let Some(window) = self.window()
        {
            window.chat.replace(&message);
        }
    }

    fn clear_log(&self) {
        self.log.borrow_mut().clear();
        if let Some(window) = self.window() {
            window.chat.show(&[]);
        }
    }

    fn show_thinking(&self) {
        if let Some(window) = self.window() {
            window.chat.set_thinking(!self.thinking.borrow().is_empty());
        }
    }

    fn keep_facts(&self, facts: Vec<String>) {
        if let Err(error) = store::save(&self.facts_path, &facts) {
            warn!(%error, path = %self.facts_path.display(), "couldn't save the remembered facts");
        }
        if let Some(window) = self.window() {
            window.show_facts(&facts);
        }
        *self.facts.borrow_mut() = facts;
    }
}

/// The running app, for opening the window from outside it.
fn app() -> Option<adw::Application> {
    gio::Application::default()?.downcast().ok()
}

fn assistant(settings: &gio::Settings, key: &str) -> Option<String> {
    Some(settings.string(key).to_string()).filter(|id| !id.is_empty())
}
