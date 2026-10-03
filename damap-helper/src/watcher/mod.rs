//! File watcher: observes the project folder and emits debounced change events.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::Result;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

const DEBOUNCE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct FolderWatcher {
    // Kept alive for as long as the watcher should run; dropping it stops watching.
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    events: Receiver<DebounceEventResult>,
}

impl FolderWatcher {
    pub fn new(root: &Path) -> Result<Self> {
        let (tx, events) = mpsc::channel();
        let mut debouncer = new_debouncer(DEBOUNCE_TIMEOUT, None, tx)?;
        debouncer.watch(root, RecursiveMode::Recursive)?;
        Ok(Self {
            _debouncer: debouncer,
            events,
        })
    }

    /// Channel of debounced event batches; `recv()` blocks until the next one.
    pub fn events(&self) -> &Receiver<DebounceEventResult> {
        &self.events
    }
}
