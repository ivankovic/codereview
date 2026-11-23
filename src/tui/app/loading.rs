//! A bar in the middle of the screen while something is fetched, read or worked out, so that a
//! key whose answer takes a while is seen to have been taken. Work done on this thread is
//! deferred by one frame: the bar is drawn first, then the work runs, then the screen it
//! leads to. Work on another thread (a pull request's fetch, the symbol index) keeps the bar up
//! until its answer is taken in.
//!
//! A child module of `app`, so it goes on using the app's own methods.

use std::time::Instant;

use super::*;

/// Work deferred until its loading bar is on screen.
pub type Work = Box<dyn FnOnce(&mut App)>;

/// What is being waited for, and since when.
#[derive(Debug, Clone)]
pub struct Loading {
    pub label: String,
    pub since: Instant,
    /// How long it may take before the bar is worth drawing: none for work about to block the
    /// screen, a moment for work going on elsewhere that is often done before anyone would see.
    pub after: std::time::Duration,
}

impl Loading {
    /// Whether the bar is drawn now.
    pub fn shown(&self) -> bool {
        self.since.elapsed() >= self.after
    }
}

impl App {
    /// Shows `label` on a bar, and runs `work` once the bar is on screen.
    pub(crate) fn later(
        &mut self,
        label: impl Into<String>,
        work: impl FnOnce(&mut App) + 'static,
    ) {
        self.show_loading(label);
        self.deferred = Some(Box::new(work));
    }

    /// Puts the bar up for work going on elsewhere; `done_loading` takes it down.
    pub(super) fn show_loading(&mut self, label: impl Into<String>) {
        self.loading = Some(Loading {
            label: label.into(),
            since: Instant::now(),
            after: std::time::Duration::ZERO,
        });
    }

    /// The bar for work in the background that is often done in a moment: drawn only once it
    /// has taken longer than that, and never over a bar already up.
    pub(crate) fn show_loading_unless_quick(&mut self, label: impl Into<String>) {
        if self.loading.is_none() {
            self.loading = Some(Loading {
                label: label.into(),
                since: Instant::now(),
                after: std::time::Duration::from_millis(400),
            });
        }
    }

    pub(super) fn done_loading(&mut self) {
        self.loading = None;
    }

    /// Runs the deferred work, if there is any, and takes its bar down unless the work put up
    /// another. True when there was work, so the screen is drawn again.
    pub fn run_deferred(&mut self) -> bool {
        let Some(work) = self.deferred.take() else {
            return false;
        };
        let before = self.loading.as_ref().map(|l| l.since);
        work(self);
        if self.loading.as_ref().map(|l| l.since) == before && self.deferred.is_none() {
            self.loading = None;
        }
        true
    }

    /// Takes down whatever bar is up, the symbols' at start included, for a test to put up its
    /// own.
    #[cfg(test)]
    pub(crate) fn done_loading_for_tests(&mut self) {
        self.loading = None;
    }

    /// Whether the bar is up, so the screen is drawn often enough to move it.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }
}
