//! The storage seam. `ClipStore` mirrors the parts of `satchel_core::Store`
//! the app uses, so the logic around it is tested against an in-memory fake
//! and the real SQLite store is plugged in at startup.

use std::sync::{Mutex, MutexGuard};

use satchel_core::{
    ClipContent, ClipId, ClipSummary, InsertOutcome, Millis, NewClip, Pinboard, PinboardId, Query,
    Result, Settings, Store,
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
