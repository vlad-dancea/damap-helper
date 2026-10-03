use std::path::Path;

use anyhow::{Context, Result};
use notify_debouncer_full::DebouncedEvent;
use notify_debouncer_full::notify::EventKind;
use notify_debouncer_full::notify::event::ModifyKind;

use super::connect;
use crate::config;
use crate::watcher::FolderWatcher;

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
    for result in watcher.events() {
        match result {
            Ok(events) => events.iter().for_each(|event| report(&root, event)),
            Err(errors) => errors.iter().for_each(|e| eprintln!("watch error: {e}")),
        }
    }
    Ok(())
}

fn report(root: &Path, event: &DebouncedEvent) {
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
    if event.paths.iter().all(|path| in_damap_dir(path)) {
        return;
    }
    let action = match event.kind {
        EventKind::Create(_) => "created",
        EventKind::Remove(_) => "deleted",
        EventKind::Modify(ModifyKind::Name(_)) => "renamed",
        // Permission and timestamp changes are noise for now.
        EventKind::Modify(ModifyKind::Metadata(_)) => return,
        EventKind::Modify(_) => "changed",
        _ => return,
    };
    match event.paths.as_slice() {
        [from, to] => println!("{action}: {} -> {}", relative(from), relative(to)),
        paths => paths
            .iter()
            .for_each(|path| println!("{action}: {}", relative(path))),
    }
}
