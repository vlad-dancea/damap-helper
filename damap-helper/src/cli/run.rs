use std::path::Path;
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result, bail};

use super::connect;
use super::screen::{Screen, Update};
use crate::agent::Reviewer;
use crate::config::{self, Config, Credentials};
use crate::watcher::{Change, FolderWatcher};

struct Review {
    reviewer: Reviewer,
    dmp: String,
}

pub fn run(dmp: Option<i64>) -> Result<()> {
    let damap = connect::resume()?;
    let root = config::project_root()?.context("this folder is not set up yet")?;

    let config = Config::load()?.context("this folder is not set up yet")?;
    let (review, status) = match (dmp.or(config.damap.dmp_id), config.ai) {
        (Some(id), Some(ai)) => {
            let reviewer = Reviewer::new(&ai, Credentials::load()?.api_key)?;
            let name = damap
                .list_dmps()?
                .into_iter()
                .find(|listed| listed.id == id)
                .and_then(|listed| listed.name().map(str::to_string))
                .unwrap_or_else(|| "(untitled)".to_string());
            let dmp = serde_json::to_string_pretty(&damap.madmp(id)?)?;
            let status = vec![
                format!("Comparing changes with project #{id} - {name}."),
                format!("Using {} from {}.", reviewer.model(), ai.url),
            ];
            (Some(Review { reviewer, dmp }), status)
        }
        (Some(_), None) if dmp.is_some() => {
            bail!("no AI model to check with; choose one with `damap-helper setup`")
        }
        (None, Some(_)) => (
            None,
            vec![
                "No DMP chosen, so changes are not checked; choose one with `damap-helper setup`."
                    .to_string(),
            ],
        ),
        _ => (None, vec!["Changes are not checked.".to_string()]),
    };

    let watcher = FolderWatcher::new(&root)?;
    let (updates, received) = mpsc::channel();
    let watched = root.clone();
    thread::spawn(move || watch(&watched, watcher, review.as_ref(), &updates));
    Screen::new(&root, status).run(received)
}

fn watch(
    root: &Path,
    watcher: FolderWatcher,
    review: Option<&Review>,
    updates: &mpsc::Sender<Update>,
) {
    for batch in watcher {
        let changes: Vec<Change> = match batch {
            Ok(changes) => changes
                .into_iter()
                .filter(|change| !change.paths().into_iter().all(|p| in_damap_dir(root, p)))
                .collect(),
            Err(errors) => {
                let errors = errors.iter().map(ToString::to_string).collect();
                if updates.send(Update::WatchErrors(errors)).is_err() {
                    return;
                }
                continue;
            }
        };
        if changes.is_empty() {
            continue;
        }
        if updates.send(Update::Changes(changes.clone())).is_err() {
            return;
        }
        if let Some(review) = review {
            let verdict = review.reviewer.review(root, &review.dmp, &changes);
            if updates.send(Update::Verdict(verdict)).is_err() {
                return;
            }
        }
    }
}

fn in_damap_dir(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .is_ok_and(|p| p.starts_with(".damap-helper"))
}
