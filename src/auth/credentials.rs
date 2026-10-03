use anyhow::Result;
use dialoguer::{Confirm, Input, Password, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::token_store::{config_file, load_from, save_to};

const FILE: &str = "credentials.json";

#[derive(Serialize, Deserialize, Clone)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
}

fn from_env(provider: &str) -> Option<OAuthClient> {
    let var = |suffix: &str| std::env::var(format!("HOIST_{}_{suffix}", provider.to_uppercase()));

    match (var("CLIENT_ID"), var("CLIENT_SECRET")) {
        (Ok(client_id), Ok(client_secret)) => Some(OAuthClient {
            client_id,
            client_secret,
        }),
        _ => None,
    }
}

fn from_file(provider: &str) -> Result<Option<OAuthClient>> {
    let mut stored: HashMap<String, OAuthClient> = load_from(&config_file(FILE)?)?;
    Ok(stored.remove(provider))
}

pub fn get(provider: &str) -> Result<Option<OAuthClient>> {
    match from_env(provider) {
        Some(client) => Ok(Some(client)),
        None => from_file(provider),
    }
}

pub fn save(provider: &str, client: &OAuthClient) -> Result<()> {
    let path = config_file(FILE)?;
    let mut stored: HashMap<String, OAuthClient> = load_from(&path)?;
    stored.insert(provider.to_string(), client.clone());
    save_to(&path, &stored)
}

/// `account add` path: reuse the saved client, prompt only when there is none.
/// Deliberately never asks about replacing - that is `client set`'s job, and
/// asking here is what made "Replace it?" appear while connecting an account.
pub fn get_or_prompt(provider: &str) -> Result<OAuthClient> {
    match get(provider)? {
        Some(client) => Ok(client),
        None => prompt_and_save(provider),
    }
}

/// `client show` path.
pub fn describe(provider: &str) -> Result<()> {
    let upper = provider.to_uppercase();

    if let Some(client) = from_env(provider) {
        println!("Client: {}", hint(&client.client_id));
        println!("Source: HOIST_{upper}_CLIENT_ID / HOIST_{upper}_CLIENT_SECRET");
        println!("These shadow the stored file - unset them to use the saved client.");
        return Ok(());
    }

    match from_file(provider)? {
        Some(client) => {
            println!("Client: {}", hint(&client.client_id));
            println!("Source: {}", config_file(FILE)?.display());
        }
        None => println!("No {provider} OAuth client set - run `hoist client set`"),
    }

    Ok(())
}

/// `client set` path: always ends with a client stored.
pub fn set(provider: &str) -> Result<OAuthClient> {
    if let Some(existing) = from_file(provider)? {
        // Say "OAuth client" explicitly: "client" alone reads as "account".
        println!(
            "Saved {provider} Cloud OAuth client: {}",
            hint(&existing.client_id)
        );
        println!("(your app registration in the provider's console - not an account)");

        let replace = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Use a different OAuth client?")
            .default(false)
            .interact()?;

        if !replace {
            println!("Keeping the saved client.\n");
            return Ok(existing);
        }
    }

    prompt_and_save(provider)
}

fn hint(client_id: &str) -> String {
    match client_id.char_indices().nth(14) {
        Some((cut, _)) => format!("{}...", &client_id[..cut]),
        None => client_id.to_string(),
    }
}

fn prompt_and_save(provider: &str) -> Result<OAuthClient> {
    println!("hoist needs a {provider} OAuth client of your own.");
    println!("See README.md for how to create one.\n");

    let client_id: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Client ID")
        .interact_text()?;

    let client_secret = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("Client Secret")
        .interact()?;

    let client = OAuthClient {
        client_id: client_id.trim().to_string(),
        client_secret: client_secret.trim().to_string(),
    };

    save(provider, &client)?;
    println!("Saved to {}\n", config_file(FILE)?.display());

    Ok(client)
}
