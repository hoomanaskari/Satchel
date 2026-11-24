//! The storage seam. `ClipStore` mirrors the parts of `satchel_core::Store`
//! the app uses, so the logic around it is tested against an in-memory fake
//! and the real SQLite store is plugged in at startup.

use std::sync::{Mutex, MutexGuard};

use satchel_core::{
    ClipContent, ClipId, ClipSummary, InsertOutcome, LinkPreviewJob, LinkPreviewOutcome, Millis,
    NewClip, Pinboard, PinboardId, Query, Result, Settings, Store,
};

pub trait ClipStore: Send {
    fn insert(&mut self, clip: NewClip, now: Millis) -> Result<InsertOutcome>;
    fn list(&self, query: &Query) -> Result<Vec<ClipSummary>>;
    fn content(&self, id: ClipId) -> Result<Option<ClipContent>>;
    fn thumbnail(&self, id: ClipId) -> Result<Option<Vec<u8>>>;
    fn touch(&mut self, id: ClipId, now: Millis) -> Result<()>;
    fn set_title(&mut self, id: ClipId, title: Option<&str>) -> Result<()>;
    fn delete(&mut self, id: ClipId) -> Result<bool>;
    fn clear_history(&mut self) -> Result<u64>;
    fn prune(&mut self, settings: &Settings, now: Millis) -> Result<u64>;
    fn pinboards(&self) -> Result<Vec<Pinboard>>;
    fn create_pinboard(&mut self, name: &str, color: &str) -> Result<Pinboard>;
    fn rename_pinboard(&mut self, id: PinboardId, name: &str) -> Result<()>;
    fn delete_pinboard(&mut self, id: PinboardId) -> Result<()>;
    fn pin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()>;
    fn unpin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()>;
    fn settings(&self) -> Result<Settings>;
    fn save_settings(&mut self, settings: &Settings) -> Result<()>;
    fn save_open_at_login(&mut self, value: Option<bool>) -> Result<()>;
    fn window_state(&self, name: &str) -> Result<Option<String>>;
    fn save_window_state(&mut self, name: &str, value: &str) -> Result<()>;
    fn queue_link_preview(&mut self, id: ClipId, now: Millis) -> Result<bool>;
    fn queue_link_backfill(&mut self, now: Millis) -> Result<Option<ClipId>>;
    fn claim_link_preview(&mut self, now: Millis, lease_ms: Millis) -> Result<Option<LinkPreviewJob>>;
    fn next_link_preview_at(&self) -> Result<Option<Millis>>;
    fn finish_link_preview(&mut self, id: ClipId, outcome: LinkPreviewOutcome, now: Millis) -> Result<bool>;
    fn link_preview_image(&self, id: ClipId) -> Result<Option<Vec<u8>>>;
    fn clear_link_previews(&mut self) -> Result<u64>;
}

impl ClipStore for Store {
    fn insert(&mut self, clip: NewClip, now: Millis) -> Result<InsertOutcome> {
        Store::insert(self, clip, now)
    }
    fn list(&self, query: &Query) -> Result<Vec<ClipSummary>> {
        Store::list(self, query)
    }
    fn content(&self, id: ClipId) -> Result<Option<ClipContent>> {
        Store::content(self, id)
    }
    fn thumbnail(&self, id: ClipId) -> Result<Option<Vec<u8>>> {
        Store::thumbnail(self, id)
    }
    fn touch(&mut self, id: ClipId, now: Millis) -> Result<()> {
        Store::touch(self, id, now)
    }
    fn set_title(&mut self, id: ClipId, title: Option<&str>) -> Result<()> {
        Store::set_title(self, id, title)
    }
    fn delete(&mut self, id: ClipId) -> Result<bool> {
        Store::delete(self, id)
    }
    fn clear_history(&mut self) -> Result<u64> {
        Store::clear_history(self)
    }
    fn prune(&mut self, settings: &Settings, now: Millis) -> Result<u64> {
        Store::prune(self, settings, now)
    }
    fn pinboards(&self) -> Result<Vec<Pinboard>> {
        Store::pinboards(self)
    }
    fn create_pinboard(&mut self, name: &str, color: &str) -> Result<Pinboard> {
        Store::create_pinboard(self, name, color)
    }
    fn rename_pinboard(&mut self, id: PinboardId, name: &str) -> Result<()> {
        Store::rename_pinboard(self, id, name)
    }
    fn delete_pinboard(&mut self, id: PinboardId) -> Result<()> {
        Store::delete_pinboard(self, id)
    }
    fn pin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()> {
        Store::pin(self, clip, pinboard)
    }
    fn unpin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()> {
        Store::unpin(self, clip, pinboard)
    }
    fn settings(&self) -> Result<Settings> {
        Store::settings(self)
    }
    fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        Store::save_settings(self, settings)
    }
    fn save_open_at_login(&mut self, value: Option<bool>) -> Result<()> {
        Store::save_open_at_login(self, value)
    }
    fn window_state(&self, name: &str) -> Result<Option<String>> {
        Store::window_state(self, name)
    }
    fn save_window_state(&mut self, name: &str, value: &str) -> Result<()> {
        Store::save_window_state(self, name, value)
    }
    fn queue_link_preview(&mut self, id: ClipId, now: Millis) -> Result<bool> {
        Store::queue_link_preview(self, id, now)
    }
    fn queue_link_backfill(&mut self, now: Millis) -> Result<Option<ClipId>> {
        Store::queue_link_backfill(self, now)
    }
    fn claim_link_preview(&mut self, now: Millis, lease_ms: Millis) -> Result<Option<LinkPreviewJob>> {
        Store::claim_link_preview(self, now, lease_ms)
    }
    fn next_link_preview_at(&self) -> Result<Option<Millis>> {
        Store::next_link_preview_at(self)
    }
    fn finish_link_preview(&mut self, id: ClipId, outcome: LinkPreviewOutcome, now: Millis) -> Result<bool> {
        Store::finish_link_preview(self, id, outcome, now)
    }
    fn link_preview_image(&self, id: ClipId) -> Result<Option<Vec<u8>>> {
        Store::link_preview_image(self, id)
    }
    fn clear_link_previews(&mut self) -> Result<u64> {
        Store::clear_link_previews(self)
    }
}

/// The single store connection, shared by the watcher thread and the command
/// handlers (which run on the async runtime's worker threads, never on the
/// main thread).
pub struct SharedStore(Mutex<Box<dyn ClipStore>>);

impl SharedStore {
    pub fn new(store: Box<dyn ClipStore>) -> Self {
        Self(Mutex::new(store))
    }

    /// A panic while holding the lock cannot leave SQLite half-written (each
    /// call is its own statement or transaction), so poisoning is ignored.
    pub fn lock(&self) -> MutexGuard<'_, Box<dyn ClipStore>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_millis() -> Millis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as Millis)
        .unwrap_or(0)
}
