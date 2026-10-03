use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::fs::{self, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SERVICE: &str = "hoist";

const EXPIRY_SKEW_SECS: u64 = 60;

fn env_var(provider: &str) -> String {
    format!("HOIST_{}_REFRESH_TOKEN", provider.to_uppercase())
}

pub fn keyring_entry(provider: &str) -> keyring::Result<Entry> {
    Entry::new(SERVICE, provider)
}

pub fn save_refresh_token(provider: &str, refresh_token: &str) -> Result<()> {
    match keyring_entry(provider).and_then(|e| e.set_password(refresh_token)) {
        Ok(()) => {
            forget_file_token(provider)?;
            println!("Saved to OS credential store.");
            Ok(())
        }
        Err(e) => {
            eprintln!("OS credential store unavailable ({e}).");
            let path = save_file_token(provider, refresh_token)?;
            eprintln!("Fell back to {} (0600).", path.display());
            Ok(())
        }
    }
}

pub fn forget_account(account_id: &str) -> Result<()> {
    if let Ok(entry) = keyring_entry(account_id) {
        let _ = entry.delete_credential();
    }
    forget_file_token(account_id)?;

    let path = cache_path()?;
    let mut cache: HashMap<String, CachedToken> = load_from(&path)?;
    if cache.remove(account_id).is_some() {
        save_to(&path, &cache)?;
    }

    Ok(())
}

pub fn get_refresh_token(provider: &str) -> Result<String> {
    match std::env::var(env_var(provider)) {
        Ok(token) if !token.is_empty() => return Ok(token),
        _ => {}
    }

    if let Ok(token) = keyring_entry(provider).and_then(|e| e.get_password()) {
        return Ok(token);
    }

    load_from::<HashMap<String, String>>(&tokens_path()?)?
        .remove(provider)
        .with_context(|| format!("not connected to {provider} - run `hoist account add` first"))
}

pub(crate) fn config_file(name: &str) -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/hoist").join(name))
}

fn tokens_path() -> Result<PathBuf> {
    config_file("tokens.json")
}

pub(crate) fn load_from<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn save_to<T: Serialize>(path: &Path, tokens: &T) -> Result<()> {
    let dir = path.parent().context("tokens path has no parent")?;
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, Permissions::from_mode(0o700))?;

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(serde_json::to_string_pretty(tokens)?.as_bytes())?;
    fs::set_permissions(path, Permissions::from_mode(0o600))?;

    Ok(())
}

fn save_file_token(provider: &str, refresh_token: &str) -> Result<PathBuf> {
    let path = tokens_path()?;
    let mut tokens: HashMap<String, String> = load_from(&path)?;
    tokens.insert(provider.to_string(), refresh_token.to_string());
    save_to(&path, &tokens)?;
    Ok(path)
}

fn forget_file_token(provider: &str) -> Result<()> {
    let path = tokens_path()?;
    let mut tokens: HashMap<String, String> = load_from(&path)?;
    if tokens.remove(provider).is_some() {
        save_to(&path, &tokens)?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct CachedToken {
    access_token: String,
    expires_at: u64,
}

fn cache_path() -> Result<PathBuf> {
    config_file("cache.json")
}

fn now_secs() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

pub fn get_cached_access_token(provider: &str) -> Result<Option<String>> {
    let cache: HashMap<String, CachedToken> = load_from(&cache_path()?)?;
    Ok(cache
        .get(provider)
        .filter(|entry| entry.expires_at > now_secs().unwrap_or(u64::MAX))
        .map(|entry| entry.access_token.clone()))
}

pub fn cache_access_token(provider: &str, access_token: &str, expires_in: u64) -> Result<()> {
    let path = cache_path()?;
    let mut cache: HashMap<String, CachedToken> = load_from(&path)?;
    cache.insert(
        provider.to_string(),
        CachedToken {
            access_token: access_token.to_string(),
            expires_at: now_secs()? + expires_in.saturating_sub(EXPIRY_SKEW_SECS),
        },
    );
    save_to(&path, &cache)
}
