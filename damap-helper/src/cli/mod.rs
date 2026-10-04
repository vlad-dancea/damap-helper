//! Command-line interface: argument parsing and one module per command.

mod connect;
mod init;
mod model;
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
    /// Set up this folder: connect to DAMAP, log in, and choose an AI model.
    Init {
        /// DAMAP backend URL; asked for if not given.
        #[arg(long)]
        url: Option<String>,
        #[command(flatten)]
        model: model::ModelArgs,
    },
    /// Watch this folder for changes.
    Run {
        /// Have the model from `[ai]` in `.damap-helper/config.toml` check each
        /// change against the DMP with this id.
        #[arg(long)]
        dmp: Option<i64>,
    },
    /// Forget the saved login for this folder.
    Logout,
}

pub fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Init { url, model } => init::run(url, model),
        Command::Run { dmp } => run::run(dmp),
        Command::Logout => {
            let mut credentials = Credentials::load()?;
            if credentials.refresh_token.take().is_some() {
                credentials.save()?;
                println!("Logged out.");
            } else {
                println!("Not logged in.");
            }
            Ok(())
        }
    }
}
