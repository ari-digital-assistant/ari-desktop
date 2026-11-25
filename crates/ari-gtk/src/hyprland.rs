//! Starting at login on Hyprland, which runs no XDG autostart entries: what
//! starts with it is whatever its own config starts. Ari adds itself only when
//! the user asks, between markers, so it can find its lines and take them out
//! again and never touches the rest of the file.
//!
//! Hyprland 0.55 moved its config to Lua (`hyprland.lua`) and still reads the
//! older `hyprland.conf` when there's no Lua file, so both are handled.

use std::io;
use std::path::{Path, PathBuf};

use gtk4::glib;

use crate::APP_ID;

/// Whether this session is Hyprland's.
pub fn running() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Lua,
    Hyprlang,
}

#[derive(Debug)]
pub struct Config {
    path: PathBuf,
    syntax: Syntax,
}

impl Config {
    /// The config Hyprland reads, if there is one.
    pub fn find() -> Option<Config> {
        let dir = config_home().join("hypr");
        [
            ("hyprland.lua", Syntax::Lua),
            ("hyprland.conf", Syntax::Hyprlang),
        ]
        .into_iter()
        .map(|(name, syntax)| Config {
            path: dir.join(name),
            syntax,
        })
        .find(|config| config.path.is_file())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn has_ari(&self) -> io::Result<bool> {
        let text = std::fs::read_to_string(&self.path)?;
        Ok(find_block(&text, self.syntax).is_some())
    }

    pub fn add_ari(&self) -> io::Result<()> {
        let text = std::fs::read_to_string(&self.path)?;
        if find_block(&text, self.syntax).is_some() {
            return Ok(());
        }
        replace(&self.path, &with_block(&text, self.syntax, &command()?))
    }

    /// Take Ari's lines out. Returns whether there were any.
    pub fn remove_ari(&self) -> io::Result<bool> {
        let text = std::fs::read_to_string(&self.path)?;
        match without_block(&text, self.syntax)? {
            Some(text) => replace(&self.path, &text).map(|()| true),
            None => Ok(false),
        }
    }
}

/// Inside Flatpak `XDG_CONFIG_HOME` is the app's own; the user's is the host's.
fn config_home() -> PathBuf {
    if ashpd::is_sandboxed() {
        std::env::var_os("HOST_XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| glib::home_dir().join(".config"))
    } else {
        glib::user_config_dir()
    }
}

/// The shell command that starts Ari without a window.
fn command() -> io::Result<String> {
    if ashpd::is_sandboxed() {
        return Ok(format!("flatpak run {APP_ID} --background"));
    }
    let exe = std::env::current_exe()?;
    Ok(format!(
        "{} --background",
        shell_quote(&exe.to_string_lossy())
    ))
}

/// Write `text` over the file at `path` in one step, since Hyprland reloads
/// its config the moment the file changes and must never read half of it.
/// Through a symlink to the file it points at, keeping dotfile links intact.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let target = std::fs::canonicalize(path)?;
    let permissions = std::fs::metadata(&target)?.permissions();
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::other("the config path has no file name"))?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(".ari");
    let temporary = target.with_file_name(temporary_name);
    std::fs::write(&temporary, text)?;
    std::fs::set_permissions(&temporary, permissions)?;
    std::fs::rename(&temporary, &target)
}

fn markers(syntax: Syntax) -> (&'static str, &'static str) {
    match syntax {
        Syntax::Lua => (
            "-- >>> Ari: start at login. Ari's Preferences add and remove this.",
            "-- <<< Ari",
        ),
        Syntax::Hyprlang => (
            "# >>> Ari: start at login. Ari's Preferences add and remove this.",
            "# <<< Ari",
        ),
    }
}

fn statement(syntax: Syntax, command: &str) -> String {
    match syntax {
        Syntax::Lua => format!(
            "hl.on(\"hyprland.start\", function() hl.exec_cmd(\"{}\") end)",
            lua_escape(command)
        ),
        // A `#` starts a comment in hyprlang; `##` is a literal one.
        Syntax::Hyprlang => format!("exec-once = {}", command.replace('#', "##")),
    }
}

/// Line indexes of the begin marker and, if present, the end marker after it.
fn find_block(text: &str, syntax: Syntax) -> Option<(usize, Option<usize>)> {
    let (begin, end) = markers(syntax);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|line| line.trim_end() == begin)?;
    let finish = lines[start..]
        .iter()
        .position(|line| line.trim_end() == end)
        .map(|offset| start + offset);
    Some((start, finish))
}

fn with_block(text: &str, syntax: Syntax, command: &str) -> String {
    let (begin, end) = markers(syntax);
    let mut out = text.to_owned();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(begin);
    out.push('\n');
    out.push_str(&statement(syntax, command));
    out.push('\n');
    out.push_str(end);
    out.push('\n');
    out
}

/// The text with Ari's block taken out, and the blank line `with_block` put
/// before it; `None` when there's no block.
fn without_block(text: &str, syntax: Syntax) -> io::Result<Option<String>> {
    let Some((start, finish)) = find_block(text, syntax) else {
        return Ok(None);
    };
    let Some(finish) = finish else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Ari's lines in the Hyprland config have lost their end marker",
        ));
    };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let first = if start > 0 && lines[start - 1].trim().is_empty() {
        start - 1
    } else {
        start
    };
    let mut out = String::with_capacity(text.len());
    for (index, line) in lines.iter().enumerate() {
        if !(first..=finish).contains(&index) {
            out.push_str(line);
        }
    }
    Ok(Some(out))
}

/// Quote for a POSIX shell when the path needs it.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

fn lua_escape(text: &str) -> String {
    text.replace('\\', r"\\")
        .replace('"', "\\\"")
        .replace('\n', r"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUA_BLOCK: &str = "-- >>> Ari: start at login. Ari's Preferences add and remove this.\n\
        hl.on(\"hyprland.start\", function() hl.exec_cmd(\"/usr/bin/ari-gtk --background\") end)\n\
        -- <<< Ari\n";

    #[test]
    fn the_lua_block_is_appended_after_a_blank_line() {
        let text = "require(\"conf.autostart\")\n";
        assert_eq!(
            with_block(text, Syntax::Lua, "/usr/bin/ari-gtk --background"),
            format!("require(\"conf.autostart\")\n\n{LUA_BLOCK}")
        );
    }

    #[test]
    fn a_file_without_a_final_newline_gets_one() {
        assert_eq!(
            with_block("a = 1", Syntax::Lua, "/usr/bin/ari-gtk --background"),
            format!("a = 1\n\n{LUA_BLOCK}")
        );
    }

    #[test]
    fn hyprlang_gets_exec_once_with_hashes_doubled() {
        assert_eq!(
            with_block("", Syntax::Hyprlang, "'/opt/#1/ari-gtk' --background"),
            "# >>> Ari: start at login. Ari's Preferences add and remove this.\n\
             exec-once = '/opt/##1/ari-gtk' --background\n\
             # <<< Ari\n"
        );
    }

    #[test]
    fn removing_restores_the_file_exactly() {
        for original in ["", "monitor = ,preferred,auto,1\n", "a\n\nb\n"] {
            for syntax in [Syntax::Lua, Syntax::Hyprlang] {
                let added = with_block(original, syntax, "ari-gtk --background");
                assert_eq!(
                    without_block(&added, syntax).unwrap().as_deref(),
                    Some(original),
                    "{syntax:?} {original:?}"
                );
            }
        }
    }

    #[test]
    fn the_block_is_removed_from_the_middle_of_the_file() {
        let text = format!("before\n\n{LUA_BLOCK}after\n");
        assert_eq!(
            without_block(&text, Syntax::Lua).unwrap().as_deref(),
            Some("before\nafter\n")
        );
    }

    #[test]
    fn no_block_means_nothing_to_remove() {
        assert_eq!(without_block("a = 1\n", Syntax::Lua).unwrap(), None);
        assert!(find_block("a = 1\n", Syntax::Lua).is_none());
    }

    #[test]
    fn a_block_without_its_end_marker_is_left_alone() {
        let text = "-- >>> Ari: start at login. Ari's Preferences add and remove this.\nhl.on()\n";
        assert_eq!(
            without_block(text, Syntax::Lua).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn the_other_syntax_markers_are_not_ours() {
        let added = with_block("", Syntax::Hyprlang, "ari-gtk --background");
        assert!(find_block(&added, Syntax::Lua).is_none());
    }

    #[test]
    fn shell_quoting_only_when_needed() {
        assert_eq!(shell_quote("/usr/bin/ari-gtk"), "/usr/bin/ari-gtk");
        assert_eq!(shell_quote("/home/a b/ari"), "'/home/a b/ari'");
        assert_eq!(shell_quote("/it's/ari"), r"'/it'\''s/ari'");
    }

    #[test]
    fn lua_strings_are_escaped() {
        assert_eq!(
            statement(Syntax::Lua, r#"'/a "b"\c' --background"#),
            r#"hl.on("hyprland.start", function() hl.exec_cmd("'/a \"b\"\\c' --background") end)"#
        );
    }

    #[test]
    fn writing_goes_through_a_symlink_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-hyprland.lua");
        std::fs::write(&real, "a = 1\n").unwrap();
        let link = dir.path().join("hyprland.lua");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        replace(&link, "a = 2\n").unwrap();

        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "a = 2\n");
    }
}
