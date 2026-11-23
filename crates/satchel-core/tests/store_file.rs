//! File-backed `Store`: permissions, persistence and migrations.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use satchel_core::{ClipKind, Error, NewClip, Query, Settings, Store};

/// A unique scratch directory removed on drop (no `tempfile` dependency).
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "satchel-core-test-{}-{nanos}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn clip(text: &str) -> NewClip {
    NewClip {
        kind: ClipKind::Text,
        text: Some(text.into()),
        html: None,
        image_png: None,
        thumbnail_png: None,
        file_paths: vec![],
        source_app: None,
    }
}

#[test]
fn data_persists_across_reopen() {
    let tmp = TempDir::new();
    let db = tmp.path().join("data").join("satchel.db");
    let id = {
        let mut s = Store::open(&db).unwrap();
        let board = s.create_pinboard("Keep", "#010101").unwrap();
        let id = match s.insert(clip("persist me"), 5).unwrap() {
            satchel_core::InsertOutcome::Inserted(c) => c.id,
            other => panic!("{other:?}"),
        };
        s.pin(id, board.id).unwrap();
        s.save_settings(&Settings {
            max_items: 7,
            ..Settings::default()
        })
        .unwrap();
        id
    };
    let s = Store::open(&db).unwrap();
    let rows = s
        .list(&Query {
            text: Some("persist".into()),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].pinboard_ids.len(), 1);
    assert_eq!(s.settings().unwrap().max_items, 7);
}

#[test]
fn schema_is_versioned() {
    let tmp = TempDir::new();
    let db = tmp.path().join("satchel.db");
    drop(Store::open(&db).unwrap());
    let conn = rusqlite::Connection::open(&db).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, satchel_core::store::SCHEMA_VERSION);
    assert!(v >= 1);
    drop(conn);
    // Reopening an up-to-date database is a no-op.
    drop(Store::open(&db).unwrap());
}

#[test]
fn database_from_a_newer_version_is_refused() {
    let tmp = TempDir::new();
    let db = tmp.path().join("satchel.db");
    drop(Store::open(&db).unwrap());
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(
            None,
            "user_version",
            satchel_core::store::SCHEMA_VERSION + 1,
        )
        .unwrap();
    }
    assert!(matches!(Store::open(&db), Err(Error::Invalid(_))));
}

fn file_bytes(dir: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            all.extend(std::fs::read(&p).unwrap());
        }
    }
    all
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn deleted_clips_leave_no_trace_on_disk() {
    let tmp = TempDir::new();
    let db = tmp.path().join("satchel.db");
    let mut s = Store::open(&db).unwrap();
    for i in 0..50 {
        s.insert(clip(&format!("filler clip number {i}")), i)
            .unwrap();
    }
    let secret = "hunter2zebracorn";
    let id = match s.insert(clip(&format!("password {secret}")), 100).unwrap() {
        satchel_core::InsertOutcome::Inserted(c) => c.id,
        other => panic!("{other:?}"),
    };
    let other = "llamaquokkasecret";
    s.insert(clip(other), 101).unwrap();
    assert!(
        contains(&file_bytes(tmp.path()), secret.as_bytes()),
        "sanity: secret is on disk"
    );

    assert!(s.delete(id).unwrap());
    assert!(
        !contains(&file_bytes(tmp.path()), secret.as_bytes()),
        "deleted text left on disk"
    );

    assert_eq!(s.clear_history().unwrap(), 51);
    assert!(
        !contains(&file_bytes(tmp.path()), other.as_bytes()),
        "cleared text left on disk"
    );
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn new_directory_and_file_are_user_only() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("nested").join("satchel");
        let db = dir.join("satchel.db");
        let mut s = Store::open(&db).unwrap();
        s.insert(clip("secret"), 1).unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&db), 0o600);
        for side in ["satchel.db-wal", "satchel.db-shm"] {
            let p = dir.join(side);
            if p.exists() {
                assert_eq!(
                    mode(&p) & 0o077,
                    0,
                    "{side} must not be group/other accessible"
                );
            }
        }
    }

    #[test]
    fn existing_loose_permissions_are_tightened() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("loose");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = dir.join("satchel.db");
        std::fs::write(&db, b"").unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(Store::open(&db).unwrap());
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&db), 0o600);
    }

    #[test]
    fn symlinked_database_file_is_refused() {
        let tmp = TempDir::new();
        let target = tmp.path().join("elsewhere.db");
        std::fs::write(&target, b"").unwrap();
        let link = tmp.path().join("satchel.db");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(Store::open(&link), Err(Error::Invalid(_))));
    }
}
