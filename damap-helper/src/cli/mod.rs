//! Command-line interface: argument parsing and one module per command.

mod connect;
mod init;
mod run;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::Credentials;

/// Keeps a research folder in line with its DAMAP data management plan.
#[derive(Parser)]
#[command(version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Set up this folder: connect to DAMAP and log in.
    Init {
        /// DAMAP backend URL; asked for if not given.
        #[arg(long)]
        url: Option<String>,
    },
    /// Watch this folder for changes.
    Run,
    /// Forget the saved login for this folder.
    Logout,
}

pub fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Init { url } => init::run(url),
        Command::Run => run::run(),
        Command::Logout => {
            if Credentials::delete()? {
                println!("Logged out.");
            } else {
                println!("Not logged in.");
            }
            Ok(())
        }
    }
}
