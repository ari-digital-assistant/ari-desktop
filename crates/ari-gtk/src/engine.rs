//! The engine in this process: the runtime with its skills, and the voice
//! pipeline whose session runs typed turns now and spoken ones from stage 5
//! of dev.md. The session calls its host on threads of its own, so [`Host`]
//! hands each call to the main loop as an [`Event`].

use std::path::Path;
use std::sync::Arc;

use ari_runtime::{Providers, Runtime};
use ari_voice::pipeline::{LogLevel, VoiceHost, VoicePipeline};
use ari_voice::session::{
    ActionOutcome, CaptureMode, CardPressed, InterceptTargets, LoggedTurn, Phrase, SessionHost,
    SessionState, TypedFinish,
};
use ari_voice::wake_log::WakeLog;
use gtk4::gio;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, info, warn};

/// What the session told the host, for the main loop.
#[derive(Debug)]
pub enum Event {
    /// Typed turn `turn` has gone to the engine.
    Thinking(u64),
    /// An answer to typed turn `turn` while it is written: the words so far.
    Answer {
        turn: u64,
        text: String,
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
}

pub struct Engine {
    runtime: Arc<Runtime>,
    pipeline: Arc<VoicePipeline>,
}

impl Engine {
    /// Start the engine with `facts` remembered, keeping the wake log in
    /// `dir`. What it says arrives on the receiver.
    pub fn start(dir: &Path, facts: Vec<String>) -> (Self, UnboundedReceiver<Event>) {
        let (events, receiver) = unbounded_channel();
        let host = Arc::new(Host { events });
        let runtime = Arc::new(Runtime::new(Providers::default()));
        runtime.engine().set_remembered_facts(facts);
        let pipeline = VoicePipeline::new(
            host.clone(),
            Arc::new(WakeLog::new(dir.join("wake-log.txt"))),
        );
        pipeline.attach_session_host(host);
        pipeline.attach_engine(runtime.clone());
        (Self { runtime, pipeline }, receiver)
    }

    /// A turn typed into the chat. The chat shows the words itself.
    pub fn submit(&self, text: String) {
        self.pipeline.submit_text(text, false);
    }

    /// Forget the conversation: typed turns still running say nothing more.
    pub fn new_conversation(&self) {
        self.pipeline.cancel_typed();
        // Off the main loop: a cancelled turn still holds the engine until
        // its call returns.
        let runtime = self.runtime.clone();
        gio::spawn_blocking(move || runtime.engine().clear_conversation());
    }

    pub async fn facts(&self) -> Vec<String> {
        let runtime = self.runtime.clone();
        gio::spawn_blocking(move || runtime.engine().remembered_facts())
            .await
            .unwrap_or_else(|_| panic!("reading the remembered facts panicked"))
    }

    pub fn set_facts(&self, facts: Vec<String>) {
        let runtime = self.runtime.clone();
        gio::spawn_blocking(move || runtime.engine().set_remembered_facts(facts));
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
/// or load yet. Alerts and cards come with stage 4.
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
        InterceptTargets::default()
    }

    fn stop_alert(&self, _alert_id: &str) {}

    fn press_card_button(&self, _card_id: &str, _button_id: &str) -> CardPressed {
        CardPressed::Gone
    }

    fn handle_action(&self, json: &str, skill_id: &str) -> ActionOutcome {
        warn!(skill_id, json, "Ari for Linux doesn't run actions yet");
        ActionOutcome {
            text: String::new(),
            display_text: None,
            cards: false,
            attachments: false,
        }
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

    fn answer_text(&self, turn: u64, so_far: &str, _markdown: Option<&str>) {
        self.send(Event::Answer {
            turn,
            text: so_far.to_string(),
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
    /// finishes fails the test rather than hanging it.
    fn events(mut receiver: UnboundedReceiver<Event>) -> mpsc::Receiver<Event> {
        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Some(event) = receiver.blocking_recv() {
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

    #[test]
    fn a_typed_sum_is_answered() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, receiver) = Engine::start(dir.path(), Vec::new());
        let events = events(receiver);

        engine.submit("what's 12 times 7".into());
        let (before, finish) = until_finished(&events);

        assert!(matches!(before[..], [Event::Thinking(1)]), "{before:?}");
        assert_eq!(reply_text(finish), "84");
    }

    #[test]
    fn a_typed_time_question_is_answered() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, receiver) = Engine::start(dir.path(), Vec::new());
        let events = events(receiver);

        engine.submit("what time is it".into());
        let (_, finish) = until_finished(&events);

        let text = reply_text(finish);
        assert!(text.starts_with("It's "), "{text}");
    }

    #[test]
    fn remembering_says_the_facts_changed_and_the_engine_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, receiver) = Engine::start(dir.path(), vec!["I'm vegetarian".into()]);
        let events = events(receiver);

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
}
