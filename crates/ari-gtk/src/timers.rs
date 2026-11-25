//! The Timers and Alarms page: what's counting down, and the alarms Ari
//! keeps, each with a way to cancel it.

use std::rc::{Rc, Weak};
use std::time::Duration;

use gettextrs::gettext;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::alarms::{self, Alarm};
use crate::ari::Ari;
use crate::cards::{Cards, left, now_ms};

pub struct TimersPage {
    pub root: gtk4::Stack,
    list: gtk4::ListBox,
    ari: Weak<Ari>,
}

impl TimersPage {
    pub fn new(ari: &Rc<Ari>) -> Self {
        let list = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk4::Align::Start)
            .build();
        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(720)
                    .margin_top(24)
                    .margin_bottom(24)
                    .margin_start(24)
                    .margin_end(24)
                    .child(&list)
                    .build(),
            )
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("alarm-symbolic")
            .title(gettext("No Timers or Alarms"))
            .description(gettext("Ask Ari to set one."))
            .build();
        let root = gtk4::Stack::new();
        root.add_named(&empty, Some("empty"));
        root.add_named(&scroller, Some("list"));
        Self {
            root,
            list,
            ari: Rc::downgrade(ari),
        }
    }

    /// Show the cards still counting down, then the alarms, soonest first.
    pub fn show(&self, cards: &Cards, alarms: &[Alarm]) {
        self.list.remove_all();
        let now = now_ms();
        let mut shown = 0;
        for card in cards.all() {
            let Some(deadline) = card.countdown_to_ts_ms.filter(|&at| at > now) else {
                continue;
            };
            let row = adw::ActionRow::builder().title(&card.title).build();
            counting(&row, deadline);
            // Through the card's own Cancel, so its skill hears of it.
            if card.actions.iter().any(|a| a.id == "cancel") {
                let id = card.id.clone();
                row.add_suffix(&self.cancel(gettext("Cancel"), move |ari| ari.tap(&id, "cancel")));
            }
            self.list.append(&row);
            shown += 1;
        }
        for alarm in alarms {
            let title = alarm.message.clone().unwrap_or_else(|| gettext("Alarm"));
            let mut when = alarms::clock(alarm.hour, alarm.minute);
            if !alarm.days.is_empty() {
                when = format!("{when} · {}", alarms::day_names(&alarm.days));
            }
            let row = adw::ActionRow::builder()
                .title(&title)
                .subtitle(&when)
                .build();
            let id = alarm.id;
            row.add_suffix(&self.cancel(gettext("Delete Alarm"), move |ari| ari.delete_alarm(id)));
            self.list.append(&row);
            shown += 1;
        }
        self.root
            .set_visible_child_name(if shown == 0 { "empty" } else { "list" });
    }

    fn cancel(&self, tooltip: String, cancel: impl Fn(&Rc<Ari>) + 'static) -> gtk4::Button {
        let button = gtk4::Button::builder()
            .icon_name("edit-delete-symbolic")
            .tooltip_text(tooltip)
            .valign(gtk4::Align::Center)
            .css_classes(["flat"])
            .build();
        let ari = self.ari.clone();
        button.connect_clicked(move |_| {
            if let Some(ari) = ari.upgrade() {
                cancel(&ari);
            }
        });
        button
    }
}

/// Keep `row`'s subtitle on the time left until `deadline`.
fn counting(row: &adw::ActionRow, deadline: i64) {
    let show = move |row: &adw::ActionRow| {
        let now = now_ms();
        row.set_subtitle(&gettext("{} left").replace("{}", &left(deadline - now)));
        now < deadline
    };
    if show(row) {
        let weak = row.downgrade();
        glib::timeout_add_local(Duration::from_millis(250), move || match weak.upgrade() {
            Some(row) if show(&row) => glib::ControlFlow::Continue,
            _ => glib::ControlFlow::Break,
        });
    }
}
