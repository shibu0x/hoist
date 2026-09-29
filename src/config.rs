use anyhow::Ok;
use clap::Subcommand;
use dialoguer::{Select, theme::ColorfulTheme};
use indicatif::HumanBytes;

use crate::{Cli, auth::accounts, auth::gdrive_auth, providers};

#[derive(Subcommand)]
pub enum Commands {
    Config,
    Upload {
        path: Option<String>,
    },
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Download {
        target: String,
        #[arg(long)]
        out: Option<String>,
    },
}

pub async fn config(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Config => {
            let accounts = accounts::list()?;

            if accounts.known.is_empty() {
                gdrive_auth().await?;
                return Ok(());
            }

            let ids: Vec<&String> = accounts.known.keys().collect();
            let mut items: Vec<String> = accounts
                .known
                .values()
                .map(|account| {
                    let active = accounts.active.as_deref() == Some(account.id().as_str());
                    format!("{}{}", account.email, if active { "  (active)" } else { "" })
                })
                .collect();
            items.push("Add another account".to_string());
            items.push("Exit".to_string());

            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Select an account")
                .default(0)
                .items(&items)
                .interact()?;

            if selection < ids.len() {
                accounts::set_active(ids[selection])?;
                println!("Switched to {}", accounts.known[ids[selection]].email);
            } else if selection == ids.len() {
                gdrive_auth().await?;
            }
        }

        Commands::List { limit } => {
            let (_, account) = accounts::active_for("google")?;
            let entries = providers::gdrive::list_files(limit).await?;

            if entries.is_empty() {
                println!("No files yet for {}.", account.email);
                return Ok(());
            }

            println!(
                "{:<32} {:>10}  {:<16}  {:<24}  {}",
                "NAME", "SIZE", "MODIFIED", "TYPE", "LINK"
            );
            for entry in &entries {
                let size = match entry.size_bytes() {
                    Some(bytes) => HumanBytes(bytes).to_string(),
                    None if entry.is_folder() => "-".to_string(),
                    None => "-".to_string(),
                };
                println!(
                    "{:<32} {:>10}  {:<16}  {:<24}  {}",
                    truncate(&entry.name, 32),
                    size,
                    &entry.modified_time.replace('T', " ")[..16.min(entry.modified_time.len())],
                    truncate(&entry.mime_type, 24),
                    entry.web_view_link.as_deref().unwrap_or(&entry.id)
                );
            }
            println!("\n{} file(s) created by transit.", entries.len());
        }

        Commands::Download { target, out } => {
            let (_, account) = accounts::active_for("google")?;
            let entry = providers::gdrive::resolve(&target).await?;

            // Default to the Drive name in the current directory.
            let dest = std::path::PathBuf::from(out.unwrap_or_else(|| entry.name.clone()));
            println!("Downloading {} from {}", entry.name, account.email);

            providers::gdrive::download_file(&entry, &dest).await?;

            println!("Saved to {}", dest.display());
        }

        Commands::Upload { path } => {
            let (_, account) = accounts::active_for("google")?;

            let path = match path {
                Some(given) => crate::picker::resolve_path(&given)?,
                None => crate::picker::pick_file(&std::env::current_dir()?)?,
            };

            println!("Uploading {} to {}", path.display(), account.email);

            let file = providers::gdrive::upload_file(&path).await?;

            println!("Uploaded successfully!");
            println!("Name: {}", file.name);
            println!(
                "Link: {}",
                file.web_view_link.as_deref().unwrap_or(&file.id)
            );
        }
    }

    Ok(())
}

fn truncate(text: &str, width: usize) -> String {
    match text.char_indices().nth(width) {
        Some((cut, _)) => format!("{}...", &text[..cut.saturating_sub(3)]),
        None => text.to_string(),
    }
}
