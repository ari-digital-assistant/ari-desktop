//! The main window: a sidebar of pages, and the page picked in it. Closing it
//! only hides it, so the same window comes back.

use std::cell::Cell;
use std::rc::Rc;

use gettextrs::{gettext, ngettext};
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::APP_ID;
use crate::ari::Ari;
use crate::chat::ChatPage;
use crate::remembered::RememberedPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Conversation,
    Timers,
    Skills,
    Remembered,
}

impl Page {
    const ALL: [Page; 4] = [
        Page::Conversation,
        Page::Timers,
        Page::Skills,
        Page::Remembered,
    ];

    fn name(self) -> &'static str {
        match self {
            Page::Conversation => "conversation",
            Page::Timers => "timers",
            Page::Skills => "skills",
            Page::Remembered => "remembered",
        }
    }

    fn title(self) -> String {
        match self {
            Page::Conversation => gettext("Conversation"),
            Page::Timers => gettext("Timers and Alarms"),
            Page::Skills => gettext("Skills"),
            Page::Remembered => gettext("Remembered"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Conversation => "chat-message-new-symbolic",
            Page::Timers => "alarm-symbolic",
            Page::Skills => "view-grid-symbolic",
            Page::Remembered => "user-bookmarks-symbolic",
        }
    }
}

pub struct Window {
    window: adw::ApplicationWindow,
    pub chat: Rc<ChatPage>,
    pub remembered: RememberedPage,
    page: Cell<Page>,
    facts: Cell<usize>,
    title: adw::WindowTitle,
    new_conversation: gtk4::Button,
}

impl Window {
    pub fn new(app: &adw::Application, ari: &Rc<Ari>) -> Rc<Self> {
        let chat = ChatPage::new(glib::clone!(
            #[weak]
            ari,
            move |text| ari.submit(text)
        ));
        let remembered = RememberedPage::new(glib::clone!(
            #[weak]
            ari,
            move |fact| ari.delete_fact(&fact)
        ));

        let stack = gtk4::Stack::new();
        for page in Page::ALL {
            let child: gtk4::Widget = match page {
                Page::Conversation => chat.root.clone().upcast(),
                Page::Remembered => remembered.root.clone().upcast(),
                // Filled in by stages 3 and 4 of dev.md.
                Page::Timers | Page::Skills => {
                    gtk4::Box::new(gtk4::Orientation::Vertical, 0).upcast()
                }
            };
            stack.add_named(&child, Some(page.name()));
        }

        let new_conversation = gtk4::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text(gettext("New Conversation"))
            .build();
        new_conversation.connect_clicked(glib::clone!(
            #[weak]
            ari,
            move |_| ari.new_conversation()
        ));
        let title = adw::WindowTitle::new("", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_end(&new_conversation);
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&stack));
        let content = adw::NavigationPage::builder()
            .title("Ari")
            .child(&content)
            .build();

        let pages = gtk4::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        for page in Page::ALL {
            pages.append(&sidebar_row(page));
        }
        let sidebar = adw::ToolbarView::new();
        sidebar.add_top_bar(&sidebar_header());
        sidebar.set_content(Some(&pages));
        let sidebar = adw::NavigationPage::builder()
            .title("Ari")
            .child(&sidebar)
            .build();

        let split = adw::NavigationSplitView::builder()
            .sidebar(&sidebar)
            .content(&content)
            .build();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Ari")
            .default_width(1000)
            .default_height(700)
            .width_request(360)
            .height_request(294)
            .hide_on_close(true)
            .content(&split)
            .build();
        let narrow = adw::Breakpoint::new(
            adw::BreakpointCondition::parse("max-width: 600sp").expect("a valid condition"),
        );
        narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(narrow);

        let window = Rc::new(Self {
            window,
            chat,
            remembered,
            page: Cell::new(Page::Conversation),
            facts: Cell::new(0),
            title,
            new_conversation,
        });
        pages.connect_row_activated(glib::clone!(
            #[weak]
            window,
            #[weak]
            stack,
            #[weak]
            split,
            move |_, row| {
                let page = Page::ALL[row.index() as usize];
                stack.set_visible_child_name(page.name());
                window.page.set(page);
                window.show_title();
                split.set_show_content(true);
            }
        ));
        pages.select_row(pages.row_at_index(0).as_ref());
        window.show_title();
        window
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn show_facts(&self, facts: &[String]) {
        self.remembered.show(facts);
        self.facts.set(facts.len());
        self.show_title();
    }

    fn show_title(&self) {
        let page = self.page.get();
        self.title.set_title(&page.title());
        let subtitle = match page {
            Page::Remembered => {
                let count = self.facts.get();
                ngettext("{} fact", "{} facts", count as u32).replace("{}", &count.to_string())
            }
            _ => String::new(),
        };
        self.title.set_subtitle(&subtitle);
        self.new_conversation
            .set_visible(page == Page::Conversation);
    }
}

fn sidebar_header() -> adw::HeaderBar {
    let menu = gio::Menu::new();
    menu.append(Some(&gettext("_Preferences")), Some("app.preferences"));
    let menu_button = gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text(gettext("Main Menu"))
        .build();
    let title = gtk4::Box::builder().spacing(10).build();
    title.append(
        &gtk4::Image::builder()
            .icon_name(APP_ID)
            .pixel_size(22)
            .build(),
    );
    title.append(
        &gtk4::Label::builder()
            .label("Ari")
            .css_classes(["heading"])
            .build(),
    );
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.pack_end(&menu_button);
    header
}

fn sidebar_row(page: Page) -> gtk4::Box {
    let row = gtk4::Box::builder().spacing(12).build();
    row.append(&gtk4::Image::from_icon_name(page.icon()));
    row.append(&gtk4::Label::new(Some(&page.title())));
    row
}
