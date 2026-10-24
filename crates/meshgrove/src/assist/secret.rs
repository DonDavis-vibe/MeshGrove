//! The endpoint's key: in the system keyring (Secret Service, Keychain, Credential Manager),
//! else in an environment variable the user names, else only for this session.

use std::sync::Mutex;

const SERVICE: &str = "meshgrove";

/// A key typed in this session when no keyring is available.
static SESSION: Mutex<Option<(String, String)>> = Mutex::new(None);

fn account(base_url: &str) -> String {
    // Per endpoint host, so keys for OpenAI and OpenRouter do not mix.
    let host = base_url.split("://").nth(1).unwrap_or(base_url).split('/').next().unwrap_or_default();
    format!("endpoint:{host}")
}

/// Stores the key; `Ok(false)` when there is no keyring and it is kept for this session only.
pub fn store(base_url: &str, key: &str) -> Result<bool, String> {
    let account = account(base_url);
    match keyring::Entry::new(SERVICE, &account).and_then(|e| e.set_password(key)) {
        Ok(()) => Ok(true),
        Err(e) => {
            log::warn!("no keyring for the endpoint key: {e}");
            *SESSION.lock().unwrap() = Some((account, key.to_owned()));
            Ok(false)
        }
    }
}

pub fn forget(base_url: &str) {
    let account = account(base_url);
    if let Ok(entry) = keyring::Entry::new(SERVICE, &account) {
        let _ = entry.delete_credential();
    }
    let mut session = SESSION.lock().unwrap();
    if session.as_ref().is_some_and(|(a, _)| *a == account) {
        *session = None;
    }
}

/// The key for the endpoint: keyring (when kept there), the environment variable, the session.
pub fn load(base_url: &str, in_keyring: bool, env: &str) -> Option<String> {
    let account = account(base_url);
    if in_keyring && let Ok(key) = keyring::Entry::new(SERVICE, &account).and_then(|e| e.get_password()) {
        return Some(key);
    }
    if !env.trim().is_empty()
        && let Ok(key) = std::env::var(env.trim())
    {
        return Some(key);
    }
    SESSION.lock().unwrap().as_ref().filter(|(a, _)| *a == account).map(|(_, k)| k.clone())
}
