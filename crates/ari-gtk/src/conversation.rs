//! The conversation log: what the Conversation page shows, kept on disk so
//! it's still there after a restart.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::store;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub from_user: bool,
    pub text: String,
    /// An assistant's answer as it wrote it, shown in place of `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// What shows under an answer besides its words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Attachment {
    /// A skill's card, shown for as long as the skill keeps it.
    Card { id: String },
    /// A skill in the store that would answer what was asked.
    Suggestion { id: String, name: String },
}

pub struct Log {
    path: PathBuf,
    messages: Vec<Message>,
    // Not reset by a clear: an answer still being written keeps the id it was
    // given, which must never come round again.
    next_id: u64,
}

impl Log {
    pub fn open(path: PathBuf) -> Self {
        let messages: Vec<Message> = store::load(&path);
        let next_id = messages.last().map_or(1, |last| last.id + 1);
        Self {
            path,
            messages,
            next_id,
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn add(&mut self, from_user: bool, text: String, markdown: Option<String>) -> &Message {
        let id = self.next_id;
        self.next_id += 1;
        self.messages.push(Message {
            id,
            from_user,
            text,
            markdown,
            attachments: Vec::new(),
        });
        self.messages.last().expect("just pushed")
    }

    /// Put `text` in place of message `id`'s. None when it has gone, cleared
    /// while its answer was being written.
    pub fn replace(&mut self, id: u64, text: String, markdown: Option<String>) -> Option<&Message> {
        let message = self.messages.iter_mut().find(|m| m.id == id)?;
        message.text = text;
        message.markdown = markdown;
        Some(message)
    }

    /// Show `attachments` under message `id`. None when it has gone.
    pub fn attach(&mut self, id: u64, attachments: Vec<Attachment>) -> Option<&Message> {
        let message = self.messages.iter_mut().find(|m| m.id == id)?;
        message.attachments = attachments;
        Some(message)
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn save(&self) {
        if let Err(error) = store::save(&self.path, &self.messages) {
            warn!(%error, path = %self.path.display(), "couldn't save the conversation");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> (tempfile::TempDir, Log) {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::open(dir.path().join("conversation.json"));
        (dir, log)
    }

    fn texts(log: &Log) -> Vec<(bool, &str)> {
        log.messages()
            .iter()
            .map(|m| (m.from_user, m.text.as_str()))
            .collect()
    }

    #[test]
    fn messages_are_kept_in_order_with_rising_ids() {
        let (_dir, mut log) = log();
        assert_eq!(log.add(true, "what time is it".into(), None).id, 1);
        assert_eq!(log.add(false, "It's 10:42.".into(), None).id, 2);
        assert_eq!(
            texts(&log),
            [(true, "what time is it"), (false, "It's 10:42.")]
        );
    }

    #[test]
    fn the_log_is_there_after_a_restart() {
        let (dir, mut log) = log();
        log.add(true, "what's 12 times 7".into(), None);
        log.add(false, "84".into(), None);
        log.save();

        let reopened = Log::open(dir.path().join("conversation.json"));
        assert_eq!(reopened.messages(), log.messages());
    }

    #[test]
    fn ids_go_on_from_the_last_after_a_restart() {
        let (dir, mut log) = log();
        log.add(true, "hello".into(), None);
        log.add(false, "Hi".into(), None);
        log.save();

        let mut reopened = Log::open(dir.path().join("conversation.json"));
        assert_eq!(reopened.add(true, "again".into(), None).id, 3);
    }

    #[test]
    fn an_answer_written_bit_by_bit_takes_its_final_text() {
        let (_dir, mut log) = log();
        let id = log.add(false, "The capital".into(), None).id;
        assert!(
            log.replace(id, "The capital of Malta is Valletta.".into(), None)
                .is_some()
        );
        assert_eq!(texts(&log), [(false, "The capital of Malta is Valletta.")]);
    }

    #[test]
    fn an_answer_s_markdown_is_kept_and_a_log_without_any_still_reads() {
        let (dir, mut log) = log();
        log.add(
            false,
            "Tea or coffee".into(),
            Some("**Tea** or coffee".into()),
        );
        log.save();
        let saved = std::fs::read_to_string(dir.path().join("conversation.json")).unwrap();
        assert!(
            saved.contains(r#""markdown":"**Tea** or coffee""#),
            "{saved}"
        );

        std::fs::write(
            dir.path().join("conversation.json"),
            r#"[{"id":1,"from_user":true,"text":"hello"}]"#,
        )
        .unwrap();
        let old = Log::open(dir.path().join("conversation.json"));
        assert_eq!(old.messages()[0].markdown, None);
    }

    #[test]
    fn an_answer_s_attachments_are_there_after_a_restart() {
        let (dir, mut log) = log();
        let id = log.add(false, "Timer set.".into(), None).id;
        let attachments = vec![
            Attachment::Card {
                id: "card_pasta".into(),
            },
            Attachment::Suggestion {
                id: "dev.heyari.coinflip".into(),
                name: "Coin Flip".into(),
            },
        ];
        log.attach(id, attachments.clone());
        log.save();

        let reopened = Log::open(dir.path().join("conversation.json"));
        assert_eq!(reopened.messages()[0].attachments, attachments);
        assert!(log.attach(id + 1, Vec::new()).is_none());
    }

    #[test]
    fn replacing_a_cleared_message_changes_nothing() {
        let (_dir, mut log) = log();
        let id = log.add(false, "The capital".into(), None).id;
        log.clear();
        assert!(
            log.replace(id, "The capital of Malta is Valletta.".into(), None)
                .is_none()
        );
        assert!(log.messages().is_empty());
    }

    #[test]
    fn an_id_is_never_given_out_twice_even_after_a_clear() {
        let (_dir, mut log) = log();
        let answer = log.add(false, "The capital".into(), None).id;
        log.clear();
        assert_ne!(log.add(true, "hello".into(), None).id, answer);
        assert!(
            log.replace(answer, "The capital of Malta is Valletta.".into(), None)
                .is_none()
        );
        assert_eq!(texts(&log), [(true, "hello")]);
    }

    #[test]
    fn a_cleared_log_stays_empty_after_a_restart() {
        let (dir, mut log) = log();
        log.add(true, "hello".into(), None);
        log.save();
        log.clear();
        log.save();

        assert!(
            Log::open(dir.path().join("conversation.json"))
                .messages()
                .is_empty()
        );
    }
}
