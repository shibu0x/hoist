use clap::Parser;

mod auth;
mod config;
mod menu;
mod picker;
mod providers;

#[derive(Parser)]
#[command(name = "hoist")]
#[command(about = "A cli tool to upload any data to any kind of storage from terminal")]
pub struct Cli {
    #[command(subcommand)]
    /// Omit for an interactive menu
    command: Option<config::Commands>,
}

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    config::config(cli).await?;

    Ok(())
}
