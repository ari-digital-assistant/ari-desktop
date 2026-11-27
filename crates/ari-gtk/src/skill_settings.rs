//! Skills' settings kept between runs. The engine's [`SettingsStore`] only
//! holds them in memory, so every value is written here too and put back at
//! start: plain ones in a JSON file, secrets (API keys) in the keyring.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ari_runtime::settings::SettingsStore;
use ari_skill_loader::SettingWriter;
use tokio::runtime::Handle;
use tokio::sync::OnceCell;
use tracing::{info, warn};

use crate::{APP_ID, store};

/// skill id → key → value.
type Values = BTreeMap<String, BTreeMap<String, String>>;

pub struct SkillSettings {
    store: Arc<SettingsStore>,
    path: PathBuf,
    plain: Mutex<Values>,
    runtime: Handle,
    keyring: OnceCell<oo7::Keyring>,
}

impl SkillSettings {
    /// The plain values saved at `path`, put into `store` at once. Secrets
    /// follow with [`Self::load_secrets`]. `runtime` runs the keyring calls.
    pub fn open(store: Arc<SettingsStore>, path: PathBuf, runtime: Handle) -> Arc<Self> {
        let plain: Values = store::load(&path);
        for (skill, values) in &plain {
            for (key, value) in values {
                store.set(skill, key, value);
            }
        }
        Arc::new(Self {
            store,
            path,
            plain: Mutex::new(plain),
            runtime,
            keyring: OnceCell::new(),
        })
    }

    /// Put every secret in the keyring into the engine's store.
    pub async fn load_secrets(self: &Arc<Self>) {
        let this = self.clone();
        let loaded = self
            .runtime
            .spawn(async move { this.read_secrets().await })
            .await;
        match loaded {
            Ok(Ok(count)) => info!(count, "skill secrets loaded from the keyring"),
            Ok(Err(error)) => warn!(%error, "couldn't read skill secrets from the keyring"),
            Err(error) => warn!(%error, "reading the keyring stopped"),
        }
    }

    async fn read_secrets(&self) -> oo7::Result<usize> {
        let keyring = self.keyring().await?;
        keyring.unlock().await?;
        let items = keyring
            .search_items(&HashMap::from([("application", APP_ID)]))
            .await?;
        let mut count = 0;
        for item in items {
            let attributes = item.attributes().await?;
            let (Some(skill), Some(key)) = (attributes.get("skill"), attributes.get("key")) else {
                continue;
            };
            let secret = item.secret().await?;
            self.store
                .set(skill, key, &String::from_utf8_lossy(secret.as_bytes()));
            count += 1;
        }
        Ok(count)
    }

    async fn keyring(&self) -> oo7::Result<&oo7::Keyring> {
        self.keyring.get_or_try_init(oo7::Keyring::new).await
    }

    /// `key` of skill `skill`, secret or not, once [`Self::load_secrets`] has
    /// run for the secrets.
    pub fn get(&self, skill: &str, key: &str) -> Option<String> {
        self.store.get_value(skill, key)
    }

    /// Set `key` of skill `skill` for the engine and keep it. An empty
    /// secret is taken out of the keyring.
    pub async fn set(
        self: &Arc<Self>,
        skill: &str,
        key: &str,
        value: &str,
        secret: bool,
    ) -> Result<(), String> {
        self.store.set(skill, key, value);
        if !secret {
            return self.keep_plain(skill, key, value);
        }
        let this = self.clone();
        let (skill, key, value) = (skill.to_string(), key.to_string(), value.to_string());
        self.runtime
            .spawn(async move { this.keep_secret(&skill, &key, &value).await })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
    }

    fn keep_plain(&self, skill: &str, key: &str, value: &str) -> Result<(), String> {
        let mut plain = self.plain.lock().expect("skill settings lock poisoned");
        plain
            .entry(skill.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
        store::save(&self.path, &*plain).map_err(|error| error.to_string())
    }

    async fn keep_secret(&self, skill: &str, key: &str, value: &str) -> oo7::Result<()> {
        let keyring = self.keyring().await?;
        keyring.unlock().await?;
        let attributes = HashMap::from([("application", APP_ID), ("skill", skill), ("key", key)]);
        if value.is_empty() {
            return keyring.delete(&attributes).await;
        }
        keyring
            .create_item(
                &format!("Ari: {skill} {key}"),
                &attributes,
                oo7::Secret::text(value),
                true,
            )
            .await
    }
}

/// What a skill's `ari::setting_set` calls, on the engine's thread.
impl SettingWriter for SkillSettings {
    fn set_value(&self, skill_id: &str, key: &str, value: &str, is_secret: bool) -> bool {
        self.store.set(skill_id, key, value);
        let kept = if is_secret {
            self.runtime
                .block_on(self.keep_secret(skill_id, key, value))
                .map_err(|error| error.to_string())
        } else {
            self.keep_plain(skill_id, key, value)
        };
        if let Err(error) = &kept {
            warn!(%error, skill_id, key, "couldn't keep a skill's setting");
        }
        kept.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Plain values only: the keyring isn't touched, so no runtime runs.
    fn settings(dir: &tempfile::TempDir, store: Arc<SettingsStore>) -> Arc<SkillSettings> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        SkillSettings::open(
            store,
            dir.path().join("skill-settings.json"),
            runtime.handle().clone(),
        )
    }

    #[test]
    fn a_plain_value_is_kept_and_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let first = Arc::new(SettingsStore::new());
        let kept = settings(&dir, first.clone());
        assert!(kept.set_value("dev.heyari.chatgpt", "tier", "smartest", false));
        assert_eq!(
            first.get_value("dev.heyari.chatgpt", "tier").as_deref(),
            Some("smartest")
        );

        let again = Arc::new(SettingsStore::new());
        settings(&dir, again.clone());
        assert_eq!(
            again.get_value("dev.heyari.chatgpt", "tier").as_deref(),
            Some("smartest")
        );
    }

    #[test]
    fn a_changed_value_replaces_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let kept = settings(&dir, Arc::new(SettingsStore::new()));
        kept.set_value("dev.heyari.chatgpt", "tier", "fast", false);
        kept.set_value("dev.heyari.chatgpt", "tier", "balanced", false);
        kept.set_value("dev.heyari.weather", "units", "metric", false);

        let again = Arc::new(SettingsStore::new());
        settings(&dir, again.clone());
        assert_eq!(
            again.get_value("dev.heyari.chatgpt", "tier").as_deref(),
            Some("balanced")
        );
        assert_eq!(
            again.get_value("dev.heyari.weather", "units").as_deref(),
            Some("metric")
        );
    }
}
