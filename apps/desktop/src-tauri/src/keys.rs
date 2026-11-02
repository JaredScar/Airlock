//! OS keychain entries. Plaintext terms and cloud keys are never written to disk.

use airlock_pipeline::{open_terms, random_key, seal_terms};
use keyring::Entry;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const SERVICE: &str = "airlock";

pub fn persist_terms(app: &AppHandle, terms: &[String]) -> Result<(), String> {
    let blob = seal_terms(terms, &term_key()?).map_err(|e| e.to_string())?;
    std::fs::write(seal_path(app)?, blob).map_err(|e| e.to_string())
}

pub fn load_terms(app: &AppHandle) -> Result<Option<Vec<String>>, String> {
    let path = seal_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let blob = std::fs::read(path).map_err(|e| e.to_string())?;
    open_terms(&blob, &term_key()?)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn store_stream_key(key_ref: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key_ref.trim().is_empty() || key.is_empty() {
        return Err("stream key is empty".into());
    }
    let entry =
        Entry::new(SERVICE, key_ref).map_err(|_| "could not open the keychain".to_string())?;
    entry
        .set_password(key)
        .map_err(|_| "could not store the stream key".to_string())
}

pub fn load_stream_key(key_ref: &str) -> Result<String, String> {
    let entry =
        Entry::new(SERVICE, key_ref).map_err(|_| "could not open the keychain".to_string())?;
    entry
        .get_password()
        .map_err(|_| "could not read the stream key".to_string())
}

/// True when a key is stored. A missing entry is not an error, and the key is not returned.
pub fn stream_key_is_saved(key_ref: &str) -> Result<bool, String> {
    let entry =
        Entry::new(SERVICE, key_ref).map_err(|_| "could not open the keychain".to_string())?;
    match entry.get_password() {
        Ok(key) => Ok(!key.trim().is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err("could not read the stream key".into()),
    }
}

pub fn store_speech_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("speech key is empty".into());
    }
    let entry =
        Entry::new(SERVICE, "deepgram").map_err(|_| "could not open the keychain".to_string())?;
    entry
        .set_password(key)
        .map_err(|_| "could not store the speech key".to_string())
}

pub fn load_speech_key() -> Result<String, String> {
    let entry =
        Entry::new(SERVICE, "deepgram").map_err(|_| "could not open the keychain".to_string())?;
    let key = entry
        .get_password()
        .map_err(|_| "could not read the speech key".to_string())?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("speech key is empty".into());
    }
    Ok(key)
}

pub fn load_cloud_key() -> Result<String, String> {
    let entry =
        Entry::new(SERVICE, "anthropic").map_err(|_| "could not open the keychain".to_string())?;
    let key = entry
        .get_password()
        .map_err(|_| "could not read the cloud judge key".to_string())?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("cloud judge key is empty".into());
    }
    Ok(key)
}

pub fn store_cloud_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("cloud judge key is empty".into());
    }
    let entry =
        Entry::new(SERVICE, "anthropic").map_err(|_| "could not open the keychain".to_string())?;
    entry
        .set_password(key)
        .map_err(|_| "could not store the cloud judge key".to_string())
}

pub fn store_obs_password(password: &str) -> Result<(), String> {
    let password = password.trim();
    if password.is_empty() {
        return Err("OBS websocket password is empty".into());
    }
    let entry =
        Entry::new(SERVICE, "obs").map_err(|_| "could not open the keychain".to_string())?;
    entry
        .set_password(password)
        .map_err(|_| "could not store the OBS websocket password".to_string())
}

pub fn load_obs_password() -> Result<String, String> {
    let entry =
        Entry::new(SERVICE, "obs").map_err(|_| "could not open the keychain".to_string())?;
    let password = entry
        .get_password()
        .map_err(|_| "could not read the OBS websocket password".to_string())?;
    let password = password.trim().to_string();
    if password.is_empty() {
        return Err("OBS websocket password is empty".into());
    }
    Ok(password)
}

pub fn obs_password_is_saved() -> Result<bool, String> {
    let entry =
        Entry::new(SERVICE, "obs").map_err(|_| "could not open the keychain".to_string())?;
    match entry.get_password() {
        Ok(password) => Ok(!password.trim().is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err("could not read the OBS websocket password".into()),
    }
}

fn term_key() -> Result<[u8; 32], String> {
    let entry = Entry::new(SERVICE, "private-terms")
        .map_err(|_| "could not open the keychain".to_string())?;
    match entry.get_password() {
        Ok(stored) => decode_key(&stored),
        Err(keyring::Error::NoEntry) => {
            let key = random_key();
            entry
                .set_password(&encode_key(&key))
                .map_err(|_| "could not store the term key".to_string())?;
            Ok(key)
        }
        Err(_) => Err("could not read the term key".into()),
    }
}

fn seal_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("private-terms.seal"))
}

fn encode_key(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_key(stored: &str) -> Result<[u8; 32], String> {
    if stored.len() != 64 {
        return Err("term key is unreadable".into());
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&stored[i * 2..i * 2 + 2], 16)
            .map_err(|_| "term key is unreadable".to_string())?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_stream_key_is_refused() {
        if let Ok(entry) = Entry::new(SERVICE, "airlock-test-stream") {
            let _ = entry.delete_credential();
        }
        let err = store_stream_key("airlock/twitch", " ").unwrap_err();
        assert_eq!(err, "stream key is empty");
    }

    #[test]
    fn a_saved_stream_key_is_readable_from_a_new_entry() {
        let account = "airlock-test-roundtrip";
        if let Ok(entry) = Entry::new(SERVICE, account) {
            let _ = entry.delete_credential();
        }
        store_stream_key(account, "round-trip-check").expect("store");
        let read = load_stream_key(account);
        if let Ok(entry) = Entry::new(SERVICE, account) {
            let _ = entry.delete_credential();
        }
        assert_eq!(
            read.expect("a new entry can read the saved key"),
            "round-trip-check"
        );
    }
}
