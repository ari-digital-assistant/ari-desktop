//! The engine in this process: the runtime with its skills, the skill store
//! and assistants, and the voice pipeline whose session runs typed turns now
//! and spoken ones from stage 5 of dev.md. The session calls its host on
//! threads of its own, so [`Host`] hands each call to the main loop as an
//! [`Event`], and waits for the answer to those that return one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ari_engine::ConversationLength;
use ari_runtime::assistants::AssistantRegistry;
use ari_runtime::settings::SettingsStore;
use ari_runtime::skills::{SkillRegistry, SkillRegistryError};
use ari_runtime::{Providers, Reply, Runtime};
use ari_skill_loader::{Capability, HostCapabilities, LoadOptions, LocaleProvider};
use ari_voice::pipeline::{LogLevel, VoiceHost, VoicePipeline};
use ari_voice::session::{
    ActionOutcome, CaptureMode, CardPressed, InterceptTargets, LoggedTurn, Phrase, SessionHost,
    SessionState, TypedFinish,
};
use ari_voice::wake_log::WakeLog;
use gtk4::gio;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::providers::{Clock, Locale, SkillLog};
use crate::skill_settings::SkillSettings;

/// What Ari for Linux can run so far. Each stage of dev.md adds what it
/// builds; a skill needing anything more isn't offered or suggested.
fn host_capabilities() -> HostCapabilities {
    HostCapabilities::none()
        .with(Capability::Http)
        .with(Capability::StorageKv)
        .with(Capability::Notifications)
        .with(Capability::Clipboard)
        .with(Capability::CriticalAlert)
        .with(Capability::Alarm)
        .with(Capability::Navigation)
        .with(Capability::MediaControl)
}

/// Where the main loop sends the answer to a call the session waits on.
pub type Answer<T> = oneshot::Sender<T>;

/// What the session told the host, for the main loop.
#[derive(Debug)]
pub enum Event {
    /// Typed turn `turn` has gone to the engine.
    Thinking(u64),
    /// An answer to typed turn `turn` while it is written: the words so far.
    Answer {
        turn: u64,
        text: String,
        markdown: Option<String>,
    },
    Finished {
        turn: u64,
        finish: TypedFinish,
    },
    /// A spoken turn, for the log.
    Logged(LoggedTurn),
    /// A remember or forget command changed the engine's facts.
    FactsChanged,
    /// Speech the session asked for has finished, or here, never started.
    SpeechDone(u64),
    /// Run skill `skill_id`'s action envelope.
    Action {
        json: String,
        skill_id: String,
        done: Answer<ActionOutcome>,
    },
    /// What the words of a turn could answer instead of the engine: the
    /// sounding alerts and the cards with their buttons.
    Targets(Answer<InterceptTargets>),
    /// Press a card's button, the words of a turn having named it.
    Press {
        card: String,
        button: String,
        done: Answer<CardPressed>,
    },
    /// Stop a sounding alert, its stop having been said.
    StopAlert(String),
}

/// The `conversation-length` setting's value as the engine's.
pub fn conversation_length(name: &str) -> ConversationLength {
    match name {
        "medium" => ConversationLength::Medium,
        "long" => ConversationLength::Long,
        _ => ConversationLength::Short,
    }
}

/// How the engine starts: what the user set, and where its files go.
pub struct Setup<'a> {
    pub dir: &'a Path,
    pub facts: Vec<String>,
    pub language: String,
    pub conversation_memory: bool,
    pub conversation_length: ConversationLength,
    /// Runs the keyring's calls.
    pub tokio: Handle,
}

pub struct Engine {
    runtime: Arc<Runtime>,
    pipeline: Arc<VoicePipeline>,
    pub skills: Arc<SkillRegistry>,
    pub assistants: Arc<AssistantRegistry>,
    pub settings: Arc<SkillSettings>,
    locale: Arc<Locale>,
    skill_dir: PathBuf,
    options: LoadOptions,
}

impl Engine {
    /// Start the engine with the built-in skills. The installed ones follow
    /// with [`Self::reload_skills`]. What it says arrives on the receiver.
    pub fn start(setup: Setup) -> Result<(Self, UnboundedReceiver<Event>), SkillRegistryError> {
        let dir = setup.dir;
        let skill_dir = dir.join("skills");
        let options = ari_runtime::load_options(&dir.join("skill-storage"), host_capabilities());
        let store = Arc::new(SettingsStore::new());
        let settings =
            SkillSettings::open(store.clone(), dir.join("skill-settings.json"), setup.tokio);
        let locale = Arc::new(Locale::new(setup.language));
        let runtime = Arc::new(Runtime::new(Providers {
            log_sink: Arc::new(SkillLog),
            local_clock: Arc::new(Clock),
            locale: locale.clone(),
            config_store: store.clone(),
            setting_writer: settings.clone(),
            ..Providers::default()
        }));
        let skills = Arc::new(SkillRegistry::open(
            skill_dir.clone(),
            options.clone(),
            store.clone(),
        )?);
        let assistants = Arc::new(AssistantRegistry::new(
            skill_dir.clone(),
            options.clone(),
            store,
        ));
        {
            let mut engine = runtime.engine();
            engine.set_remembered_facts(setup.facts);
            engine.set_conversation_memory_enabled(setup.conversation_memory);
            engine.set_conversation_length(setup.conversation_length);
            engine.set_skill_suggestions_path(skills.index_cache_path(), host_capabilities());
        }

        let (events, receiver) = unbounded_channel();
        let host = Arc::new(Host { events });
        let pipeline = VoicePipeline::new(
            host.clone(),
            Arc::new(WakeLog::new(dir.join("wake-log.txt"))),
        );
        pipeline.attach_session_host(host);
        pipeline.attach_engine(runtime.clone());
        let engine = Self {
            runtime,
            pipeline,
            skills,
            assistants,
            settings,
            locale,
            skill_dir,
            options,
        };
        Ok((engine, receiver))
    }

    /// Run `call` on the runtime off the main loop: the engine is held for
    /// as long as a turn takes, which can be a while for a cloud assistant.
    fn off_main<T: Send + 'static>(
        &self,
        call: impl FnOnce(&Runtime) -> T + Send + 'static,
    ) -> gio::JoinHandle<T> {
        let runtime = self.runtime.clone();
        gio::spawn_blocking(move || call(&runtime))
    }

    /// Load the installed skills and assistants into the engine again, after
    /// an install, update or removal, and at start.
    pub fn reload_skills(&self) -> gio::JoinHandle<()> {
        let assistants = self.assistants.clone();
        let (dir, options) = (self.skill_dir.clone(), self.options.clone());
        self.off_main(move |runtime| {
            let loaded = runtime.reload_community_skills(&dir, options);
            assistants.reload_community_assistants();
            assistants.apply_to(runtime);
            info!(loaded, "installed skills loaded");
        })
    }

    /// Choose the assistant that answers when no skill does, or none.
    pub fn set_assistant(&self, id: Option<String>) {
        self.assistants.set_active_assistant(id);
        let assistants = self.assistants.clone();
        self.off_main(move |runtime| assistants.apply_to(runtime));
    }

    pub fn set_language(&self, code: String) {
        self.locale.set(code);
    }

    pub fn language(&self) -> String {
        self.locale.current_locale()
    }

    pub fn set_conversation_memory(&self, enabled: bool) {
        self.off_main(move |runtime| runtime.engine().set_conversation_memory_enabled(enabled));
    }

    pub fn set_conversation_length(&self, length: ConversationLength) {
        self.off_main(move |runtime| runtime.engine().set_conversation_length(length));
    }

    /// A turn typed into the chat. The chat shows the words itself.
    pub fn submit(&self, text: String) {
        self.pipeline.submit_text(text, false);
    }

    /// The engine's reply to `text`, outside the conversation's turns: the
    /// words a card's button or a notification's sends.
    pub async fn respond(&self, text: String) -> Option<Reply> {
        let reply = self.off_main(move |runtime| runtime.reply(&text, None, None).0);
        reply.await.ok()
    }

    /// The file a skill's `asset:` reference names, if it has one.
    pub fn asset(&self, skill_id: &str, reference: &str) -> Option<PathBuf> {
        self.skills.asset_path(skill_id, reference)
    }

    /// Forget the conversation: typed turns still running say nothing more.
    pub fn new_conversation(&self) {
        self.pipeline.cancel_typed();
        self.off_main(|runtime| runtime.engine().clear_conversation());
    }

    pub async fn facts(&self) -> Vec<String> {
        self.off_main(|runtime| runtime.engine().remembered_facts())
            .await
            .unwrap_or_else(|_| panic!("reading the remembered facts panicked"))
    }

    pub fn set_facts(&self, facts: Vec<String>) {
        self.off_main(move |runtime| runtime.engine().set_remembered_facts(facts));
    }

    pub fn speech_finished(&self, id: u64) {
        self.pipeline.speech_finished(id);
    }
}

struct Host {
    events: UnboundedSender<Event>,
}

impl Host {
    fn send(&self, event: Event) {
        // Only fails once the main loop has gone, as the app quits.
        let _ = self.events.send(event);
    }

    /// Hand the main loop a call and wait for its answer. None once the main
    /// loop has gone.
    fn ask<T>(&self, call: impl FnOnce(Answer<T>) -> Event) -> Option<T> {
        let (done, answer) = oneshot::channel();
        self.send(call(done));
        answer.blocking_recv().ok()
    }
}

impl VoiceHost for Host {
    /// The wake word and speech models arrive with stages 5 and 6.
    fn load_model(&self, name: &str) -> Option<Vec<u8>> {
        debug!(name, "no models yet");
        None
    }

    fn wake_detected(&self) {
        debug!("wake word heard, before Ari listens for one");
    }

    fn log(&self, level: LogLevel, message: &str) {
        match level {
            LogLevel::Debug => debug!(target: "ari_voice", "{message}"),
            LogLevel::Info => info!(target: "ari_voice", "{message}"),
            LogLevel::Warn => warn!(target: "ari_voice", "{message}"),
        }
    }
}

/// Only typed turns run until stage 5 of dev.md, and the session never starts
/// a spoken one, so the calls a spoken turn makes have nothing to show, play
/// or load yet.
impl SessionHost for Host {
    fn state(&self, state: SessionState) {
        debug!(?state, "voice session");
    }

    fn reveal(&self) {}

    fn play_cue(&self, _volume: f32) {}

    fn speak(&self, id: u64, text: &str, _flush: bool) {
        debug!(text, "nothing to speak with yet");
        self.send(Event::SpeechDone(id));
    }

    fn stop_speaking(&self) {}

    fn phrase(&self, _phrase: Phrase) -> String {
        String::new()
    }

    fn capture_mode(&self, _mode: CaptureMode) {}

    fn load_speech_model(&self) -> bool {
        false
    }

    fn intercept_targets(&self) -> InterceptTargets {
        self.ask(Event::Targets).unwrap_or_default()
    }

    fn stop_alert(&self, alert_id: &str) {
        self.send(Event::StopAlert(alert_id.to_string()));
    }

    fn press_card_button(&self, card_id: &str, button_id: &str) -> CardPressed {
        self.ask(|done| Event::Press {
            card: card_id.to_string(),
            button: button_id.to_string(),
            done,
        })
        .unwrap_or(CardPressed::Gone)
    }

    fn handle_action(&self, json: &str, skill_id: &str) -> ActionOutcome {
        self.ask(|done| Event::Action {
            json: json.to_string(),
            skill_id: skill_id.to_string(),
            done,
        })
        .unwrap_or(ActionOutcome {
            text: String::new(),
            display_text: None,
            cards: false,
            attachments: false,
        })
    }

    fn turn_logged(&self, turn: LoggedTurn) {
        self.send(Event::Logged(turn));
    }

    fn facts_changed(&self) {
        self.send(Event::FactsChanged);
    }

    fn false_wake(&self) {}

    fn dictated(&self, _text: &str) {}

    fn typed_thinking(&self, turn: u64) {
        self.send(Event::Thinking(turn));
    }

    fn answer_text(&self, turn: u64, so_far: &str, markdown: Option<&str>) {
        self.send(Event::Answer {
            turn,
            text: so_far.to_string(),
            markdown: markdown.map(str::to_string),
        });
    }

    fn typed_finished(&self, turn: u64, finish: TypedFinish) {
        self.send(Event::Finished { turn, finish });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// The events a session sends, with a deadline, so a turn that never
    /// finishes fails the test rather than hanging it. Nothing is on screen,
    /// so the words of every turn go to the engine.
    fn events(mut receiver: UnboundedReceiver<Event>) -> mpsc::Receiver<Event> {
        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Some(event) = receiver.blocking_recv() {
                if let Event::Targets(done) = event {
                    let _ = done.send(InterceptTargets::default());
                    continue;
                }
                if sender.send(event).is_err() {
                    break;
                }
            }
        });
        events
    }

    /// Every event up to and including the turn's end.
    fn until_finished(events: &mpsc::Receiver<Event>) -> (Vec<Event>, TypedFinish) {
        let mut before = Vec::new();
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(Event::Finished { finish, .. }) => return (before, finish),
                Ok(event) => before.push(event),
                Err(error) => panic!("the turn never finished: {error}"),
            }
        }
    }

    fn reply_text(finish: TypedFinish) -> String {
        finish.reply.expect("an answer").text
    }

    /// An engine with its files in `dir`, its events, and the runtime its
    /// keyring calls would run on, which has to outlive it.
    fn start(
        dir: &tempfile::TempDir,
        facts: Vec<String>,
    ) -> (Engine, mpsc::Receiver<Event>, tokio::runtime::Runtime) {
        let tokio = tokio::runtime::Runtime::new().unwrap();
        let (engine, receiver) = Engine::start(Setup {
            dir: dir.path(),
            facts,
            language: "en".into(),
            conversation_memory: true,
            conversation_length: ConversationLength::Short,
            tokio: tokio.handle().clone(),
        })
        .unwrap();
        (engine, events(receiver), tokio)
    }

    #[test]
    fn a_typed_sum_is_answered() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, events, _tokio) = start(&dir, Vec::new());

        engine.submit("what's 12 times 7".into());
        let (before, finish) = until_finished(&events);

        assert!(matches!(before[..], [Event::Thinking(1)]), "{before:?}");
        assert_eq!(reply_text(finish), "84");
    }

    #[test]
    fn a_typed_time_question_is_answered() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, events, _tokio) = start(&dir, Vec::new());

        engine.submit("what time is it".into());
        let (_, finish) = until_finished(&events);

        let text = reply_text(finish);
        assert!(text.starts_with("It's "), "{text}");
    }

    #[test]
    fn remembering_says_the_facts_changed_and_the_engine_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, events, _tokio) = start(&dir, vec!["I'm vegetarian".into()]);

        engine.submit("remember that my name is Keith".into());
        let (before, _) = until_finished(&events);

        assert!(
            before.iter().any(|e| matches!(e, Event::FactsChanged)),
            "{before:?}"
        );
        let facts = gtk4::glib::MainContext::new().block_on(engine.facts());
        // The engine keeps a fact as it normalised the words: lower case.
        assert_eq!(facts, ["I'm vegetarian", "my name is keith"]);
    }

    /// Against the live registry, so not run by default:
    /// `cargo test -- --ignored from_the_registry`.
    #[test]
    #[ignore]
    fn a_skill_installed_from_the_registry_answers() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, events, _tokio) = start(&dir, Vec::new());

        let browse = engine.skills.browse_registry().unwrap();
        let message = browse
            .iter()
            .find(|e| e.id == "dev.heyari.message")
            .unwrap();
        assert!(
            message
                .missing_capabilities
                .contains(&"send_message".to_string())
        );

        engine
            .skills
            .install_skill_by_id("dev.heyari.coinflip")
            .unwrap();
        // Spawned inside, as the job reports to the main context it was
        // started from.
        gtk4::glib::MainContext::new()
            .block_on(async { engine.reload_skills().await })
            .unwrap();
        engine.submit("flip a coin".into());
        let answer = reply_text(until_finished(&events).1);
        assert!(
            answer.contains("Heads") || answer.contains("Tails"),
            "{answer}"
        );
    }
}
