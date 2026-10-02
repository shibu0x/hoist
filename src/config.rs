use clap::Subcommand;
use dialoguer::{Confirm, Select, theme::ColorfulTheme};
use indicatif::HumanBytes;

use crate::{
    Cli,
    auth::{accounts, accounts::Accounts, gdrive_auth},
    providers,
};

#[derive(Subcommand)]
pub enum Commands {
    /// Connect, switch and disconnect storage accounts
    Account {
        #[command(subcommand)]
        action: AccountAction,
    },
    /// Manage the OAuth client (your app registration, not an account)
    Client {
        #[command(subcommand)]
        action: ClientAction,
    },
    /// Upload a file. Omit the path to browse and search for one.
    Upload {
        path: Option<String>,
        /// Destination folder path, created if missing (like mkdir -p)
        #[arg(long)]
        folder: Option<String>,
    },
    /// List a folder's contents. Defaults to the top level.
    List {
        /// Folder path or id. Omit for the top level.
        folder: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Create a folder, including any missing parents
    Mkdir { path: String },
    /// Download a file by link, id, or name
    Download {
        target: String,
        #[arg(long)]
        out: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum AccountAction {
    /// Authorise a new account in the browser
    Add,
    /// Show connected accounts
    List,
    /// Choose which account other commands act on
    Switch,
    /// Revoke access and delete an account's stored credentials
    Remove,
}

#[derive(Subcommand)]
pub enum ClientAction {
    /// Enter or replace the client ID and secret
    Set,
    /// Show which OAuth client is configured
    Show,
}

pub async fn config(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Account { action } => account(action).await?,
        Commands::Client { action } => match action {
            ClientAction::Set => {
                crate::auth::credentials::set("google")?;
            }
            ClientAction::Show => crate::auth::credentials::describe("google")?,
        },

        Commands::Mkdir { path } => {
            let (_, account) = accounts::active_for("google")?;
            providers::gdrive::ensure_folder_path(&path).await?;
            println!("Folder {path} ready on {}", account.email);
        }

        Commands::Upload { path, folder } => {
            let (_, account) = accounts::active_for("google")?;

            let path = match path {
                Some(given) => crate::picker::resolve_path(&given)?,
                None => match crate::picker::pick_file(&std::env::current_dir()?)? {
                    Some(picked) => picked,
                    None => {
                        println!("Cancelled.");
                        return Ok(());
                    }
                },
            };

            // Create the destination if needed: an upload that fails because a
            // folder is missing is worse than one that makes it.
            let parent = match &folder {
                Some(folder) => Some(providers::gdrive::ensure_folder_path(folder).await?),
                None => None,
            };

            println!(
                "Uploading {} to {}{}",
                path.display(),
                account.email,
                folder.map(|f| format!(" ({f})")).unwrap_or_default()
            );
            let file = providers::gdrive::upload_file(&path, parent.as_deref()).await?;

            println!("Uploaded successfully!");
            println!("Name: {}", file.name);
            println!(
                "Link: {}",
                file.web_view_link.as_deref().unwrap_or(&file.id)
            );
        }

        Commands::List { folder, limit } => {
            let (_, account) = accounts::active_for("google")?;

            let parent = match &folder {
                Some(path) => providers::gdrive::resolve_folder(path).await?,
                None => providers::gdrive::ROOT.to_string(),
            };
            let entries = providers::gdrive::list_folder(&parent, limit).await?;

            let location = folder.as_deref().unwrap_or("top level");
            if entries.is_empty() {
                println!("Nothing in {location} for {}.", account.email);
                return Ok(());
            }
            println!("{location} - {}\n", account.email);

            println!(
                "{:<32} {:>10}  {:<16}  {:<24}  {}",
                "NAME", "SIZE", "MODIFIED", "TYPE", "LINK"
            );
            for entry in &entries {
                let size = entry
                    .size_bytes()
                    .map(|bytes| HumanBytes(bytes).to_string())
                    .unwrap_or_else(|| "-".to_string());
                let name = if entry.is_folder() {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };

                println!(
                    "{:<32} {:>10}  {:<16}  {:<24}  {}",
                    truncate(&name, 32),
                    size,
                    &entry.modified_time.replace('T', " ")[..16.min(entry.modified_time.len())],
                    truncate(if entry.is_folder() { "folder" } else { &entry.mime_type }, 24),
                    entry.web_view_link.as_deref().unwrap_or(&entry.id)
                );
            }
            println!("\n{} file(s) created by transit.", entries.len());
        }

        Commands::Download { target, out } => {
            let (_, account) = accounts::active_for("google")?;
            let entry = providers::gdrive::resolve(&target).await?;

            let dest = std::path::PathBuf::from(out.unwrap_or_else(|| entry.name.clone()));
            println!("Downloading {} from {}", entry.name, account.email);

            providers::gdrive::download_file(&entry, &dest).await?;
            println!("Saved to {}", dest.display());
        }
    }

    Ok(())
}

async fn account(action: AccountAction) -> anyhow::Result<()> {
    match action {
        AccountAction::Add => gdrive_auth().await?,

        AccountAction::List => {
            let accounts = accounts::list()?;
            if accounts.known.is_empty() {
                println!("No accounts connected - run `transit account add`");
                return Ok(());
            }
            for entry in accounts.known.values() {
                let active = accounts.active.as_deref() == Some(entry.id().as_str());
                println!(
                    "{} {:<32} {}",
                    if active { "*" } else { " " },
                    entry.email,
                    entry.provider
                );
            }
        }

        AccountAction::Switch => {
            let accounts = accounts::list()?;
            let Some(id) = pick_account(&accounts, "Switch to")? else {
                return Ok(());
            };
            accounts::set_active(&id)?;
            println!("Switched to {}", accounts.known[&id].email);
        }

        AccountAction::Remove => {
            let accounts = accounts::list()?;
            let Some(id) = pick_account(&accounts, "Remove")? else {
                return Ok(());
            };
            let email = accounts.known[&id].email.clone();

            if !Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Remove {email} and revoke transit's access?"))
                .default(false)
                .interact()?
            {
                println!("Cancelled.");
                return Ok(());
            }

            // Revoke first: deleting the token locally leaves nothing to
            // authorise its own revocation.
            match crate::auth::gdrive_revoke(&id).await {
                Ok(()) => println!("Revoked access for {email}"),
                Err(e) => println!(
                    "Could not revoke remotely ({e}).\n\
                     Remove it by hand at https://myaccount.google.com/permissions"
                ),
            }

            crate::auth::token_store::forget_account(&id)?;
            providers::gdrive::forget_sessions_for(&id)?;
            accounts::remove_account(&id)?;

            println!("Removed {email}");
            println!("Your OAuth client is kept - reconnect without re-entering it.");
        }
    }

    Ok(())
}

/// Shared by switch and remove. The id travels inside each row rather than
/// being looked up by index in a parallel vec, so adding or reordering rows
/// cannot silently act on the wrong account.
fn pick_account(accounts: &Accounts, verb: &str) -> anyhow::Result<Option<String>> {
    if accounts.known.is_empty() {
        println!("No accounts connected - run `transit account add`");
        return Ok(None);
    }

    let mut rows: Vec<(String, Option<String>)> = accounts
        .known
        .values()
        .map(|entry| {
            let active = accounts.active.as_deref() == Some(entry.id().as_str());
            let label = format!("{}{}", entry.email, if active { "  (active)" } else { "" });
            (label, Some(entry.id()))
        })
        .collect();
    rows.push(("Exit".to_string(), None));

    let labels: Vec<&str> = rows.iter().map(|(label, _)| label.as_str()).collect();
    let Some(selection) = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("{verb} which account?"))
        .default(0)
        .items(&labels)
        .interact_opt()?
    else {
        return Ok(None);
    };

    Ok(rows[selection].1.clone())
}

fn truncate(text: &str, width: usize) -> String {
    match text.char_indices().nth(width) {
        Some((cut, _)) => format!("{}...", &text[..cut.saturating_sub(3)]),
        None => text.to_string(),
    }
}
