use std::path::Path;

use anyhow::{Context, Result};

use super::connect;
use crate::config;
use crate::watcher::{Change, FolderWatcher};

pub fn run() -> Result<()> {
    // Connect before watching, so an unreachable DAMAP or an expired login
    // shows up now rather than at the first change.
    let _damap = connect::resume()?;
    let root = config::project_root()?.context("this folder is not set up yet")?;

    let watcher = FolderWatcher::new(&root)?;
    println!(
        "Watching {} for changes. Press Ctrl+C to stop.",
        root.display()
    );
    for batch in watcher {
        match batch {
            Ok(changes) => changes.iter().for_each(|change| report(&root, change)),
            Err(errors) => errors.iter().for_each(|e| eprintln!("watch error: {e}")),
        }
    }
    Ok(())
}

fn report(root: &Path, change: &Change) {
    let relative = |path: &Path| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    // Our own state is not part of the research data.
    let in_damap_dir = |path: &Path| {
        path.strip_prefix(root)
            .is_ok_and(|p| p.starts_with(".damap"))
    };
    if change.paths().into_iter().all(in_damap_dir) {
        return;
    }
    match change {
        Change::Created(path) => println!("created: {}", relative(path)),
        Change::Changed(path) => println!("changed: {}", relative(path)),
        Change::Renamed { from, to } => {
            println!("renamed: {} -> {}", relative(from), relative(to))
        }
        Change::Deleted(path) => println!("deleted: {}", relative(path)),
    }
}
