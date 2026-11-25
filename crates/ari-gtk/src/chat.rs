//! The Conversation page: the messages, and the composer to type to Ari.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gettextrs::gettext;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::APP_ID;
use crate::conversation::Message;

pub struct ChatPage {
    pub root: gtk4::Box,
    messages: gtk4::Box,
    labels: RefCell<HashMap<u64, gtk4::Label>>,
    thinking: gtk4::Box,
    composer: gtk4::Box,
    /// Keep the newest message in view: true until the user scrolls up.
    follow: Rc<Cell<bool>>,
}

impl ChatPage {
    /// `send` gets what the user typed, trimmed and never empty.
    pub fn new(send: impl Fn(String) + 'static) -> Rc<Self> {
        let messages = gtk4::Box::new(gtk4::Orientation::Vertical, 20);
        let thinking = gtk4::Box::builder().spacing(12).visible(false).build();
        thinking.append(&avatar());
        thinking.append(
            &gtk4::Label::builder()
                .label(gettext("Thinking…"))
                .css_classes(["dim-label"])
                .build(),
        );
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
        let adjustment = scroller.vadjustment();
        adjustment.connect_value_changed(glib::clone!(
            #[strong]
            follow,
            move |a| follow.set(a.value() >= a.upper() - a.page_size() - 1.0)
        ));
        adjustment.connect_upper_notify(glib::clone!(
            #[strong]
            follow,
            move |a| {
                if follow.get() {
                    a.set_value(a.upper() - a.page_size());
                }
            }
        ));

        let (composer, entry, button) = composer();
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
            labels: RefCell::default(),
            thinking,
            composer,
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
            move |_| send(&entry)
        ));
        page
    }

    /// Show `messages` in place of everything shown.
    pub fn show(&self, messages: &[Message]) {
        while let Some(child) = self.messages.first_child() {
            self.messages.remove(&child);
        }
        self.labels.borrow_mut().clear();
        self.follow.set(true);
        for message in messages {
            self.add(message);
        }
    }

    pub fn add(&self, message: &Message) {
        // What the user sends is always brought into view.
        if message.from_user {
            self.follow.set(true);
        }
        let label = gtk4::Label::builder()
            .label(&message.text)
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .xalign(0.0)
            .selectable(true)
            .build();
        let row: gtk4::Widget = if message.from_user {
            label.set_max_width_chars(56);
            label.set_halign(gtk4::Align::End);
            label.add_css_class("user-message");
            label.clone().upcast()
        } else {
            label.set_max_width_chars(72);
            let row = gtk4::Box::builder()
                .spacing(12)
                .halign(gtk4::Align::Start)
                .build();
            row.append(&avatar());
            row.append(&label);
            row.upcast()
        };
        self.messages.append(&row);
        self.labels.borrow_mut().insert(message.id, label);
    }

    pub fn replace(&self, id: u64, text: &str) {
        if let Some(label) = self.labels.borrow().get(&id) {
            label.set_label(text);
        }
    }

    pub fn set_thinking(&self, thinking: bool) {
        self.thinking.set_visible(thinking);
        if thinking {
            self.composer.add_css_class("thinking");
        } else {
            self.composer.remove_css_class("thinking");
        }
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
/// while there's nothing typed, Send once there is. The microphone does
/// nothing until stage 5 of dev.md, so it's greyed out until then.
fn composer() -> (gtk4::Box, gtk4::Entry, gtk4::Button) {
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
    show_button(&button, false);
    entry.connect_changed(glib::clone!(
        #[weak]
        button,
        move |entry| show_button(&button, !entry.text().trim().is_empty())
    ));

    let composer = gtk4::Box::builder()
        .spacing(10)
        .css_classes(["composer"])
        .build();
    composer.append(&entry);
    composer.append(&button);
    (composer, entry, button)
}

fn show_button(button: &gtk4::Button, typed: bool) {
    if typed {
        button.set_icon_name("go-up-symbolic");
        button.set_tooltip_text(Some(&gettext("Send")));
    } else {
        button.set_icon_name("audio-input-microphone-symbolic");
        button.set_tooltip_text(Some(&gettext("Talk to Ari")));
    }
    button.set_sensitive(typed);
}
