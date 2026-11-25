//! Translations, through the system's gettext.

use std::path::PathBuf;

use gettextrs::{bind_textdomain_codeset, bindtextdomain, textdomain};
use tracing::warn;

const DOMAIN: &str = "ari-desktop";

/// Bind Ari's text domain. GTK sets the locale itself as it starts, before
/// any string here is looked up.
pub fn init() {
    let Some(dir) = locale_dir() else {
        warn!("can't find where Ari is installed; the window will be in English");
        return;
    };
    let bound = bindtextdomain(DOMAIN, dir)
        .and_then(|_| bind_textdomain_codeset(DOMAIN, "UTF-8"))
        .and_then(|_| textdomain(DOMAIN));
    if let Err(error) = bound {
        warn!(%error, "gettext refused the text domain; the window will be in English");
    }
}

/// `share/locale` beside the `bin` this runs from: `/usr` or `/app` once
/// installed, and `target/` in development, where `just run` puts the
/// compiled translations.
fn locale_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let prefix = exe.parent()?.parent()?;
    Some(prefix.join("share").join("locale"))
}
