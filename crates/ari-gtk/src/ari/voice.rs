//! Spoken turns: the microphone, open only while Ari listens, the turn's
//! state for the window, the "your turn" cue, and how speech is transcribed.

use std::rc::Rc;

use ari_voice::stt_models::SttModel;
use tokio::sync::watch;

use ari_voice::cloud_stt::CloudSttService;
use ari_voice::session::{SessionError, SessionState};
use gettextrs::gettext;
use gtk4::glib;
use gtk4::prelude::*;
use tracing::warn;

use super::Ari;
use crate::alerts;
use crate::engine::{self, Speech};
use crate::speech_models;

/// Where the cloud transcription key is kept, as on the phone.
pub const SPEECH_KEY: (&str, &str) = ("stt.cloud", "api_key");

impl Ari {
    /// Listen for a spoken turn, or stop the one under way.
    pub fn talk(self: &Rc<Self>) {
        if self.mic.borrow().is_some() {
            self.engine.dismiss();
            return;
        }
        match self.engine.open_mic() {
            Ok(mic) => *self.mic.borrow_mut() = Some(mic),
            Err(error) => {
                warn!(%error, "the microphone won't open");
                self.add(false, gettext("I can't open the microphone."), None);
                self.log.borrow().save();
                return;
            }
        }
        let ari = self.clone();
        glib::spawn_future_local(async move {
            if !ari.engine.start_turn().await {
                warn!("a voice turn is already under way");
            }
        });
    }

    pub(super) fn voice_state(&self, state: SessionState) {
        let listening = match &state {
            SessionState::Preparing { message } => Some(message.clone()),
            SessionState::Listening { partial } if partial.is_empty() => {
                Some(gettext("Listening…"))
            }
            SessionState::Listening { partial } => Some(partial.clone()),
            _ => None,
        };
        if let Some(window) = self.window() {
            window.chat.set_listening(listening);
            window.chat.set_talking(!matches!(
                state,
                SessionState::Idle { .. } | SessionState::Error(_)
            ));
        }
        match state {
            SessionState::Idle { .. } => {
                self.mic.borrow_mut().take();
            }
            SessionState::Error(error) => {
                self.mic.borrow_mut().take();
                if let Some(said) = trouble(&error) {
                    self.add(false, said, None);
                    self.log.borrow().save();
                }
            }
            _ => {}
        }
    }

    pub(super) fn cue(&self, volume: f32) {
        let sound = engine::installed().map(|installed| installed.sounds.join("ready.wav"));
        *self.cue.borrow_mut() = sound.and_then(|sound| alerts::cue(&sound, volume));
    }

    /// Hand the engine how Preferences says to transcribe speech, and load
    /// the on-device model if that's what it says.
    pub fn follow_speech(&self) {
        let settings = &self.settings;
        let cloud = match settings.string("speech-recognition").as_str() {
            "openai" => Some(CloudSttService::OpenAi),
            "self-hosted" => Some(CloudSttService::SelfHosted {
                endpoint: settings.string("speech-server").to_string(),
                model: settings.string("speech-server-model").to_string(),
            }),
            _ => None,
        };
        let on_device = cloud.is_none();
        let model = Some(settings.string("speech-model").to_string()).filter(|m| !m.is_empty());
        let (skill, key) = SPEECH_KEY;
        self.engine.set_speech(Speech {
            cloud,
            key: self.engine.settings.get(skill, key),
            model,
        });
        if on_device {
            let warm = self.engine.warm_speech();
            glib::spawn_future_local(async move {
                if warm.await.is_err() {
                    warn!("loading the speech model panicked");
                }
            });
        }
    }
}

/// Where a speech model's download is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Download {
    /// Bytes done of the whole.
    Running(u64, u64),
    Failed(String),
    Done,
}

impl Ari {
    /// Download `model`, or follow the download already under way. It goes
    /// on with Preferences closed, and its model loads once it's done.
    pub fn download_speech(self: &Rc<Self>, model: SttModel) -> watch::Receiver<Download> {
        if let Some(running) = self.downloads.borrow().get(model.id) {
            return running.clone();
        }
        let (tell, progress) = watch::channel(Download::Running(0, model.total_bytes));
        self.downloads
            .borrow_mut()
            .insert(model.id, progress.clone());
        let ari = self.clone();
        glib::spawn_future_local(async move {
            let fetched = gtk4::gio::spawn_blocking(move || {
                let dir = speech_models::dir(&model);
                let fetched = speech_models::download(&model, &dir, &|done, total| {
                    let _ = tell.send(Download::Running(done, total));
                });
                (tell, fetched)
            })
            .await;
            ari.downloads.borrow_mut().remove(model.id);
            let Ok((tell, fetched)) = fetched else {
                warn!(model = model.id, "the speech model's download panicked");
                return;
            };
            let _ = tell.send(match fetched {
                Ok(()) => {
                    ari.follow_speech();
                    Download::Done
                }
                Err(error) => {
                    warn!(error, model = model.id, "the speech model didn't download");
                    Download::Failed(error)
                }
            });
        });
        progress
    }

    /// The download of `model` under way, if there is one.
    pub fn speech_download(&self, model: &SttModel) -> Option<watch::Receiver<Download>> {
        self.downloads.borrow().get(model.id).cloned()
    }
}

/// What to tell the user when a voice turn fails, if anything.
fn trouble(error: &SessionError) -> Option<String> {
    match error {
        SessionError::NoSpeechModel => Some(gettext(
            "I need a speech model first. Download one in Preferences, under Speech.",
        )),
        // Nothing was said: nothing to say about it.
        SessionError::NoSpeech => None,
        SessionError::Stt(error) => {
            warn!(?error, "speech-to-text failed");
            Some(gettext("I couldn't make out what you said."))
        }
        SessionError::Failed(message) => {
            warn!(message, "a voice turn failed");
            Some(gettext("Something went wrong while I was listening."))
        }
    }
}
