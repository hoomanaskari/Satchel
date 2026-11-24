//! Fakes for every seam in `Ctx`, used by the unit tests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use satchel_core::{
    ClipContent, ClipId, ClipKind, ClipSummary, Error, InsertOutcome, LinkPreviewJob,
    LinkPreviewOutcome, Millis, NewClip, Pinboard, PinboardId, Query, Result, Settings,
    privacy::CaptureContext,
};

use crate::clipboard::{
    ClipboardSink, ClipboardSource, Payload, ReadLimits, SELF_MARKER_TYPE, SelfWrites,
    WriteRequest,
};
use crate::ctx::{Ctx, Policy, SettingsWindowControl, ShelfControl};
use crate::error::CmdResult;
use crate::events::{ChangeReason, EventSink};
use crate::login_item::{LoginItems, Status, Unavailable};
use crate::platform::{AppCatalog, AppInfo, Clock, Desktop, FrontApp};
use crate::shortcut::ShortcutRegistrar;
use crate::storage::{ClipStore, SharedStore};

pub fn sample_settings() -> Settings {
    // Built field by field: `Settings::default()` calls into the core.
    Settings {
        max_items: 2000,
        retention_days: Some(90),
        excluded_app_ids: vec!["com.agilebits.onepassword7".into()],
        capture_paused: false,
        capture_images: true,
        shelf_shortcut: "CommandOrControl+Shift+V".into(),
        paste_on_select: true,
        paste_plain_text: false,
        link_previews: false,
        check_for_updates: false,
        open_at_login: None,
    }
}

/// Shared log of everything the fakes observed, in order.
#[derive(Default)]
pub struct Log(Mutex<Vec<String>>);

impl Log {
    pub fn push(&self, entry: impl Into<String>) {
        self.0.lock().unwrap().push(entry.into());
    }
    pub fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

// ---------------------------------------------------------------- store

struct Row {
    id: ClipId,
    clip: NewClip,
    title: Option<String>,
    created_at: Millis,
    last_used_at: Millis,
    pins: Vec<PinboardId>,
}

#[derive(Default)]
pub struct FakeStore {
    rows: Vec<Row>,
    boards: Vec<Pinboard>,
    next_id: i64,
    settings: Option<Settings>,
    windows: std::collections::HashMap<String, String>,
    pub fail: bool,
}

impl FakeStore {
    pub fn failing() -> Self {
        Self { fail: true, ..Self::default() }
    }

    fn check(&self) -> Result<()> {
        if self.fail {
            Err(Error::Invalid("secret clip text".into()))
        } else {
            Ok(())
        }
    }

    fn summary_of(row: &Row) -> ClipSummary {
        ClipSummary {
            id: row.id,
            kind: row.clip.kind,
            title: row.title.clone(),
            preview: row.clip.text.clone().unwrap_or_default(),
            source_app: row.clip.source_app.clone(),
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            byte_size: 0,
            has_thumbnail: row.clip.thumbnail_png.is_some(),
            pinboard_ids: row.pins.clone(),
            link_preview: None,
        }
    }

    fn row_mut(&mut self, id: ClipId) -> Result<&mut Row> {
        self.rows.iter_mut().find(|r| r.id == id).ok_or(Error::NotFound)
    }
}

fn same_content(a: &NewClip, b: &NewClip) -> bool {
    a.kind == b.kind && a.text == b.text && a.image_png == b.image_png && a.file_paths == b.file_paths
}

impl ClipStore for FakeStore {
    fn insert(&mut self, clip: NewClip, now: Millis) -> Result<InsertOutcome> {
        self.check()?;
        if let Some(i) = self.rows.iter().position(|r| same_content(&r.clip, &clip)) {
            self.rows[i].last_used_at = now;
            return Ok(InsertOutcome::Bumped(Self::summary_of(&self.rows[i])));
        }
        self.next_id += 1;
        let row = Row {
            id: self.next_id,
            clip,
            title: None,
            created_at: now,
            last_used_at: now,
            pins: vec![],
        };
        let summary = Self::summary_of(&row);
        self.rows.push(row);
        Ok(InsertOutcome::Inserted(summary))
    }
    fn list(&self, query: &Query) -> Result<Vec<ClipSummary>> {
        self.check()?;
        let mut rows: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| query.kind.is_none_or(|k| r.clip.kind == k))
            .filter(|r| query.pinboard_id.is_none_or(|p| r.pins.contains(&p)))
            .filter(|r| {
                query
                    .text
                    .as_ref()
                    .is_none_or(|t| r.clip.text.as_deref().unwrap_or("").contains(t.as_str()))
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse((r.last_used_at, r.id)));
        let limit = if query.limit == 0 { 50 } else { query.limit as usize };
        Ok(rows.into_iter().take(limit).map(Self::summary_of).collect())
    }
    fn content(&self, id: ClipId) -> Result<Option<ClipContent>> {
        self.check()?;
        Ok(self.rows.iter().find(|r| r.id == id).map(|r| ClipContent {
            id: r.id,
            kind: r.clip.kind,
            text: r.clip.text.clone(),
            html: r.clip.html.clone(),
            rtf: r.clip.rtf.clone(),
            image_png: r.clip.image_png.clone(),
            file_paths: r.clip.file_paths.clone(),
        }))
    }
    fn thumbnail(&self, id: ClipId) -> Result<Option<Vec<u8>>> {
        self.check()?;
        Ok(self.rows.iter().find(|r| r.id == id).and_then(|r| r.clip.thumbnail_png.clone()))
    }
    fn touch(&mut self, id: ClipId, now: Millis) -> Result<()> {
        self.check()?;
        self.row_mut(id)?.last_used_at = now;
        Ok(())
    }
    fn set_title(&mut self, id: ClipId, title: Option<&str>) -> Result<()> {
        self.check()?;
        self.row_mut(id)?.title = title.map(str::to_string);
        Ok(())
    }
    fn delete(&mut self, id: ClipId) -> Result<bool> {
        self.check()?;
        let before = self.rows.len();
        self.rows.retain(|r| r.id != id);
        Ok(self.rows.len() != before)
    }
    fn clear_history(&mut self) -> Result<u64> {
        self.check()?;
        let before = self.rows.len();
        self.rows.retain(|r| !r.pins.is_empty());
        Ok((before - self.rows.len()) as u64)
    }
    fn prune(&mut self, settings: &Settings, _now: Millis) -> Result<u64> {
        self.check()?;
        let unpinned = self.rows.iter().filter(|r| r.pins.is_empty()).count();
        let excess = unpinned.saturating_sub(settings.max_items as usize);
        let mut oldest: Vec<(Millis, ClipId)> = self
            .rows
            .iter()
            .filter(|r| r.pins.is_empty())
            .map(|r| (r.last_used_at, r.id))
            .collect();
        oldest.sort();
        let doomed: Vec<ClipId> = oldest.into_iter().take(excess).map(|(_, id)| id).collect();
        self.rows.retain(|r| !doomed.contains(&r.id));
        Ok(doomed.len() as u64)
    }
    fn pinboards(&self) -> Result<Vec<Pinboard>> {
        self.check()?;
        Ok(self.boards.clone())
    }
    fn create_pinboard(&mut self, name: &str, color: &str) -> Result<Pinboard> {
        self.check()?;
        self.next_id += 1;
        let board = Pinboard {
            id: self.next_id,
            name: name.into(),
            color: color.into(),
            position: self.boards.len() as i64,
        };
        self.boards.push(board.clone());
        Ok(board)
    }
    fn rename_pinboard(&mut self, id: PinboardId, name: &str) -> Result<()> {
        self.check()?;
        let board = self.boards.iter_mut().find(|b| b.id == id).ok_or(Error::NotFound)?;
        board.name = name.into();
        Ok(())
    }
    fn delete_pinboard(&mut self, id: PinboardId) -> Result<()> {
        self.check()?;
        self.boards.retain(|b| b.id != id);
        for row in &mut self.rows {
            row.pins.retain(|&p| p != id);
        }
        Ok(())
    }
    fn pin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()> {
        self.check()?;
        if !self.boards.iter().any(|b| b.id == pinboard) {
            return Err(Error::NotFound);
        }
        let row = self.row_mut(clip)?;
        if !row.pins.contains(&pinboard) {
            row.pins.push(pinboard);
        }
        Ok(())
    }
    fn unpin(&mut self, clip: ClipId, pinboard: PinboardId) -> Result<()> {
        self.check()?;
        self.row_mut(clip)?.pins.retain(|&p| p != pinboard);
        Ok(())
    }
    fn settings(&self) -> Result<Settings> {
        self.check()?;
        Ok(self.settings.clone().unwrap_or_else(sample_settings))
    }
    fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        self.check()?;
        // Like the real store: "open at login" has its own write.
        let kept = self.settings.as_ref().and_then(|s| s.open_at_login);
        self.settings = Some(Settings { open_at_login: kept, ..settings.clone() });
        Ok(())
    }
    fn save_open_at_login(&mut self, value: Option<bool>) -> Result<()> {
        self.check()?;
        let mut settings = self.settings.clone().unwrap_or_default();
        settings.open_at_login = value;
        self.settings = Some(settings);
        Ok(())
    }
    fn window_state(&self, name: &str) -> Result<Option<String>> {
        self.check()?;
        Ok(self.windows.get(name).cloned())
    }
    fn save_window_state(&mut self, name: &str, value: &str) -> Result<()> {
        self.check()?;
        self.windows.insert(name.to_owned(), value.to_owned());
        Ok(())
    }
    // Link previews are tested against the real store
    // (`Harness::with_real_core`); the fake has none.
    fn queue_link_preview(&mut self, _id: ClipId, _now: Millis) -> Result<bool> {
        self.check()?;
        Ok(false)
    }
    fn queue_link_backfill(&mut self, _now: Millis) -> Result<Option<ClipId>> {
        self.check()?;
        Ok(None)
    }
    fn claim_link_preview(&mut self, _now: Millis, _lease_ms: Millis) -> Result<Option<LinkPreviewJob>> {
        self.check()?;
        Ok(None)
    }
    fn next_link_preview_at(&self) -> Result<Option<Millis>> {
        Ok(None)
    }
    fn finish_link_preview(&mut self, _id: ClipId, _outcome: LinkPreviewOutcome, _now: Millis) -> Result<bool> {
        Ok(false)
    }
    fn link_preview_image(&self, _id: ClipId) -> Result<Option<Vec<u8>>> {
        self.check()?;
        Ok(None)
    }
    fn clear_link_previews(&mut self) -> Result<u64> {
        self.check()?;
        Ok(0)
    }
}

// ---------------------------------------------------------------- events

/// Records events in the log, except `clip_saved`, which is counted apart
/// (it fires alongside `clips-changed inserted`, so logging it would only
/// repeat that entry).
pub struct FakeEvents(pub Arc<Log>, pub Arc<AtomicUsize>);

impl EventSink for FakeEvents {
    fn clips_changed(&self, reason: ChangeReason, id: Option<ClipId>) {
        let reason = serde_json::to_value(reason).unwrap();
        let reason = reason.as_str().unwrap();
        match id {
            Some(id) => self.0.push(format!("event clips-changed {reason} {id}")),
            None => self.0.push(format!("event clips-changed {reason}")),
        }
    }
    fn pinboards_changed(&self) {
        self.0.push("event pinboards-changed");
    }
    fn settings_changed(&self, settings: &Settings) {
        self.0.push(format!("event settings-changed paused={}", settings.capture_paused));
    }
    fn capture_skipped(&self, kind: ClipKind, limit_bytes: u64) {
        let kind = serde_json::to_value(kind).unwrap();
        self.0.push(format!("event capture-skipped {} {limit_bytes}", kind.as_str().unwrap()));
    }
    fn clip_saved(&self) {
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------- desktop

pub struct FakeDesktop {
    pub log: Arc<Log>,
    pub front: Mutex<Option<FrontApp>>,
    pub trusted: AtomicBool,
    /// When false, `activate_app` does not change the frontmost app.
    pub activation_works: AtomicBool,
}

impl Desktop for FakeDesktop {
    fn frontmost_app(&self) -> Option<FrontApp> {
        self.front.lock().unwrap().clone()
    }
    fn activate_app(&self, pid: i32) -> bool {
        self.log.push(format!("activate {pid}"));
        if self.activation_works.load(Ordering::SeqCst) {
            *self.front.lock().unwrap() = Some(FrontApp {
                pid,
                id: format!("app.{pid}"),
                name: format!("App {pid}"),
            });
        }
        true
    }
    fn can_post_keystrokes(&self) -> bool {
        self.trusted.load(Ordering::SeqCst)
    }
    fn request_keystroke_permission(&self) {
        self.log.push("prompt accessibility");
    }
    fn open_keystroke_permission_settings(&self) {
        self.log.push("open accessibility settings");
    }
    fn post_paste_keystroke(&self) -> std::result::Result<(), ()> {
        self.log.push("keystroke paste");
        Ok(())
    }
}

// ---------------------------------------------------------------- clipboard

/// Fake system clipboard. Shared by the source (watcher side) and the sink
/// (paste side), like the real pasteboard.
#[derive(Default)]
pub struct FakeBoard {
    pub change: AtomicI64,
    pub formats: Mutex<Vec<String>>,
    pub payload: Mutex<Option<Payload>>,
    pub fail_writes: AtomicBool,
    /// The latest request Satchel wrote, to check every flavor it carried.
    pub written: Mutex<Option<WriteRequest>>,
}

impl FakeBoard {
    /// Simulates another app copying something.
    pub fn external_copy(&self, formats: &[&str], payload: Payload) {
        *self.formats.lock().unwrap() = formats.iter().map(|s| s.to_string()).collect();
        *self.payload.lock().unwrap() = Some(payload);
        self.change.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct FakeSource {
    pub board: Arc<FakeBoard>,
    pub log: Arc<Log>,
}

impl ClipboardSource for FakeSource {
    fn change_count(&mut self) -> i64 {
        self.board.change.load(Ordering::SeqCst)
    }
    fn format_names(&mut self) -> Vec<String> {
        self.log.push("read formats");
        self.board.formats.lock().unwrap().clone()
    }
    fn read(&mut self, limits: &ReadLimits) -> Option<Payload> {
        self.log.push(format!("read content images={}", limits.images));
        let payload = self.board.payload.lock().unwrap().clone()?;
        match payload {
            Payload::Image { .. } if !limits.images => None,
            other => Some(other),
        }
    }
}

pub struct FakeSink {
    pub board: Arc<FakeBoard>,
    pub log: Arc<Log>,
}

impl ClipboardSink for FakeSink {
    fn write(&self, request: &WriteRequest) -> std::result::Result<i64, ()> {
        if self.board.fail_writes.load(Ordering::SeqCst) {
            return Err(());
        }
        let what = if let Some(t) = &request.text {
            // Rich flavors are named, so a plain write that keeps one shows.
            let html = if request.html.is_some() { " +html" } else { "" };
            let rtf = if request.rtf.is_some() { " +rtf" } else { "" };
            format!("text:{t}{html}{rtf}")
        } else if request.png.is_some() {
            "image".to_string()
        } else {
            format!("files:{}", request.files.join(","))
        };
        self.log.push(format!("write {what}"));
        *self.board.written.lock().unwrap() = Some(request.clone());
        *self.board.formats.lock().unwrap() = vec![SELF_MARKER_TYPE.to_string()];
        Ok(self.board.change.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

// ---------------------------------------------------------------- policy

pub struct FakePolicy {
    pub log: Arc<Log>,
    pub allow: AtomicBool,
}

impl Policy for FakePolicy {
    fn should_capture(&self, ctx: &CaptureContext<'_>, settings: &Settings) -> bool {
        self.log.push(format!(
            "gate app={} formats={} self={}",
            ctx.source_app_id.unwrap_or("-"),
            ctx.format_names.join("|"),
            ctx.written_by_self
        ));
        !settings.capture_paused && !ctx.written_by_self && self.allow.load(Ordering::SeqCst)
    }
    fn classify_text(&self, text: &str) -> ClipKind {
        if text.starts_with("http") { ClipKind::Link } else { ClipKind::Text }
    }
}

// ---------------------------------------------------------------- shelf, shortcuts, clock

pub struct FakeShelf(pub Arc<Log>);

impl ShelfControl for FakeShelf {
    fn hide(&self) {
        self.0.push("hide shelf");
    }

    fn dismiss(&self) {
        self.0.push("dismiss shelf");
    }
}

pub struct FakeSettingsWindow(pub Arc<Log>);

impl SettingsWindowControl for FakeSettingsWindow {
    fn open(&self, return_to: Option<i32>) {
        match return_to {
            Some(pid) => self.0.push(format!("open settings (return to {pid})")),
            None => self.0.push("open settings"),
        }
    }
    fn close(&self) {
        self.0.push("close settings");
    }
}

pub struct FakeShortcuts {
    pub log: Arc<Log>,
    pub refuse: Mutex<Option<String>>,
}

impl ShortcutRegistrar for FakeShortcuts {
    fn register(&self, accelerator: &str) -> CmdResult<()> {
        self.log.push(format!("register {accelerator}"));
        if self.refuse.lock().unwrap().as_deref() == Some(accelerator) {
            return Err("shortcut is unavailable".into());
        }
        Ok(())
    }
    fn unregister(&self, accelerator: &str) {
        self.log.push(format!("unregister {accelerator}"));
    }
}

pub struct FakeClock {
    pub now: AtomicI64,
    pub log: Arc<Log>,
    /// Actions run when the clock sleeps (simulate the OS reacting).
    pub on_sleep: Mutex<VecDeque<Box<dyn FnOnce() + Send>>>,
}

impl Clock for FakeClock {
    fn now(&self) -> Millis {
        self.now.load(Ordering::SeqCst)
    }
    fn sleep(&self, duration: Duration) {
        self.log.push(format!("sleep {}", duration.as_millis()));
        self.now.fetch_add(duration.as_millis() as i64, Ordering::SeqCst);
        if let Some(action) = self.on_sleep.lock().unwrap().pop_front() {
            action();
        }
    }
}

// ---------------------------------------------------------------- harness

/// Installed: TextEdit, Vault, Notes (twice); running: Notes.
pub struct FakeApps {
    pub log: Arc<Log>,
}

fn fake_app(id: &str, name: &str, running: bool) -> AppInfo {
    AppInfo { id: id.into(), name: name.into(), running }
}

impl AppCatalog for FakeApps {
    fn installed_apps(&self) -> Vec<AppInfo> {
        vec![
            fake_app("com.example.vault", "Vault", false),
            fake_app("com.apple.TextEdit", "TextEdit", false),
            fake_app("com.apple.Notes", "Notes", false),
            fake_app("com.apple.notes", "Notes", false),
        ]
    }
    fn running_apps(&self) -> Vec<AppInfo> {
        vec![fake_app("com.apple.Notes", "Notes", true)]
    }
    fn find_app(&self, id: &str) -> Option<AppInfo> {
        self.installed_apps().into_iter().find(|a| a.id.eq_ignore_ascii_case(id))
    }
    fn icon_png(&self, id: &str, px: u32) -> Option<Vec<u8>> {
        self.log.push(format!("icon {id} {px}"));
        self.find_app(id).map(|a| format!("png:{}", a.id).into_bytes())
    }
    fn choose_app(&self) -> Option<AppInfo> {
        Some(fake_app("com.example.chosen", "Chosen", false))
    }
}

/// Login items: starts offered and not registered.
pub struct FakeLoginItems {
    status: Mutex<Status>,
    unavailable: Mutex<Option<Unavailable>>,
    fail: AtomicBool,
    calls: Mutex<Vec<bool>>,
}

impl Default for FakeLoginItems {
    fn default() -> Self {
        Self {
            status: Mutex::new(Status::NotRegistered),
            unavailable: Mutex::new(None),
            fail: AtomicBool::new(false),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl FakeLoginItems {
    pub fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status;
    }
    pub fn set_unavailable(&self, why: Option<Unavailable>) {
        *self.unavailable.lock().unwrap() = why;
    }
    /// Every later register or unregister fails.
    pub fn fail(&self) {
        self.fail.store(true, Ordering::SeqCst);
    }
    /// What was asked: true to register, false to unregister.
    pub fn calls(&self) -> Vec<bool> {
        self.calls.lock().unwrap().clone()
    }
}

impl LoginItems for FakeLoginItems {
    fn unavailable(&self) -> Option<Unavailable> {
        *self.unavailable.lock().unwrap()
    }
    fn status(&self) -> Option<Status> {
        Some(*self.status.lock().unwrap())
    }
    fn set(&self, on: bool) -> std::result::Result<(), ()> {
        self.calls.lock().unwrap().push(on);
        if self.fail.load(Ordering::SeqCst) {
            return Err(());
        }
        self.set_status(if on { Status::Enabled } else { Status::NotRegistered });
        Ok(())
    }
}

pub struct Harness {
    /// "Open at login".
    pub login: Arc<FakeLoginItems>,
    pub ctx: Arc<Ctx>,
    pub log: Arc<Log>,
    pub board: Arc<FakeBoard>,
    pub desktop: Arc<FakeDesktop>,
    pub policy: Arc<FakePolicy>,
    pub clock: Arc<FakeClock>,
    pub shortcuts: Arc<FakeShortcuts>,
    /// How many times the menu bar animation was asked for.
    pub clips_saved: Arc<AtomicUsize>,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_store(FakeStore::default())
    }

    pub fn with_store(store: FakeStore) -> Self {
        Self::build(Box::new(store), false)
    }

    /// The real SQLite store (in memory) and the real core policy.
    pub fn with_real_core() -> Self {
        let store = satchel_core::Store::open_in_memory().expect("in-memory store");
        Self::build(Box::new(store), true)
    }

    fn build(store: Box<dyn ClipStore>, real_policy: bool) -> Self {
        let log = Arc::new(Log::default());
        let board = Arc::new(FakeBoard::default());
        let desktop = Arc::new(FakeDesktop {
            log: log.clone(),
            front: Mutex::new(Some(FrontApp {
                pid: 42,
                id: "com.apple.TextEdit".into(),
                name: "TextEdit".into(),
            })),
            trusted: AtomicBool::new(true),
            activation_works: AtomicBool::new(true),
        });
        let policy = Arc::new(FakePolicy { log: log.clone(), allow: AtomicBool::new(true) });
        let clock = Arc::new(FakeClock {
            now: AtomicI64::new(1_000_000),
            log: log.clone(),
            on_sleep: Mutex::new(VecDeque::new()),
        });
        let shortcuts = Arc::new(FakeShortcuts { log: log.clone(), refuse: Mutex::new(None) });
        let clips_saved = Arc::new(AtomicUsize::new(0));
        let login = Arc::new(FakeLoginItems::default());
        let ctx = Arc::new(Ctx {
            store: SharedStore::new(store),
            settings: RwLock::new(sample_settings()),
            self_writes: SelfWrites::default(),
            events: Arc::new(FakeEvents(log.clone(), clips_saved.clone())),
            desktop: desktop.clone(),
            clipboard: Arc::new(FakeSink { board: board.clone(), log: log.clone() }),
            policy: if real_policy {
                Arc::new(crate::ctx::CorePolicy)
            } else {
                policy.clone()
            },
            shelf: Arc::new(FakeShelf(log.clone())),
            settings_window: Arc::new(FakeSettingsWindow(log.clone())),
            shortcuts: shortcuts.clone(),
            clock: clock.clone(),
            apps: Arc::new(FakeApps { log: log.clone() }),
            app_icons: Default::default(),
            previous_app: Mutex::new(None),
            paste_lock: Mutex::new(()),
            accessibility_prompted: AtomicBool::new(false),
            link_wake: Default::default(),
            login_items: login.clone(),
        });
        Self { login, ctx, log, board, desktop, policy, clock, shortcuts, clips_saved }
    }

    pub fn source(&self) -> FakeSource {
        FakeSource { board: self.board.clone(), log: self.log.clone() }
    }

    /// Inserts a clip straight into the store and returns its id.
    pub fn seed(&self, kind: ClipKind, text: &str) -> ClipId {
        self.seed_clip(NewClip {
            kind,
            text: Some(text.into()),
            html: None,
            rtf: None,
            image_png: None,
            thumbnail_png: None,
            file_paths: vec![],
            source_app: None,
        })
    }

    /// A text clip with HTML and RTF, as copied from a rich text editor.
    pub fn seed_rich(&self, text: &str) -> ClipId {
        self.seed_clip(NewClip {
            kind: ClipKind::Text,
            text: Some(text.into()),
            html: Some(format!("<b>{text}</b>")),
            rtf: Some(format!("{{\\rtf1 \\b {text}}}").into_bytes()),
            image_png: None,
            thumbnail_png: None,
            file_paths: vec![],
            source_app: None,
        })
    }

    /// A file clip of `paths`.
    pub fn seed_files(&self, paths: &[&str]) -> ClipId {
        self.seed_clip(NewClip {
            kind: ClipKind::File,
            text: None,
            html: None,
            rtf: None,
            image_png: None,
            thumbnail_png: None,
            file_paths: paths.iter().map(|p| p.to_string()).collect(),
            source_app: None,
        })
    }

    fn seed_clip(&self, clip: NewClip) -> ClipId {
        let now = self.clock.now();
        self.clock.now.fetch_add(1, Ordering::SeqCst);
        match self.ctx.store.lock().insert(clip, now).unwrap() {
            InsertOutcome::Inserted(s) | InsertOutcome::Bumped(s) => s.id,
        }
    }
}
