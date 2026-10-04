use anyhow::Result;

use super::connect;
use super::model::{self, ModelArgs};

pub fn run(url: Option<String>, model: ModelArgs) -> Result<()> {
    let damap = connect::init(url)?;
    let dmps = damap.list_dmps()?;
    match dmps.len() {
        0 => println!("Logged in. You have no DMPs in DAMAP yet."),
        1 => println!("Logged in. You have 1 DMP:"),
        n => println!("Logged in. You have {n} DMPs:"),
    }
    for dmp in dmps {
        let title = dmp.title.as_deref().unwrap_or("(untitled)");
        println!("  #{:<5} {title}", dmp.id);
    }
    model::setup(model)?;
    println!("Run `damap-helper run` to start watching this folder.");
    Ok(())
}
