//! Ari's state in this process, window or no window: the engine, the
//! conversation log and the remembered facts. The window shows them while
//! it's open.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use ari_runtime::skills::SkillRegistryError;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use tokio::runtime::Handle;
use tracing::warn;

use crate::conversation::Log;
use crate::engine::{Engine, Event, Setup, conversation_length};
use crate::window::Window;
use crate::{providers, store};

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
        Ok(ari)
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
                // "Clear history": the engine has forgotten the conversation,
                // so the log goes too, request included, before the reply.
                if finish.clear {
                    self.clear_log();
                }
                if let Some(reply) = finish.reply {
                    match shown {
                        Some(id) if !finish.clear => self.replace(id, reply.text, reply.markdown),
                        _ => {
                            self.add(false, reply.text, reply.markdown);
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
                    self.add(true, turn.heard, None);
                }
                if let Some(reply) = turn.reply {
                    self.add(false, reply.text, reply.markdown);
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

    fn add(&self, from_user: bool, text: String, markdown: Option<String>) -> u64 {
        let message = self.log.borrow_mut().add(from_user, text, markdown).clone();
        if let Some(window) = self.window() {
            window.chat.add(&message);
        }
        message.id
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

fn assistant(settings: &gio::Settings, key: &str) -> Option<String> {
    Some(settings.string(key).to_string()).filter(|id| !id.is_empty())
}
