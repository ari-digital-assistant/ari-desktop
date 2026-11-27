//! The on-device speech models on this computer: where each is kept, whether
//! it's all there, and downloading it by its manifest, resuming a download
//! that stopped and checking each file against the manifest's checksum.
//! Which models there are, and which serves which language, is the engine's
//! (`ari_voice::stt_models`).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ari_core::model_manifest::ModelManifest;
use ari_voice::stt_models::SttModel;
use gettextrs::gettext;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::store;

/// Where `model` is kept.
pub fn dir(model: &SttModel) -> PathBuf {
    store::dir().join("models").join(model.id)
}

/// Every file of `model` is in `dir`.
pub fn downloaded(model: &SttModel, dir: &Path) -> bool {
    model.files().iter().all(|file| dir.join(file).is_file())
}

/// What `model` is called and what it is, in the user's words.
pub fn describe(model: &SttModel) -> (String, String) {
    match model.id {
        "kroko-2025-08-06" => (
            gettext("English speech model"),
            gettext("Kroko, 71 MB. Works offline."),
        ),
        "parakeet-ultra-int8-2026-11" => (
            gettext("European speech model"),
            gettext(
                "Parakeet Ultra, 671 MB. Works offline, 25 European languages. Your words appear once you finish speaking.",
            ),
        ),
        "whisper-turbo-int8-2024-09" => (
            gettext("Multilingual speech model"),
            gettext("Whisper Turbo, 1 GB. Works offline, 99 languages."),
        ),
        other => (other.to_string(), String::new()),
    }
}

/// One file to fetch.
struct Fetch {
    name: String,
    url: String,
    /// None without a manifest, when the files come straight from their
    /// source and can't be checked.
    sha256: Option<String>,
    size: Option<u64>,
}

/// Download `model` into `dir`, telling `progress` the bytes done of the
/// whole. Files already there are kept, and a file part-fetched before is
/// resumed. Blocking: run it off the main loop.
pub fn download(model: &SttModel, dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // A gigabyte takes as long as it takes.
        .timeout(None)
        .build()
        .map_err(|e| e.to_string())?;
    let fetches = match manifest(&client, model.manifest_url) {
        Ok(manifest) => manifest
            .files
            .into_iter()
            .map(|f| Fetch {
                name: f.name,
                url: f.url,
                sha256: Some(f.sha256),
                size: Some(f.size_bytes),
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            warn!(
                error,
                model = model.id,
                "no manifest, so the files go unchecked"
            );
            model
                .files()
                .into_iter()
                .map(|name| Fetch {
                    name: name.to_string(),
                    url: format!("{}/{name}", model.base_url),
                    sha256: None,
                    size: None,
                })
                .collect()
        }
    };
    let total = fetches
        .iter()
        .map(|f| f.size)
        .sum::<Option<u64>>()
        .unwrap_or(model.total_bytes);
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut done = 0;
    for fetch in &fetches {
        let path = dir.join(&fetch.name);
        if !path.is_file() {
            fetch_file(&client, fetch, &path, &mut |bytes| {
                progress(done + bytes, total)
            })?;
        }
        done += fs::metadata(&path).map(|m| m.len()).unwrap_or_default();
        progress(done, total);
    }
    Ok(())
}

fn manifest(client: &reqwest::blocking::Client, url: &str) -> Result<ModelManifest, String> {
    let text = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::text)
        .map_err(|e| e.to_string())?;
    ModelManifest::parse(&text).map_err(|e| e.to_string())
}

/// Fetch `fetch` into `path`, by way of `path.part`, which a stopped
/// download is resumed from.
fn fetch_file(
    client: &reqwest::blocking::Client,
    fetch: &Fetch,
    path: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let part = path.with_extension(path.extension().map_or("part".into(), |ext| {
        format!("{}.part", ext.to_string_lossy())
    }));
    let have = fs::metadata(&part).map(|m| m.len()).unwrap_or_default();
    let mut request = client.get(&fetch.url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut response = request
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("{}: {e}", fetch.name))?;
    // A server that ignores the range sends the whole file again.
    let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)
        .map_err(|e| e.to_string())?;
    let mut done = if resumed { have } else { 0 };
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", fetch.name))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        done += read as u64;
        progress(done);
    }
    file.flush().map_err(|e| e.to_string())?;
    if let Some(size) = fetch.size
        && done != size
    {
        return Err(format!("{}: got {done} bytes of {size}", fetch.name));
    }
    if let Some(expected) = &fetch.sha256 {
        let actual = sha256(&part)?;
        if !actual.eq_ignore_ascii_case(expected) {
            // Start it again next time rather than resume a bad file.
            let _ = fs::remove_file(&part);
            return Err(format!("{} didn't match its checksum", fetch.name));
        }
    }
    fs::rename(&part, path).map_err(|e| e.to_string())
}

fn sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ari_voice::stt_models::{KROKO, WHISPER_TURBO};

    #[test]
    fn a_model_is_downloaded_once_every_file_is_there() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["encoder.onnx", "decoder.onnx", "joiner.onnx"] {
            fs::write(dir.path().join(file), b"x").unwrap();
        }
        assert!(!downloaded(&KROKO, dir.path()));
        fs::write(dir.path().join("tokens.txt"), b"x").unwrap();
        assert!(downloaded(&KROKO, dir.path()));
        assert!(!downloaded(&WHISPER_TURBO, dir.path()));
    }

    #[test]
    fn a_file_s_checksum_is_its_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.txt");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
