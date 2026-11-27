//! The Conversation page: the messages, and the composer to type to Ari.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gettextrs::gettext;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::APP_ID;
use crate::conversation::{Attachment, Message};
use crate::markdown;

/// A card drawn as it is now, by its id, or None once it has gone.
pub type CardWidget = Box<dyn Fn(&str) -> Option<gtk4::Widget>>;

/// How the conversation draws what shows under an answer.
pub struct Attachments {
    pub card: CardWidget,
    /// Show a skill from the store.
    pub open_skill: Rc<dyn Fn(&str)>,
}

pub struct ChatPage {
    pub root: gtk4::Box,
    messages: gtk4::Box,
    /// Each message's box, whose content is drawn again when it changes.
    contents: RefCell<HashMap<u64, gtk4::Box>>,
    attachments: Attachments,
    /// Where each card shows, drawn again when it changes.
    card_slots: RefCell<HashMap<String, Vec<glib::WeakRef<gtk4::Box>>>>,
    history: RefCell<History>,
    /// The row under the messages saying what Ari is doing.
    thinking: gtk4::Box,
    status: gtk4::Label,
    /// What Ari has heard so far, while it listens.
    listening: RefCell<Option<String>>,
    /// A typed turn is waiting on the engine.
    waiting: Cell<bool>,
    composer: gtk4::Box,
    entry: gtk4::Entry,
    button: gtk4::Button,
    /// A voice turn is under way.
    talking: Rc<Cell<bool>>,
    /// Keep the newest message in view: true until the user scrolls up.
    follow: Rc<Cell<bool>>,
}

impl ChatPage {
    /// `send` gets what the user typed, trimmed and never empty; `talk`
    /// starts a voice turn, or stops the one under way.
    pub fn new(
        send: impl Fn(String) + 'static,
        talk: impl Fn() + 'static,
        attachments: Attachments,
    ) -> Rc<Self> {
        let messages = gtk4::Box::new(gtk4::Orientation::Vertical, 20);
        let thinking = gtk4::Box::builder().spacing(12).visible(false).build();
        thinking.append(&avatar());
        let status = gtk4::Label::builder()
            .label(gettext("Thinking…"))
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        thinking.append(&status);
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(20)
            .margin_top(28)
            .margin_bottom(16)
            .margin_start(24)
            .margin_end(24)
            .build();
        column.append(&messages);
        column.append(&thinking);
        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(760)
                    .child(&column)
                    .build(),
            )
            .build();

        let follow = Rc::new(Cell::new(true));
        // The height the view last followed to. A move away from the bottom
        // with the height unchanged is the user scrolling up; one that comes
        // with a new height is the conversation growing, which mustn't stop
        // the following.
        let followed = Rc::new(Cell::new(0.0));
        let adjustment = scroller.vadjustment();
        adjustment.connect_value_changed(glib::clone!(
            #[strong]
            follow,
            #[strong]
            followed,
            move |a| {
                if a.value() >= a.upper() - a.page_size() - 1.0 {
                    follow.set(true);
                } else if a.upper() == followed.get() {
                    follow.set(false);
                }
            }
        ));
        let stick = glib::clone!(
            #[strong]
            follow,
            move |a: &gtk4::Adjustment| {
                followed.set(a.upper());
                if follow.get() {
                    a.set_value(a.upper() - a.page_size());
                }
            }
        );
        adjustment.connect_upper_notify(stick.clone());
        adjustment.connect_page_size_notify(stick);

        let talking = Rc::new(Cell::new(false));
        let (composer, entry, button) = composer(talking.clone());
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.append(&scroller);
        root.append(
            &adw::Clamp::builder()
                .maximum_size(760)
                .margin_top(8)
                .margin_bottom(18)
                .margin_start(24)
                .margin_end(24)
                .child(&composer)
                .build(),
        );

        let page = Rc::new(Self {
            root,
            messages,
            contents: RefCell::default(),
            attachments,
            card_slots: RefCell::default(),
            history: RefCell::default(),
            thinking,
            status,
            listening: RefCell::default(),
            waiting: Cell::new(false),
            composer,
            entry: entry.clone(),
            button: button.clone(),
            talking,
            follow,
        });
        let send = Rc::new(move |entry: &gtk4::Entry| {
            let text = entry.text().trim().to_string();
            if !text.is_empty() {
                entry.set_text("");
                send(text);
            }
        });
        entry.connect_activate(glib::clone!(
            #[strong]
            send,
            move |entry| send(entry)
        ));
        button.connect_clicked(glib::clone!(
            #[weak]
            entry,
            move |_| {
                if entry.text().trim().is_empty() {
                    talk();
                } else {
                    send(&entry);
                }
            }
        ));

        // Up and Down step through what was sent before, as in a shell. They
        // never move focus out of the field: past the oldest message, Up
        // would otherwise land in the conversation above.
        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[weak]
            page,
            #[weak]
            entry,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| {
                if !modifiers.is_empty() {
                    return glib::Propagation::Proceed;
                }
                let recalled = match key {
                    gtk4::gdk::Key::Up => page.history.borrow_mut().back(&entry.text()),
                    gtk4::gdk::Key::Down => page.history.borrow_mut().forward(),
                    _ => return glib::Propagation::Proceed,
                };
                if let Some(text) = recalled {
                    entry.set_text(&text);
                    entry.set_position(-1);
                }
                glib::Propagation::Stop
            }
        ));
        entry.add_controller(keys);
        page
    }

    /// Show `messages` in place of everything shown.
    pub fn show(&self, messages: &[Message]) {
        while let Some(child) = self.messages.first_child() {
            self.messages.remove(&child);
        }
        self.contents.borrow_mut().clear();
        self.card_slots.borrow_mut().clear();
        self.history.borrow_mut().clear();
        self.follow.set(true);
        for message in messages {
            self.add(message);
        }
    }

    pub fn add(&self, message: &Message) {
        // What the user sends is always brought into view.
        if message.from_user {
            self.follow.set(true);
            self.history.borrow_mut().push(message.text.clone());
        }
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        self.show_content(&content, message);
        let row: gtk4::Widget = if message.from_user {
            content.set_halign(gtk4::Align::End);
            content.add_css_class("user-message");
            content.clone().upcast()
        } else {
            let row = gtk4::Box::builder()
                .spacing(12)
                .halign(gtk4::Align::Start)
                .build();
            row.append(&avatar());
            row.append(&content);
            row.upcast()
        };
        self.messages.append(&row);
        self.contents.borrow_mut().insert(message.id, content);
    }

    pub fn replace(&self, message: &Message) {
        if let Some(content) = self.contents.borrow().get(&message.id) {
            self.show_content(content, message);
        }
    }

    /// Draw card `id` again wherever it shows, or nothing once it has gone.
    pub fn refresh_card(&self, id: &str) {
        let mut slots = self.card_slots.borrow_mut();
        let Some(shown) = slots.get_mut(id) else {
            return;
        };
        shown.retain(|slot| slot.upgrade().is_some());
        for slot in shown.iter().filter_map(|slot| slot.upgrade()) {
            self.fill_card(&slot, id);
        }
    }

    fn fill_card(&self, slot: &gtk4::Box, id: &str) {
        while let Some(child) = slot.first_child() {
            slot.remove(&child);
        }
        if let Some(card) = (self.attachments.card)(id) {
            slot.append(&card);
        }
    }

    fn show_content(&self, content: &gtk4::Box, message: &Message) {
        while let Some(child) = content.first_child() {
            content.remove(&child);
        }
        if !message.text.is_empty() || message.attachments.is_empty() {
            content.append(&words(message));
        }
        for attachment in &message.attachments {
            match attachment {
                Attachment::Card { id } => {
                    let slot = gtk4::Box::builder().halign(gtk4::Align::Start).build();
                    self.fill_card(&slot, id);
                    self.card_slots
                        .borrow_mut()
                        .entry(id.clone())
                        .or_default()
                        .push(slot.downgrade());
                    content.append(&slot);
                }
                Attachment::Suggestion { id, name } => {
                    let button = gtk4::Button::builder()
                        .label(gettext("Get {}").replace("{}", name))
                        .halign(gtk4::Align::Start)
                        .css_classes(["pill", "suggested-action"])
                        .build();
                    let (open, id) = (self.attachments.open_skill.clone(), id.clone());
                    button.connect_clicked(move |_| open(&id));
                    content.append(&button);
                }
            }
        }
    }

    pub fn set_thinking(&self, thinking: bool) {
        self.waiting.set(thinking);
        if thinking {
            self.composer.add_css_class("thinking");
        } else {
            self.composer.remove_css_class("thinking");
        }
        self.show_status();
    }

    /// Show what Ari has heard so far, or None once it has stopped listening.
    pub fn set_listening(&self, heard: Option<String>) {
        *self.listening.borrow_mut() = heard;
        self.show_status();
    }

    /// A voice turn has started or ended: the mic button stops it meanwhile.
    pub fn set_talking(&self, talking: bool) {
        self.talking.set(talking);
        show_button(&self.button, !self.entry.text().trim().is_empty(), talking);
    }

    fn show_status(&self) {
        match &*self.listening.borrow() {
            Some(heard) => {
                self.status.set_label(heard);
                self.thinking.set_visible(true);
            }
            None => {
                self.status.set_label(&gettext("Thinking…"));
                self.thinking.set_visible(self.waiting.get());
            }
        }
    }
}

/// A message's words, as markdown when the assistant wrote some.
fn words(message: &Message) -> gtk4::Widget {
    let width = if message.from_user { 56 } else { 72 };
    match &message.markdown {
        Some(markdown) => markdown::widget(markdown, width).upcast(),
        None => gtk4::Label::builder()
            .label(&message.text)
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .xalign(0.0)
            .selectable(true)
            .max_width_chars(width)
            .build()
            .upcast(),
    }
}

/// What the user has sent, for Up and Down in the composer.
#[derive(Default)]
struct History {
    sent: Vec<String>,
    /// The message shown in the composer, if it's one from here.
    at: Option<usize>,
    /// What was typed before stepping back, for stepping forward past the end.
    draft: String,
}

impl History {
    fn push(&mut self, text: String) {
        self.sent.push(text);
        self.at = None;
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    /// The message before the one shown, keeping `typed` for later. None at
    /// the oldest.
    fn back(&mut self, typed: &str) -> Option<String> {
        let at = match self.at {
            None if self.sent.is_empty() => return None,
            None => {
                self.draft = typed.to_string();
                self.sent.len() - 1
            }
            Some(0) => return None,
            Some(at) => at - 1,
        };
        self.at = Some(at);
        Some(self.sent[at].clone())
    }

    /// The message after the one shown, or past the newest what was typed.
    /// None when not stepping through.
    fn forward(&mut self) -> Option<String> {
        let at = self.at?;
        if at + 1 < self.sent.len() {
            self.at = Some(at + 1);
            return Some(self.sent[at + 1].clone());
        }
        self.at = None;
        Some(std::mem::take(&mut self.draft))
    }
}

fn avatar() -> gtk4::Image {
    gtk4::Image::builder()
        .icon_name(APP_ID)
        .pixel_size(28)
        .valign(gtk4::Align::Start)
        .build()
}

/// The text field in its ring, and the button beside it: the microphone
/// while there's nothing typed, Send once there is, and Stop while Ari
/// listens.
fn composer(talking: Rc<Cell<bool>>) -> (gtk4::Box, gtk4::Entry, gtk4::Button) {
    let entry = gtk4::Entry::builder()
        .hexpand(true)
        .has_frame(false)
        .placeholder_text(gettext("Ask Ari something"))
        .build();
    entry.update_property(&[gtk4::accessible::Property::Label(&gettext("Message Ari"))]);
    let button = gtk4::Button::builder()
        .valign(gtk4::Align::Center)
        .css_classes(["circular", "suggested-action"])
        .build();
    show_button(&button, false, false);
    entry.connect_changed(glib::clone!(
        #[weak]
        button,
        move |entry| show_button(&button, !entry.text().trim().is_empty(), talking.get())
    ));

    let composer = gtk4::Box::builder()
        .spacing(10)
        .css_classes(["composer"])
        .build();
    composer.append(&entry);
    composer.append(&button);
    (composer, entry, button)
}

fn show_button(button: &gtk4::Button, typed: bool, talking: bool) {
    let (icon, tooltip) = if typed {
        ("go-up-symbolic", gettext("Send"))
    } else if talking {
        ("media-playback-stop-symbolic", gettext("Stop Listening"))
    } else {
        ("audio-input-microphone-symbolic", gettext("Talk to Ari"))
    };
    button.set_icon_name(icon);
    button.set_tooltip_text(Some(&tooltip));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(sent: &[&str]) -> History {
        let mut history = History::default();
        for text in sent {
            history.push(text.to_string());
        }
        history
    }

    #[test]
    fn up_steps_back_from_the_newest_and_stops_at_the_oldest() {
        let mut h = history(&["one", "two", "three"]);
        assert_eq!(h.back("").as_deref(), Some("three"));
        assert_eq!(h.back("").as_deref(), Some("two"));
        assert_eq!(h.back("").as_deref(), Some("one"));
        assert_eq!(h.back(""), None);
    }

    #[test]
    fn down_steps_forward_and_past_the_newest_gives_back_what_was_typed() {
        let mut h = history(&["one", "two"]);
        h.back("half a thou");
        h.back("");
        assert_eq!(h.forward().as_deref(), Some("two"));
        assert_eq!(h.forward().as_deref(), Some("half a thou"));
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn nothing_sent_means_nothing_to_step_through() {
        let mut h = History::default();
        assert_eq!(h.back("typing"), None);
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn sending_starts_again_from_the_newest() {
        let mut h = history(&["one", "two"]);
        h.back("");
        h.back("");
        h.push("three".into());
        assert_eq!(h.back("").as_deref(), Some("three"));
    }
}
