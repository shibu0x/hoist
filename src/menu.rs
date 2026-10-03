use anyhow::Result;
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use indicatif::HumanBytes;
use std::io::IsTerminal;

use crate::auth::accounts;
use crate::providers::gdrive::{self, DriveEntry, ROOT};

pub async fn run() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "the interactive menu needs a terminal - run `hoist --help` for the commands"
        );
    }

    if accounts::list()?.known.is_empty() {
        println!("No account connected yet - let's do that first.\n");
        crate::config::account(crate::config::AccountAction::Add).await?;
        println!();
    }

    loop {
        let choice = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("What do you want to do?")
            .default(0)
            .items([
                "Upload a file",
                "Browse files",
                "Accounts",
                "OAuth client",
                "Exit",
            ])
            .interact()?;

        match choice {
            0 => upload().await?,
            1 => browse().await?,
            2 => accounts_menu().await?,
            3 => client_menu()?,
            _ => return Ok(()),
        }
    }
}

async fn upload() -> Result<()> {
    let Some(path) = crate::picker::pick_file(&std::env::current_dir()?)? else {
        return Ok(());
    };

    let folder: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Destination folder (blank for top level)")
        .allow_empty(true)
        .interact_text()?;

    let parent = match folder.trim() {
        "" => None,
        path => Some(gdrive::ensure_folder_path(path).await?),
    };

    let replace = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Replace a file of the same name instead of duplicating?")
        .default(false)
        .interact()?;

    let file = gdrive::upload_file(&path, parent.as_deref(), replace).await?;
    println!("Uploaded {}", file.name);
    println!(
        "Link: {}",
        file.web_view_link.as_deref().unwrap_or(&file.id)
    );

    Ok(())
}

enum Row {
    Up,
    Enter(DriveEntry),
    Act(DriveEntry),
    Exit,
}

async fn browse() -> Result<()> {
    let mut stack: Vec<(String, String)> = vec![(ROOT.to_string(), "/".to_string())];

    loop {
        let (id, _) = stack.last().expect("stack is never empty");
        let entries = gdrive::list_folder(id, 100).await?;

        let mut rows = Vec::new();
        let mut labels = Vec::new();

        if stack.len() > 1 {
            rows.push(Row::Up);
            labels.push("..".to_string());
        }

        for entry in entries {
            let label = if entry.is_folder() {
                format!("{}/", entry.name)
            } else {
                format!(
                    "{}   {}",
                    entry.name,
                    entry
                        .size_bytes()
                        .map(|b| HumanBytes(b).to_string())
                        .unwrap_or_else(|| "-".into())
                )
            };
            labels.push(label);
            rows.push(if entry.is_folder() {
                Row::Enter(entry)
            } else {
                Row::Act(entry)
            });
        }

        rows.push(Row::Exit);
        labels.push("Back to menu".to_string());

        let path: Vec<&str> = stack.iter().map(|(_, name)| name.as_str()).collect();
        let Some(selection) = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(path.join("/").replace("//", "/"))
            .default(0)
            .items(&labels)
            .interact_opt()?
        else {
            return Ok(());
        };

        match rows.swap_remove(selection) {
            Row::Up => {
                stack.pop();
            }
            Row::Enter(entry) => stack.push((entry.id, entry.name)),
            Row::Act(entry) => file_actions(&entry).await?,
            Row::Exit => return Ok(()),
        }
    }
}

async fn file_actions(entry: &DriveEntry) -> Result<()> {
    let choice = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(&entry.name)
        .default(0)
        .items(["Download", "Copy link", "Delete", "Back"])
        .interact()?;

    match choice {
        0 => {
            let out: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Save as")
                .default(entry.name.clone())
                .interact_text()?;
            let dest = std::path::PathBuf::from(out);
            gdrive::download_file(entry, &dest).await?;
            println!("Saved to {}", dest.display());
        }
        1 => println!("{}", entry.web_view_link.as_deref().unwrap_or(&entry.id)),
        2 if Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("Move {} to Drive trash?", entry.name))
            .default(false)
            .interact()? =>
        {
            gdrive::trash(&entry.id).await?;
            println!("Moved {} to trash", entry.name);
        }
        _ => {}
    }

    Ok(())
}

async fn accounts_menu() -> Result<()> {
    use crate::config::AccountAction;

    let choice = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Accounts")
        .default(0)
        .items(["List", "Add", "Switch", "Remove", "Back"])
        .interact()?;

    match choice {
        0 => crate::config::account(AccountAction::List).await,
        1 => crate::config::account(AccountAction::Add).await,
        2 => crate::config::account(AccountAction::Switch).await,
        3 => crate::config::account(AccountAction::Remove).await,
        _ => Ok(()),
    }
}

fn client_menu() -> Result<()> {
    let choice = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("OAuth client")
        .default(0)
        .items(["Show", "Set or replace", "Back"])
        .interact()?;

    match choice {
        0 => crate::auth::credentials::describe("google"),
        1 => crate::auth::credentials::set("google").map(|_| ()),
        _ => Ok(()),
    }
}
