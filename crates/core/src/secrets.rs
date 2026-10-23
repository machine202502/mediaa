use crate::agent::AgentSettings;
use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "MediaA";

pub trait Store: Send {
    fn get(&self, id: &str) -> anyhow::Result<Option<String>>;
    fn set(&mut self, id: &str, key: &str) -> anyhow::Result<()>;
    fn delete(&mut self, id: &str) -> anyhow::Result<()>;
}

pub struct Keychain;

fn entry(id: &str) -> anyhow::Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, &format!("provider-{id}"))?)
}

impl Store for Keychain {
    fn get(&self, id: &str) -> anyhow::Result<Option<String>> {
        match entry(id)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&mut self, id: &str, key: &str) -> anyhow::Result<()> {
        Ok(entry(id)?.set_password(key)?)
    }

    fn delete(&mut self, id: &str) -> anyhow::Result<()> {
        match entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

pub struct Keys<S: Store> {
    store: S,
    known: HashMap<String, String>,
}

impl<S: Store> Keys<S> {
    pub fn new(store: S) -> Keys<S> {
        Keys { store, known: HashMap::new() }
    }

    pub fn fill(&mut self, entries: Vec<(String, &mut String)>) {
        for (id, key) in entries {
            match self.store.get(&id) {
                Ok(stored) => {
                    *key = stored.unwrap_or_default();
                    if !key.is_empty() {
                        self.known.insert(id, key.clone());
                    }
                }
                Err(e) => log::warn!("keychain: reading the key of {id}: {e:#}"),
            }
        }
    }

    pub fn sync(&mut self, entries: &[(String, String)]) -> anyhow::Result<()> {
        let now: HashMap<&str, &str> = entries.iter().filter(|(_, k)| !k.is_empty()).map(|(i, k)| (i.as_str(), k.as_str())).collect();
        let gone: Vec<String> = self.known.keys().filter(|id| !now.contains_key(id.as_str())).cloned().collect();
        for id in gone {
            self.store.delete(&id)?;
            self.known.remove(&id);
        }
        for (id, key) in now {
            if self.known.get(id).map(String::as_str) != Some(key) {
                self.store.set(id, key)?;
                self.known.insert(id.to_string(), key.to_string());
            }
        }
        Ok(())
    }
}

static KEYCHAIN: Mutex<Option<Keys<Keychain>>> = Mutex::new(None);

fn with_keychain<R>(f: impl FnOnce(&mut Keys<Keychain>) -> R) -> R {
    let mut guard = KEYCHAIN.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(|| Keys::new(Keychain)))
}

const WEB_KEY: &str = "web-search";

fn entries_mut(agent: &mut AgentSettings) -> Vec<(String, &mut String)> {
    let mut e: Vec<(String, &mut String)> = agent.providers.iter_mut().map(|p| (p.id.clone(), &mut p.api_key)).collect();
    e.push((WEB_KEY.into(), &mut agent.web.api_key));
    e
}

fn entries(agent: &AgentSettings) -> Vec<(String, String)> {
    let mut e: Vec<(String, String)> = agent.providers.iter().map(|p| (p.id.clone(), p.api_key.clone())).collect();
    e.push((WEB_KEY.into(), agent.web.api_key.clone()));
    e
}

pub fn load(agent: &mut AgentSettings) {
    with_keychain(|k| k.fill(entries_mut(agent)));
}

pub fn save(agent: &AgentSettings) -> anyhow::Result<()> {
    with_keychain(|k| k.sync(&entries(agent)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Provider, ProviderKind};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Memory(Arc<Mutex<(HashMap<String, String>, usize)>>);

    impl Store for Memory {
        fn get(&self, id: &str) -> anyhow::Result<Option<String>> {
            Ok(self.0.lock().unwrap().0.get(id).cloned())
        }
        fn set(&mut self, id: &str, key: &str) -> anyhow::Result<()> {
            let mut m = self.0.lock().unwrap();
            m.0.insert(id.into(), key.into());
            m.1 += 1;
            Ok(())
        }
        fn delete(&mut self, id: &str) -> anyhow::Result<()> {
            let mut m = self.0.lock().unwrap();
            m.0.remove(id);
            m.1 += 1;
            Ok(())
        }
    }

    #[test]
    fn keys_round_trip_and_follow_providers() {
        let memory = Memory::default();
        let mut agent = AgentSettings::default();
        let mut a = Provider::new(ProviderKind::OpenRouter, "a");
        a.api_key = "sk-a".into();
        let mut b = Provider::new(ProviderKind::HuggingFace, "b");
        b.id.push('b');
        b.api_key = "hf-b".into();
        agent.providers = vec![a, b];
        agent.web.api_key = "brave-key".into();
        let mut keys = Keys::new(memory.clone());
        keys.sync(&entries(&agent)).unwrap();
        assert_eq!(memory.0.lock().unwrap().1, 3);
        keys.sync(&entries(&agent)).unwrap();
        assert_eq!(memory.0.lock().unwrap().1, 3, "unchanged keys are not rewritten");

        let mut loaded = agent.clone();
        loaded.providers.iter_mut().for_each(|p| p.api_key.clear());
        loaded.web.api_key.clear();
        let mut keys = Keys::new(memory.clone());
        keys.fill(entries_mut(&mut loaded));
        assert_eq!(loaded, agent);

        loaded.providers.truncate(1);
        loaded.providers[0].api_key.clear();
        loaded.web.api_key.clear();
        keys.sync(&entries(&loaded)).unwrap();
        assert!(memory.0.lock().unwrap().0.is_empty());
    }

    #[test]
    #[ignore]
    fn os_keychain() {
        let id = format!("test-{}", std::process::id());
        let mut k = Keychain;
        k.set(&id, "sk-test").unwrap();
        assert_eq!(k.get(&id).unwrap().as_deref(), Some("sk-test"));
        k.delete(&id).unwrap();
        assert_eq!(k.get(&id).unwrap(), None);
        k.delete(&id).unwrap();
    }

    #[test]
    fn keys_are_not_in_the_settings_file() {
        let mut p = Provider::new(ProviderKind::OpenRouter, "x");
        p.api_key = "sk-secret".into();
        let text = toml::to_string(&p).unwrap();
        assert!(!text.contains("sk-secret"));
    }
}
