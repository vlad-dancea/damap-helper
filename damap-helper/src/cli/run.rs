use std::path::Path;

use anyhow::{Context, Result, bail};

use super::connect;
use crate::agent::Reviewer;
use crate::config::{self, Config, Credentials};
use crate::watcher::{Change, FolderWatcher};

/// Checks changes against one DMP.
struct Review {
    reviewer: Reviewer,
    /// The DMP as pretty-printed maDMP JSON, fetched once at start.
    dmp: String,
}

pub fn run(dmp: Option<i64>) -> Result<()> {
    // Connect before watching, so an unreachable DAMAP or an expired login
    // shows up now rather than at the first change.
    let damap = connect::resume()?;
    let root = config::project_root()?.context("this folder is not set up yet")?;

    let config = Config::load()?.context("this folder is not set up yet")?;
    let review = match (dmp.or(config.damap.dmp_id), config.ai) {
        (Some(id), Some(ai)) => {
            let reviewer = Reviewer::new(&ai, Credentials::load()?.api_key)?;
            let dmp = serde_json::to_string_pretty(&damap.madmp(id)?)?;
            println!(
                "Checking changes against DMP #{id} with {}.",
                reviewer.model()
            );
            Some(Review { reviewer, dmp })
        }
        (Some(_), None) if dmp.is_some() => {
            bail!("no AI model to check with; choose one with `damap-helper init`")
        }
        (None, Some(_)) => {
            println!(
                "No DMP chosen, so changes are not checked; choose one with `damap-helper init`."
            );
            None
        }
        _ => None,
    };

    let watcher = FolderWatcher::new(&root)?;
    println!(
        "Watching {} for changes. Press Ctrl+C to stop.",
        root.display()
    );
    for batch in watcher {
        match batch {
            Ok(changes) => {
                // Our own state is not part of the research data.
                let changes: Vec<Change> = changes
                    .into_iter()
                    .filter(|change| !change.paths().into_iter().all(|p| in_damap_dir(&root, p)))
                    .collect();
                changes.iter().for_each(|change| report(&root, change));
                if let Some(review) = &review
                    && !changes.is_empty()
                {
                    check(&root, review, &changes);
                }
            }
            Err(errors) => errors.iter().for_each(|e| eprintln!("watch error: {e}")),
        }
    }
    Ok(())
}

fn in_damap_dir(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .is_ok_and(|p| p.starts_with(".damap-helper"))
}

fn report(root: &Path, change: &Change) {
    let relative = |path: &Path| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    match change {
        Change::Created(path) => println!("created: {}", relative(path)),
        Change::Changed(path) => println!("changed: {}", relative(path)),
        Change::Renamed { from, to } => {
            println!("renamed: {} -> {}", relative(from), relative(to))
        }
        Change::Deleted(path) => println!("deleted: {}", relative(path)),
    }
}

fn check(root: &Path, review: &Review, changes: &[Change]) {
    match review.reviewer.review(root, &review.dmp, changes) {
        Ok(verdict) if verdict.contradicts => {
            let field = verdict
                .dmp_field
                .map(|field| format!(" ({field})"))
                .unwrap_or_default();
            println!("  contradicts the DMP{field}: {}", verdict.explanation);
        }
        Ok(verdict) => println!("  in line with the DMP: {}", verdict.explanation),
        Err(e) => eprintln!("  review failed: {e:#}"),
    }
}
