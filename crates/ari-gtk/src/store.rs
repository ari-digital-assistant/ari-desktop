//! Ari's own files: the conversation log and the remembered facts, as JSON
//! under the user's data directory.

use std::io;
use std::path::{Path, PathBuf};

use gtk4::glib;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::warn;

pub fn dir() -> PathBuf {
    glib::user_data_dir().join("ari")
}

/// What `path` holds, or the default when there's nothing there yet. A file
/// that doesn't read is moved aside to `<path>.unreadable` rather than left
/// for the next save to overwrite.
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    let error = match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => return value,
            Err(error) => io::Error::new(io::ErrorKind::InvalidData, error),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => return T::default(),
        Err(error) => error,
    };
    let aside = path.with_extension("json.unreadable");
    warn!(%error, path = %path.display(), aside = %aside.display(), "couldn't read this, so it's set aside and Ari starts without it");
    if let Err(error) = std::fs::rename(path, &aside) {
        warn!(%error, "couldn't set it aside");
    }
    T::default()
}

/// Write `value` to `path` whole or not at all: a crash halfway leaves the
/// last version.
pub fn save<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, serde_json::to_vec(value)?)?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_saved_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("facts.json");
        save(&path, &vec!["my name is Keith".to_string()]).unwrap();
        assert_eq!(load::<Vec<String>>(&path), vec!["my name is Keith"]);
    }

    #[test]
    fn nothing_there_yet_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load::<Vec<String>>(&dir.path().join("facts.json")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_unreadable_file_is_set_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.json");
        std::fs::write(&path, "not json").unwrap();

        assert_eq!(load::<Vec<String>>(&path), Vec::<String>::new());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("facts.json.unreadable")).unwrap(),
            "not json"
        );
    }
}
