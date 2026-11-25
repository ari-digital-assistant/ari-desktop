//! The main window.

use gettextrs::gettext;
use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::APP_ID;

/// Show the main window, building it the first time. Closing it only hides
/// it, so later calls bring back the same window.
pub fn present(app: &adw::Application) {
    if let Some(window) = app.windows().into_iter().next() {
        window.present();
        return;
    }
    build(app).present();
}

fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let menu = gio::Menu::new();
    menu.append(Some(&gettext("_Preferences")), Some("app.preferences"));
    let menu_button = gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text(gettext("Main Menu"))
        .build();

    let header = adw::HeaderBar::new();
    header.pack_end(&menu_button);

    let page = adw::StatusPage::builder()
        .icon_name(APP_ID)
        .title("Ari")
        .build();

    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(&page));

    adw::ApplicationWindow::builder()
        .application(app)
        .title("Ari")
        .default_width(1000)
        .default_height(700)
        .hide_on_close(true)
        .content(&content)
        .build()
}
