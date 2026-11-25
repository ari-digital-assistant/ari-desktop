//! Preferences → "Start at Login", on by default.
//!
//! Ari runs in the background whatever this says; the setting only decides
//! whether it also starts when the user logs in. Inside Flatpak the Background
//! portal starts it, and outside Ari writes its own XDG autostart entry.
//! Hyprland reads neither, so there the user can have Ari add itself to their
//! Hyprland config (`hyprland`). Turning the setting off takes that out again,
//! since leaving it would start Ari at login against the switch.

use std::io;
use std::path::{Path, PathBuf};

use gettextrs::gettext;
use gtk4::gio::prelude::*;
use gtk4::{gio, glib};
use tracing::{info, warn};

use crate::{APP_ID, hyprland};

pub const KEY: &str = "start-at-login";

/// Make starting at login match the setting now, and whenever it changes.
pub fn follow(settings: &gio::Settings) {
    sync(settings.boolean(KEY));
    settings.connect_changed(Some(KEY), |settings, key| {
        let wanted = settings.boolean(key);
        sync(wanted);
        if !wanted {
            forget_hyprland();
        }
    });
}

fn sync(wanted: bool) {
    if ashpd::is_sandboxed() {
        // The portal may ask on screen, and nothing should wait for that.
        glib::spawn_future_local(ask_portal(wanted));
        return;
    }
    let result = match std::env::current_exe() {
        Ok(exe) => write_entry(&entry_path(), wanted.then_some(exe.as_path())),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        warn!(%error, wanted, "couldn't change Ari's autostart entry");
    }
}

fn forget_hyprland() {
    let Some(config) = hyprland::Config::find() else {
        return;
    };
    match config.remove_ari() {
        Ok(true) => info!(path = %config.path().display(), "took Ari out of the Hyprland config"),
        Ok(false) => {}
        Err(error) => {
            warn!(%error, path = %config.path().display(), "couldn't take Ari out of the Hyprland config")
        }
    }
}

/// Asked at every start rather than once: the portal answers with what it
/// granted, and asking again is how a changed setting reaches it.
async fn ask_portal(wanted: bool) {
    let reason = gettext("Ari keeps running so it's ready when you need it");
    let answer = ashpd::desktop::background::Background::request()
        .reason(reason.as_str())
        .auto_start(wanted)
        .command(["ari-gtk", "--background"])
        .dbus_activatable(false)
        .send()
        .await
        .and_then(|request| request.response());
    match answer {
        Ok(granted) => info!(
            background = granted.run_in_background(),
            at_login = granted.auto_start(),
            "the Background portal answered"
        ),
        Err(error) => warn!(%error, "the Background portal didn't answer"),
    }
}

fn entry_path() -> PathBuf {
    glib::user_config_dir()
        .join("autostart")
        .join(format!("{APP_ID}.desktop"))
}

/// Write the autostart entry for `exe`, or remove it when there is none.
/// Written at every start, so it follows the program if it moves.
fn write_entry(path: &Path, exe: Option<&Path>) -> io::Result<()> {
    let Some(exe) = exe else {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, entry(&exe.to_string_lossy()))
}

fn entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Ari\n\
         Exec={} --background\n\
         Icon={APP_ID}\n\
         NoDisplay=true\n",
        exec_argument(exe)
    )
}

/// Quote one argument of an `Exec` key, as the Desktop Entry spec has it:
/// double quotes around anything with a reserved character, a backslash
/// before `"`, `` ` ``, `$` and `\` inside them, then the string escape that
/// doubles every backslash again. `%` starts a field code, so it's doubled.
fn exec_argument(argument: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(',
        ')', '`',
    ];
    let argument = argument.replace('%', "%%");
    if !argument.contains(RESERVED) {
        return argument;
    }
    let mut quoted = String::from("\"");
    for c in argument.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_path_is_left_alone() {
        assert_eq!(exec_argument("/usr/bin/ari-gtk"), "/usr/bin/ari-gtk");
    }

    #[test]
    fn a_path_with_a_space_is_quoted() {
        assert_eq!(exec_argument("/home/a b/ari-gtk"), "\"/home/a b/ari-gtk\"");
    }

    #[test]
    fn quotes_dollars_and_backslashes_are_escaped_twice() {
        assert_eq!(exec_argument("/x/\"$\\"), r#""/x/\\"\\$\\\\""#);
    }

    #[test]
    fn a_percent_is_not_a_field_code() {
        assert_eq!(exec_argument("/opt/100%/ari-gtk"), "/opt/100%%/ari-gtk");
    }

    #[test]
    fn the_entry_starts_ari_without_a_window() {
        assert_eq!(
            entry("/usr/bin/ari-gtk"),
            "[Desktop Entry]\nType=Application\nName=Ari\nExec=/usr/bin/ari-gtk --background\nIcon=dev.heyari.Ari\nNoDisplay=true\n"
        );
    }

    #[test]
    fn the_entry_is_written_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autostart").join("dev.heyari.Ari.desktop");

        write_entry(&path, Some(Path::new("/usr/bin/ari-gtk"))).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            entry("/usr/bin/ari-gtk")
        );

        write_entry(&path, None).unwrap();
        assert!(!path.exists());

        // Removing what isn't there is not an error.
        write_entry(&path, None).unwrap();
    }
}
