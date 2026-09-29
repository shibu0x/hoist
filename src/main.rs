use anyhow::Ok;
use clap::Parser;

mod config;
mod picker;
mod providers;
mod auth;

#[derive(Parser)]
#[command(name = "transit")]
#[command(about = "A cli tool to upload any data to any kind of storage from terminal")]
pub struct Cli {
    #[command(subcommand)]
    command: config::Commands
}

#[tokio::main]
pub async fn main() -> anyhow::Result<()>{
    let cli = Cli::parse();
    let _ = config::config(cli).await?;

    Ok(())
}
