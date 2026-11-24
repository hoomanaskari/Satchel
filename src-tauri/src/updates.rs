//! Automatic updates for the installed Satchel (never Satchel Dev).
//!
//! Tauri's updater plugin (`tauri-plugin-updater`) does the network and file
//! work, all of it in Rust: the web view is given no updater permission and
//! never reaches the network. [`Updates`] is the state machine around it:
//!
//! - **Checks.** With "Check for updates automatically" on, one check
//!   `FIRST_CHECK_AFTER_MS` after launch, then every `CHECK_EVERY_MS`
//!   (6 hours; `RETRY_AFTER_FAILURE_MS` after a failed one). A check is one
//!   GET of `latest.json` from the newest GitHub Release (the endpoint in
//!   `tauri.conf.json`, with no variables in it), sent with only the
//!   plugin's `Accept` and `User-Agent: tauri-plugin-updater/<version>`
//!   headers. With the setting off the scheduler sleeps until it is turned
//!   back on, so no background request is ever made; a manual check
//!   ([`Updates::check_now`]) still works.
//! - **Install**, only when the user chooses it: download the versioned
//!   archive, verify its minisign signature against the public key in the
//!   config, which must also sign its version (`requireSignedVersion`),
//!   replace the app bundle in place and relaunch. A failure leaves the
//!   offer up with a content-free reason (download, verify, install).
//! - Nothing announces itself: state changes go to the tray (badge and menu
//!   items) and to open windows as `update-status`; no window is shown and
//!   focus is never taken. The relaunch waits for a paste in progress.
//!
//! The source of updates sits behind [`UpdateSource`], so this logic is
//! tested with fakes; [`PluginSource`] is the real one.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Serialize;

use satchel_core::Millis;

use crate::platform::Clock;

/// Event sent to every window when the status changes.
pub const UPDATE_STATUS: &str = "update-status";

/// First automatic check, this long after launch (startup settles first).
pub const FIRST_CHECK_AFTER_MS: Millis = 5_000;
/// Automatic checks after a completed one.
pub const CHECK_EVERY_MS: Millis = 6 * 60 * 60 * 1000;
/// Automatic checks after one that failed (offline, the feed unreachable).
pub const RETRY_AFTER_FAILURE_MS: Millis = 60 * 60 * 1000;

/// Error returned to the UI when this Satchel does not update (Satchel Dev).
pub const UNAVAILABLE: &str = "updates are not available in this build";

/// What stopped an install. Deliberately coarse: no URL, path or error text.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Failure {
    /// The archive could not be fetched (offline, server error, timeout).
    Download,
    /// The archive's signature or signed version did not match the key.
    Verify,
    /// The new app could not be put in place of this one.
    Install,
}

/// Where an available update stands.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Phase {
    /// No newer version known.
    Idle,
    /// A newer version is offered.
    Available { version: String },
    /// Downloading it: bytes so far and the total when the server says.
    Downloading { version: String, downloaded: u64, total: Option<u64> },
    /// Verified and downloaded; replacing the app and relaunching.
    Installing { version: String },
    /// The last attempt failed; it can be tried again.
    Failed { version: String, during: Failure },
}

impl Phase {
    /// The version on offer, if any.
    pub fn version(&self) -> Option<&str> {
        match self {
            Phase::Idle => None,
            Phase::Available { version }
            | Phase::Downloading { version, .. }
            | Phase::Installing { version }
            | Phase::Failed { version, .. } => Some(version),
        }
    }

    fn busy(&self) -> bool {
        matches!(self, Phase::Downloading { .. } | Phase::Installing { .. })
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CheckOutcome {
    UpToDate,
    Available,
    Failed,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LastCheck {
    pub at: Millis,
    pub outcome: CheckOutcome,
}

/// Everything the tray, the shelf and the settings window show.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// False in Satchel Dev: no checks, no controls.
    pub supported: bool,
    pub current_version: String,
    /// A check is in flight (kept apart from `phase`, so a background check
    /// never hides an offer that is already shown).
    pub checking: bool,
    pub phase: Phase,
    /// The last check that finished in this run; `None` before the first.
    pub last_check: Option<LastCheck>,
}

/// Download progress reported by an [`UpdateSource`].
pub enum Progress {
    Chunk { downloaded: u64, total: Option<u64> },
    /// The archive is in and its signature verified; installing next.
    Verified,
}

/// Where updates come from: Tauri's updater, or a fake in tests. Calls
/// block; they run on Satchel's own threads, never the main thread.
pub trait UpdateSource: Send + Sync {
    /// Asks the feed once. `Ok(Some(version))` when it offers a newer one,
    /// which is kept for [`UpdateSource::install`].
    fn check(&self) -> Result<Option<String>, ()>;
    /// Downloads, verifies and installs what the last check found.
    fn install(&self, progress: &mut dyn FnMut(Progress)) -> Result<(), Failure>;
    /// Relaunches into the installed version. Does not return on success.
    fn restart(&self);
}

/// Told about every status change (the tray and the windows).
pub trait UpdateEvents: Send + Sync {
    fn status_changed(&self, status: &UpdateStatus);
}

struct Inner {
    status: UpdateStatus,
    /// "Check for updates automatically".
    auto: bool,
    /// When the next automatic check is due.
    next_due: Millis,
    /// Last progress sent, so a download emits at most once per percent.
    sent_percent: Option<u64>,
}

pub struct Updates {
    inner: Mutex<Inner>,
    wake: Condvar,
    source: Arc<dyn UpdateSource>,
    events: Arc<dyn UpdateEvents>,
    clock: Arc<dyn Clock>,
}

impl Updates {
    pub fn new(
        supported: bool,
        current_version: String,
        auto: bool,
        source: Arc<dyn UpdateSource>,
        events: Arc<dyn UpdateEvents>,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self> {
        let now = clock.now();
        Arc::new(Self {
            inner: Mutex::new(Inner {
                status: UpdateStatus { supported, current_version, checking: false, phase: Phase::Idle, last_check: None },
                auto,
                next_due: now.saturating_add(FIRST_CHECK_AFTER_MS),
                sent_percent: None,
            }),
            wake: Condvar::new(),
            source,
            events,
            clock,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn status(&self) -> UpdateStatus {
        self.lock().status.clone()
    }

    fn changed(&self, inner: std::sync::MutexGuard<'_, Inner>) {
        let status = inner.status.clone();
        drop(inner);
        self.events.status_changed(&status);
    }

    /// "Check for updates automatically" was turned on or off.
    pub fn set_auto(&self, on: bool) {
        let mut inner = self.lock();
        if inner.auto != on {
            inner.auto = on;
            self.wake.notify_all();
        }
    }

    /// A check the user asked for. Runs in the background; the result
    /// arrives as a status change.
    pub fn check_now(self: &Arc<Self>) -> Result<(), &'static str> {
        if !self.lock().status.supported {
            return Err(UNAVAILABLE);
        }
        let this = self.clone();
        std::thread::Builder::new()
            .name("satchel-update-check".into())
            .spawn(move || this.run_check())
            .map(|_| ())
            .map_err(|_| crate::error::INTERNAL)
    }

    /// One check, blocking. Skipped while another check or an install runs.
    pub fn run_check(&self) {
        {
            let mut inner = self.lock();
            if !inner.status.supported || inner.status.checking || inner.status.phase.busy() {
                return;
            }
            inner.status.checking = true;
            self.changed(inner);
        }
        let result = self.source.check();
        let now = self.clock.now();
        let mut inner = self.lock();
        inner.status.checking = false;
        let outcome = match result {
            Ok(Some(version)) => {
                match &inner.status.phase {
                    // An install started meanwhile, or this version already
                    // failed and keeps its "try again".
                    p if p.busy() => {}
                    Phase::Failed { version: v, .. } if *v == version => {}
                    _ => inner.status.phase = Phase::Available { version },
                }
                CheckOutcome::Available
            }
            Ok(None) => {
                if !inner.status.phase.busy() {
                    inner.status.phase = Phase::Idle;
                }
                CheckOutcome::UpToDate
            }
            Err(()) => CheckOutcome::Failed,
        };
        inner.status.last_check = Some(LastCheck { at: now, outcome });
        let wait = if outcome == CheckOutcome::Failed { RETRY_AFTER_FAILURE_MS } else { CHECK_EVERY_MS };
        inner.next_due = now.saturating_add(wait);
        self.changed(inner);
    }

    /// The user chose to update: downloads, verifies, installs and
    /// relaunches in the background. Only while an update is on offer.
    pub fn install(self: &Arc<Self>) -> Result<(), &'static str> {
        if !self.start_install()? {
            return Ok(());
        }
        let this = self.clone();
        std::thread::Builder::new()
            .name("satchel-update-install".into())
            .spawn(move || this.run_install())
            .map(|_| ())
            .map_err(|_| crate::error::INTERNAL)
    }

    /// Moves an offered update to `Downloading`. False when there is
    /// nothing to install or an install already runs.
    fn start_install(&self) -> Result<bool, &'static str> {
        let mut inner = self.lock();
        if !inner.status.supported {
            return Err(UNAVAILABLE);
        }
        let version = match &inner.status.phase {
            Phase::Available { version } | Phase::Failed { version, .. } => version.clone(),
            _ => return Ok(false),
        };
        inner.status.phase = Phase::Downloading { version, downloaded: 0, total: None };
        inner.sent_percent = None;
        self.changed(inner);
        Ok(true)
    }

    fn run_install(&self) {
        let result = self.source.install(&mut |p| self.progress(p));
        let mut inner = self.lock();
        let version = inner.status.phase.version().unwrap_or_default().to_string();
        match result {
            Ok(()) => {
                if !matches!(inner.status.phase, Phase::Installing { .. }) {
                    inner.status.phase = Phase::Installing { version };
                    self.changed(inner);
                } else {
                    drop(inner);
                }
                self.source.restart();
            }
            Err(during) => {
                eprintln!("satchel: the update failed ({during:?})");
                inner.status.phase = Phase::Failed { version, during };
                self.changed(inner);
            }
        }
    }

    fn progress(&self, p: Progress) {
        let mut inner = self.lock();
        let version = inner.status.phase.version().unwrap_or_default().to_string();
        match p {
            Progress::Chunk { downloaded, total } => {
                inner.status.phase = Phase::Downloading { version, downloaded, total };
                // At most one event per percent (or per MiB without a total).
                let step = match total {
                    Some(t) if t > 0 => downloaded.saturating_mul(100) / t,
                    _ => downloaded >> 20,
                };
                if inner.sent_percent == Some(step) {
                    return;
                }
                inner.sent_percent = Some(step);
            }
            Progress::Verified => inner.status.phase = Phase::Installing { version },
        }
        self.changed(inner);
    }

    /// How long the scheduler waits before its next automatic check, or
    /// `None` to sleep until woken (automatic checks off).
    fn wait_for(inner: &Inner, now: Millis) -> Option<Millis> {
        inner.auto.then(|| inner.next_due.saturating_sub(now).max(0))
    }

    /// Starts the background scheduler (installed Satchel only). It sleeps
    /// on a condition variable between checks: no polling.
    pub fn spawn_scheduler(self: &Arc<Self>) -> std::io::Result<()> {
        if !self.lock().status.supported {
            return Ok(());
        }
        let this = self.clone();
        std::thread::Builder::new().name("satchel-updates".into()).spawn(move || {
            let mut inner = this.lock();
            loop {
                match Self::wait_for(&inner, this.clock.now()) {
                    Some(0) => {
                        // A check already in flight (a manual one) moves
                        // `next_due` itself when it finishes.
                        let busy = inner.status.checking || inner.status.phase.busy();
                        if busy {
                            inner.next_due = this.clock.now().saturating_add(RETRY_AFTER_FAILURE_MS);
                        }
                        drop(inner);
                        if !busy {
                            this.run_check();
                        }
                        inner = this.lock();
                    }
                    Some(ms) => {
                        let wait = Duration::from_millis(u64::try_from(ms).unwrap_or(0));
                        inner = this.wake.wait_timeout(inner, wait).unwrap_or_else(|p| p.into_inner()).0;
                    }
                    None => inner = this.wake.wait(inner).unwrap_or_else(|p| p.into_inner()),
                }
            }
        })?;
        Ok(())
    }
}

/// Tauri's updater plugin as an [`UpdateSource`].
pub struct PluginSource<R: tauri::Runtime> {
    pub app: tauri::AppHandle<R>,
    /// What the last check found, kept to install.
    pub found: Mutex<Option<tauri_plugin_updater::Update>>,
    /// Its paste lock is held while relaunching, so a paste in progress
    /// finishes first.
    pub ctx: Arc<crate::ctx::Ctx>,
}

/// Longest a check may take, and a download.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

impl<R: tauri::Runtime> UpdateSource for PluginSource<R> {
    fn check(&self) -> Result<Option<String>, ()> {
        use tauri_plugin_updater::UpdaterExt;
        let updater = self.app.updater_builder().timeout(CHECK_TIMEOUT).build().map_err(|_| ())?;
        let found = tauri::async_runtime::block_on(updater.check()).map_err(|_| ())?;
        let version = found.as_ref().map(|u| u.version.clone());
        *self.found.lock().unwrap_or_else(|p| p.into_inner()) = found;
        Ok(version)
    }

    fn install(&self, progress: &mut dyn FnMut(Progress)) -> Result<(), Failure> {
        use tauri_plugin_updater::Error;
        let Some(mut update) = self.found.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
            return Err(Failure::Download);
        };
        update.timeout = Some(DOWNLOAD_TIMEOUT);
        let mut downloaded = 0u64;
        let bytes = tauri::async_runtime::block_on(update.download(
            |chunk, total| {
                downloaded = downloaded.saturating_add(chunk as u64);
                progress(Progress::Chunk { downloaded, total });
            },
            || {},
        ))
        .map_err(|e| match e {
            Error::Minisign(_)
            | Error::Base64(_)
            | Error::SignatureUtf8(_)
            | Error::SignedVersionMismatch { .. }
            | Error::MissingSignedVersion => Failure::Verify,
            _ => Failure::Download,
        })?;
        progress(Progress::Verified);
        update.install(bytes).map_err(|_| Failure::Install)
    }

    fn restart(&self) {
        let _paste = self.ctx.paste_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.app.restart();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct FakeClock(AtomicI64);
    impl Clock for FakeClock {
        fn now(&self) -> Millis {
            self.0.load(Ordering::SeqCst)
        }
        fn sleep(&self, _: Duration) {}
    }

    #[derive(Default)]
    struct FakeSource {
        /// `None`: up to date. `Some(None)`: the check fails.
        offer: Mutex<Option<Option<String>>>,
        install: Mutex<Option<Result<(), Failure>>>,
        restarts: Mutex<u32>,
        checks: Mutex<u32>,
    }
    impl UpdateSource for FakeSource {
        fn check(&self) -> Result<Option<String>, ()> {
            *self.checks.lock().unwrap() += 1;
            match self.offer.lock().unwrap().clone() {
                None => Ok(None),
                Some(None) => Err(()),
                Some(Some(v)) => Ok(Some(v)),
            }
        }
        fn install(&self, progress: &mut dyn FnMut(Progress)) -> Result<(), Failure> {
            for d in [0, 25, 50, 50, 51, 100] {
                progress(Progress::Chunk { downloaded: d, total: Some(100) });
            }
            let result = self.install.lock().unwrap().unwrap_or(Ok(()));
            if result.is_ok() {
                progress(Progress::Verified);
            }
            result
        }
        fn restart(&self) {
            *self.restarts.lock().unwrap() += 1;
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<UpdateStatus>>);
    impl UpdateEvents for Recorder {
        fn status_changed(&self, status: &UpdateStatus) {
            self.0.lock().unwrap().push(status.clone());
        }
    }

    struct H {
        updates: Arc<Updates>,
        source: Arc<FakeSource>,
        events: Arc<Recorder>,
        clock: Arc<FakeClock>,
    }

    fn harness(supported: bool) -> H {
        let source = Arc::new(FakeSource::default());
        let events = Arc::new(Recorder::default());
        let clock = Arc::new(FakeClock(AtomicI64::new(1_000)));
        let updates = Updates::new(supported, "0.1.1".into(), true, source.clone(), events.clone(), clock.clone());
        H { updates, source, events, clock }
    }

    fn offer(h: &H, v: Result<Option<&str>, ()>) {
        *h.source.offer.lock().unwrap() = match v {
            Ok(None) => None,
            Err(()) => Some(None),
            Ok(Some(v)) => Some(Some(v.to_string())),
        };
    }

    #[test]
    fn a_check_finds_an_update_and_says_so_while_it_runs() {
        let h = harness(true);
        offer(&h, Ok(Some("0.1.2")));
        h.clock.0.store(50_000, Ordering::SeqCst);
        h.updates.run_check();
        let seen = h.events.0.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].checking && seen[0].phase == Phase::Idle);
        let s = &seen[1];
        assert!(!s.checking);
        assert_eq!(s.phase, Phase::Available { version: "0.1.2".into() });
        assert_eq!(s.last_check, Some(LastCheck { at: 50_000, outcome: CheckOutcome::Available }));
        assert_eq!(h.updates.lock().next_due, 50_000 + CHECK_EVERY_MS);
    }

    #[test]
    fn a_failed_check_keeps_the_offer_and_retries_sooner() {
        let h = harness(true);
        offer(&h, Ok(Some("0.1.2")));
        h.updates.run_check();
        offer(&h, Err(()));
        h.updates.run_check();
        let s = h.updates.status();
        assert_eq!(s.phase, Phase::Available { version: "0.1.2".into() });
        assert_eq!(s.last_check.unwrap().outcome, CheckOutcome::Failed);
        assert_eq!(h.updates.lock().next_due, 1_000 + RETRY_AFTER_FAILURE_MS);
    }

    #[test]
    fn up_to_date_clears_nothing_but_an_offer() {
        let h = harness(true);
        h.updates.run_check();
        let s = h.updates.status();
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.last_check.unwrap().outcome, CheckOutcome::UpToDate);
    }

    #[test]
    fn install_downloads_with_throttled_progress_then_relaunches() {
        let h = harness(true);
        offer(&h, Ok(Some("0.1.2")));
        h.updates.run_check();
        h.events.0.lock().unwrap().clear();
        assert!(h.updates.start_install().unwrap());
        h.updates.run_install();
        let phases: Vec<Phase> = h.events.0.lock().unwrap().iter().map(|s| s.phase.clone()).collect();
        let v = || "0.1.2".to_string();
        assert_eq!(
            phases,
            vec![
                Phase::Downloading { version: v(), downloaded: 0, total: None },
                Phase::Downloading { version: v(), downloaded: 0, total: Some(100) },
                Phase::Downloading { version: v(), downloaded: 25, total: Some(100) },
                Phase::Downloading { version: v(), downloaded: 50, total: Some(100) },
                Phase::Downloading { version: v(), downloaded: 51, total: Some(100) },
                Phase::Downloading { version: v(), downloaded: 100, total: Some(100) },
                Phase::Installing { version: v() },
            ]
        );
        assert_eq!(*h.source.restarts.lock().unwrap(), 1);
    }

    #[test]
    fn a_rejected_archive_fails_without_relaunching_and_can_be_retried() {
        let h = harness(true);
        offer(&h, Ok(Some("0.1.2")));
        h.updates.run_check();
        *h.source.install.lock().unwrap() = Some(Err(Failure::Verify));
        assert!(h.updates.start_install().unwrap());
        h.updates.run_install();
        assert_eq!(h.updates.status().phase, Phase::Failed { version: "0.1.2".into(), during: Failure::Verify });
        assert_eq!(*h.source.restarts.lock().unwrap(), 0);
        // A later check of the same version keeps "try again"; installing again works.
        h.updates.run_check();
        assert!(matches!(h.updates.status().phase, Phase::Failed { .. }));
        assert!(h.updates.start_install().unwrap());
    }

    #[test]
    fn nothing_to_install_is_a_no_op_and_checks_wait_for_an_install() {
        let h = harness(true);
        assert!(!h.updates.start_install().unwrap());
        offer(&h, Ok(Some("0.1.2")));
        h.updates.run_check();
        assert!(h.updates.start_install().unwrap());
        assert!(!h.updates.start_install().unwrap(), "already downloading");
        let checks = *h.source.checks.lock().unwrap();
        h.updates.run_check();
        assert_eq!(*h.source.checks.lock().unwrap(), checks, "no check during an install");
    }

    #[test]
    fn satchel_dev_never_checks_or_installs() {
        let h = harness(false);
        h.updates.run_check();
        assert_eq!(*h.source.checks.lock().unwrap(), 0);
        assert!(h.events.0.lock().unwrap().is_empty());
        assert_eq!(h.updates.check_now(), Err(UNAVAILABLE));
        assert_eq!(h.updates.install(), Err(UNAVAILABLE));
        assert!(!h.updates.status().supported);
        h.updates.spawn_scheduler().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*h.source.checks.lock().unwrap(), 0);
    }

    #[test]
    fn the_scheduler_waits_for_the_due_time_and_sleeps_while_off() {
        let h = harness(true);
        let inner = h.updates.lock();
        assert_eq!(Updates::wait_for(&inner, 1_000), Some(FIRST_CHECK_AFTER_MS));
        assert_eq!(Updates::wait_for(&inner, 1_000 + FIRST_CHECK_AFTER_MS + 9), Some(0));
        drop(inner);
        h.updates.set_auto(false);
        assert_eq!(Updates::wait_for(&h.updates.lock(), 1_000_000), None);
    }

    #[test]
    fn with_automatic_checks_off_the_scheduler_makes_no_request() {
        let h = harness(true);
        h.updates.set_auto(false);
        h.clock.0.store(i64::MAX / 2, Ordering::SeqCst); // long past due
        h.updates.spawn_scheduler().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(*h.source.checks.lock().unwrap(), 0);
        // Turned on: the overdue check runs at once.
        h.updates.set_auto(true);
        for _ in 0..100 {
            if *h.source.checks.lock().unwrap() > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(*h.source.checks.lock().unwrap(), 1);
    }

    #[test]
    fn status_serializes_for_the_ui() {
        let h = harness(true);
        let json = serde_json::to_value(h.updates.status()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "supported": true, "currentVersion": "0.1.1", "checking": false,
                "phase": { "state": "idle" }, "lastCheck": null
            })
        );
        let p = Phase::Downloading { version: "0.1.2".into(), downloaded: 3, total: None };
        assert_eq!(
            serde_json::to_value(p).unwrap(),
            serde_json::json!({ "state": "downloading", "version": "0.1.2", "downloaded": 3, "total": null })
        );
        let f = Phase::Failed { version: "0.1.2".into(), during: Failure::Verify };
        assert_eq!(serde_json::to_value(f).unwrap(), serde_json::json!({ "state": "failed", "version": "0.1.2", "during": "verify" }));
        let c = LastCheck { at: 5, outcome: CheckOutcome::UpToDate };
        assert_eq!(serde_json::to_value(c).unwrap(), serde_json::json!({ "at": 5, "outcome": "upToDate" }));
    }
}
