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

    pub fn add(&mut self, from_user: bool, text: String) -> &Message {
        let id = self.next_id;
        self.next_id += 1;
        self.messages.push(Message {
            id,
            from_user,
            text,
        });
        self.messages.last().expect("just pushed")
    }

    /// Put `text` in place of message `id`'s. False when it has gone, cleared
    /// while its answer was being written.
    pub fn replace(&mut self, id: u64, text: String) -> bool {
        match self.messages.iter_mut().find(|m| m.id == id) {
            Some(message) => {
                message.text = text;
                true
            }
            None => false,
        }
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
        assert_eq!(log.add(true, "what time is it".into()).id, 1);
        assert_eq!(log.add(false, "It's 10:42.".into()).id, 2);
        assert_eq!(
            texts(&log),
            [(true, "what time is it"), (false, "It's 10:42.")]
        );
    }

    #[test]
    fn the_log_is_there_after_a_restart() {
        let (dir, mut log) = log();
        log.add(true, "what's 12 times 7".into());
        log.add(false, "84".into());
        log.save();

        let reopened = Log::open(dir.path().join("conversation.json"));
        assert_eq!(reopened.messages(), log.messages());
    }

    #[test]
    fn ids_go_on_from_the_last_after_a_restart() {
        let (dir, mut log) = log();
        log.add(true, "hello".into());
        log.add(false, "Hi".into());
        log.save();

        let mut reopened = Log::open(dir.path().join("conversation.json"));
        assert_eq!(reopened.add(true, "again".into()).id, 3);
    }

    #[test]
    fn an_answer_written_bit_by_bit_takes_its_final_text() {
        let (_dir, mut log) = log();
        let id = log.add(false, "The capital".into()).id;
        assert!(log.replace(id, "The capital of Malta is Valletta.".into()));
        assert_eq!(texts(&log), [(false, "The capital of Malta is Valletta.")]);
    }

    #[test]
    fn replacing_a_cleared_message_changes_nothing() {
        let (_dir, mut log) = log();
        let id = log.add(false, "The capital".into()).id;
        log.clear();
        assert!(!log.replace(id, "The capital of Malta is Valletta.".into()));
        assert!(log.messages().is_empty());
    }

    #[test]
    fn an_id_is_never_given_out_twice_even_after_a_clear() {
        let (_dir, mut log) = log();
        let answer = log.add(false, "The capital".into()).id;
        log.clear();
        assert_ne!(log.add(true, "hello".into()).id, answer);
        assert!(!log.replace(answer, "The capital of Malta is Valletta.".into()));
        assert_eq!(texts(&log), [(true, "hello")]);
    }

    #[test]
    fn a_cleared_log_stays_empty_after_a_restart() {
        let (dir, mut log) = log();
        log.add(true, "hello".into());
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
