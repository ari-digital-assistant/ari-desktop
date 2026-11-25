//! The Remembered page: the facts Ari was asked to remember, each with a
//! button to delete it.

use std::rc::Rc;

use gettextrs::gettext;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

pub struct RememberedPage {
    pub root: gtk4::ScrolledWindow,
    list: gtk4::ListBox,
    delete: Rc<dyn Fn(String)>,
}

impl RememberedPage {
    /// `delete` gets the fact whose delete button was pressed.
    pub fn new(delete: impl Fn(String) + 'static) -> Self {
        let intro = gtk4::Label::builder()
            .label(gettext(
                "Ari only remembers things when you ask it to: say “remember that…”. These are kept until you delete them.",
            ))
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        let list = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(14)
            .margin_top(32)
            .margin_bottom(32)
            .margin_start(24)
            .margin_end(24)
            .build();
        column.append(&intro);
        column.append(&list);
        let root = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(640)
                    .child(&column)
                    .build(),
            )
            .build();
        Self {
            root,
            list,
            delete: Rc::new(delete),
        }
    }

    pub fn show(&self, facts: &[String]) {
        self.list.remove_all();
        self.list.set_visible(!facts.is_empty());
        for fact in facts {
            let button = gtk4::Button::builder()
                .icon_name("user-trash-symbolic")
                .valign(gtk4::Align::Center)
                .tooltip_text(gettext("Delete"))
                .css_classes(["flat", "circular"])
                .build();
            button.update_property(&[gtk4::accessible::Property::Label(
                &gettext("Delete “{}”").replace("{}", fact),
            )]);
            let delete = self.delete.clone();
            let fact = fact.clone();
            let row = adw::ActionRow::builder()
                .title(&fact)
                .use_markup(false)
                .build();
            button.connect_clicked(move |_| delete(fact.clone()));
            row.add_suffix(&button);
            self.list.append(&row);
        }
    }
}
