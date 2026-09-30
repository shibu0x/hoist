use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;


use super::token_store::{config_file, load_from, save_to};

const FILE: &str = "accounts.json";

#[derive(Serialize, Deserialize, Clone)]
pub struct Account {
    pub provider: String,
    pub email: String,
}

impl Account {
    pub fn id(&self) -> String {
        format!("{}:{}", self.provider, self.email)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Accounts {
    pub active: Option<String>,
    pub known: BTreeMap<String, Account>,
}

pub fn list() -> Result<Accounts> {
    load_from(&config_file(FILE)?)
}

fn store(accounts: &Accounts) -> Result<()> {
    save_to(&config_file(FILE)?, accounts)
}

pub fn add(provider: &str, email: &str) -> Result<String> {
    let account = Account {
        provider: provider.to_string(),
        email: email.to_string(),
    };
    let id = account.id();

    let mut accounts = list()?;
    accounts.known.insert(id.clone(), account);
    accounts.active = Some(id.clone());
    store(&accounts)?;

    Ok(id)
}

pub fn set_active(id: &str) -> Result<()> {
    let mut accounts = list()?;
    anyhow::ensure!(accounts.known.contains_key(id), "unknown account: {id}");
    accounts.active = Some(id.to_string());
    store(&accounts)
}

pub fn active_for(provider: &str) -> Result<(String, Account)> {
    let accounts = list()?;
    // Two different problems with two different fixes, so say which.
    let Some(id) = accounts.active.clone() else {
        anyhow::bail!(
            if accounts.known.is_empty() {
                "no account connected - run `transit account add`"
            } else {
                "no account selected - run `transit account switch`"
            }
        );
    };
    let account = accounts
        .known
        .get(&id)
        .with_context(|| format!("active account {id} is missing - run `transit account switch`"))?;

    anyhow::ensure!(
        account.provider == provider,
        "active account {} is not a {provider} account",
        account.email
    );

    Ok((id, account.clone()))
}

pub fn remove_account(id: &str) -> Result<()> {
    let mut accounts = list()?;
    anyhow::ensure!(accounts.known.remove(id).is_some(), "unknown account: {id}");

    if accounts.active.as_deref() == Some(id) {
        accounts.active = accounts.known.keys().next().cloned();
    }

    store(&accounts)
}