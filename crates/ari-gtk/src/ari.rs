//! Ari's state in this process, window or no window: the engine, the
//! conversation log and the remembered facts. The window shows them while
//! it's open.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::glib;
use libadwaita as adw;
use tracing::warn;

use crate::conversation::Log;
use crate::engine::{Engine, Event};
use crate::store;
use crate::window::Window;

pub struct Ari {
    engine: Engine,
    log: RefCell<Log>,
    facts: RefCell<Vec<String>>,
    facts_path: PathBuf,
    /// Typed turns whose answer is shown while it's written: turn → message.
    answers: RefCell<HashMap<u64, u64>>,
    /// Typed turns gone to the engine that haven't answered yet.
    thinking: RefCell<HashSet<u64>>,
    window: RefCell<Option<Rc<Window>>>,
}

impl Ari {
    pub fn start() -> Rc<Self> {
        let dir = store::dir();
        let facts_path = dir.join("facts.json");
        let facts: Vec<String> = store::load(&facts_path);
        let (engine, mut events) = Engine::start(&dir, facts.clone());
        let ari = Rc::new(Self {
            engine,
            log: RefCell::new(Log::open(dir.join("conversation.json"))),
            facts: RefCell::new(facts),
            facts_path,
            answers: RefCell::default(),
            thinking: RefCell::default(),
            window: RefCell::default(),
        });
        let weak = Rc::downgrade(&ari);
        glib::spawn_future_local(async move {
            while let Some(event) = events.recv().await {
                let Some(ari) = weak.upgrade() else { break };
                ari.handle(event);
            }
        });
        ari
    }

    /// Show the window, building it the first time.
    pub fn present(self: &Rc<Self>, app: &adw::Application) {
        let window = self.window.borrow().clone();
        let window = window.unwrap_or_else(|| {
            let window = Window::new(app, self);
            window.chat.show(self.log.borrow().messages());
            window.chat.set_thinking(!self.thinking.borrow().is_empty());
            window.show_facts(&self.facts.borrow());
            *self.window.borrow_mut() = Some(window.clone());
            window
        });
        window.present();
    }

    fn window(&self) -> Option<Rc<Window>> {
        self.window.borrow().clone()
    }

    pub fn submit(&self, text: String) {
        let message = self.log.borrow_mut().add(true, text.clone()).clone();
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
            Event::Answer { turn, text } => {
                self.thinking.borrow_mut().remove(&turn);
                let shown = self.answers.borrow().get(&turn).copied();
                match shown {
                    Some(id) => self.replace(id, text),
                    None => {
                        let id = self.add(false, text);
                        self.answers.borrow_mut().insert(turn, id);
                    }
                }
                self.show_thinking();
            }
            Event::Finished { turn, finish } => {
                self.thinking.borrow_mut().remove(&turn);
                let shown = self.answers.borrow_mut().remove(&turn);
                // "Clear history": the engine has forgotten the conversation,
                // so the log goes too, request included, before the reply.
                if finish.clear {
                    self.clear_log();
                }
                if let Some(reply) = finish.reply {
                    match shown {
                        Some(id) if !finish.clear => self.replace(id, reply.text),
                        _ => {
                            self.add(false, reply.text);
                        }
                    }
                }
                self.log.borrow().save();
                self.show_thinking();
            }
            Event::Logged(turn) => {
                if turn.clear {
                    self.clear_log();
                } else {
                    self.add(true, turn.heard);
                }
                if let Some(reply) = turn.reply {
                    self.add(false, reply.text);
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
        }
    }

    fn add(&self, from_user: bool, text: String) -> u64 {
        let message = self.log.borrow_mut().add(from_user, text).clone();
        if let Some(window) = self.window() {
            window.chat.add(&message);
        }
        message.id
    }

    fn replace(&self, id: u64, text: String) {
        let replaced = self.log.borrow_mut().replace(id, text.clone());
        if replaced && let Some(window) = self.window() {
            window.chat.replace(id, &text);
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
