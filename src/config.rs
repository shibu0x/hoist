use anyhow::Ok;
use clap::Subcommand;

use crate::{Cli, auth::gdrive_auth, providers};

#[derive(Subcommand)]
pub enum Commands {
    Config,
    Upload { path: String },
}

pub async fn config(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Config => {
            gdrive_auth().await?;
        }

        Commands::Upload { path } => {
            println!("Uploading: {path}");

            let file = providers::gdrive::upload_file(std::path::Path::new(&path)).await?;

            println!("Uploaded successfully!");
            println!("Name: {}", file.name);
            println!("ID: {}", file.id);
        }
    }

    Ok(())
}
