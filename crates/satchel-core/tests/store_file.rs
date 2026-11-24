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
        rtf: None,
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

/// A 3x2 RGBA PNG (signature + IHDR is all the store reads).
fn tiny_png() -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    png.extend_from_slice(&3u32.to_be_bytes());
    png.extend_from_slice(&2u32.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png
}

#[test]
fn images_files_and_rich_text_survive_reopen() {
    let tmp = TempDir::new();
    let db = tmp.path().join("satchel.db");
    let rtf = b"{\\rtf1 \\b bold}".to_vec();
    let (img, rich, files) = {
        let mut s = Store::open(&db).unwrap();
        let id = |o| match o {
            satchel_core::InsertOutcome::Inserted(c) => c.id,
            other => panic!("{other:?}"),
        };
        let img = id(s
            .insert(
                NewClip {
                    kind: ClipKind::Image,
                    text: None,
                    image_png: Some(tiny_png()),
                    thumbnail_png: Some(tiny_png()),
                    ..clip("")
                },
                1,
            )
            .unwrap());
        let rich = id(s
            .insert(NewClip { html: Some("<b>bold</b>".into()), rtf: Some(rtf.clone()), ..clip("bold") }, 2)
            .unwrap());
        let files = id(s
            .insert(
                NewClip {
                    kind: ClipKind::File,
                    text: None,
                    file_paths: vec!["/a/b.txt".into(), "/c/folder".into()],
                    ..clip("")
                },
                3,
            )
            .unwrap());
        (img, rich, files)
    };
    let s = Store::open(&db).unwrap();
    let c = s.content(img).unwrap().unwrap();
    assert_eq!(c.image_png, Some(tiny_png()));
    assert_eq!(s.thumbnail(img).unwrap(), Some(tiny_png()));
    let images = s.list(&Query { kind: Some(ClipKind::Image), ..Query::default() }).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].preview, "3 \u{d7} 2");
    let c = s.content(rich).unwrap().unwrap();
    assert_eq!(c.html.as_deref(), Some("<b>bold</b>"));
    assert_eq!(c.rtf, Some(rtf));
    assert_eq!(s.content(files).unwrap().unwrap().file_paths, vec!["/a/b.txt", "/c/folder"]);
}

#[test]
fn version_1_databases_gain_the_rtf_column() {
    let tmp = TempDir::new();
    let db = tmp.path().join("satchel.db");
    let old = {
        let mut s = Store::open(&db).unwrap();
        match s.insert(clip("from version 1"), 1).unwrap() {
            satchel_core::InsertOutcome::Inserted(c) => c.id,
            other => panic!("{other:?}"),
        }
    };
    {
        // Put the file back the way schema version 1 left it.
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(&format!("{UNDO_VERSION_3}\nALTER TABLE clip_data DROP COLUMN rtf; PRAGMA user_version = 1;"))
            .unwrap();
    }
    let mut s = Store::open(&db).unwrap();
    let c = s.content(old).unwrap().unwrap();
    assert_eq!(c.text.as_deref(), Some("from version 1"));
    assert_eq!(c.rtf, None);
    let rich = match s.insert(NewClip { rtf: Some(b"{\\rtf1 x}".to_vec()), ..clip("new") }, 2).unwrap() {
        satchel_core::InsertOutcome::Inserted(c) => c.id,
        other => panic!("{other:?}"),
    };
    assert_eq!(s.content(rich).unwrap().unwrap().rtf.as_deref(), Some(&b"{\\rtf1 x}"[..]));
}

#[test]
fn version_2_databases_gain_link_previews_with_the_setting_on() {
    let tmp = TempDir::new();
    let db = tmp.path().join("satchel.db");
    {
        let mut s = Store::open(&db).unwrap();
        s.insert(NewClip { kind: ClipKind::Link, ..clip("https://www.example.com/old") }, 1).unwrap();
        s.insert(clip("searchable words"), 2).unwrap();
        s.save_settings(&Settings { max_items: 500, ..Settings::default() }).unwrap();
    }
    {
        // Put the file back the way schema version 2 left it: the version 2
        // view and triggers, no preview tables, no `link_previews` setting.
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            &format!("{UNDO_VERSION_3}\n            DELETE FROM settings WHERE key = 'link_previews';
            PRAGMA user_version = 2;
            "),
        )
        .unwrap();
    }
    let mut s = Store::open(&db).unwrap();
    let settings = s.settings().unwrap();
    assert_eq!(settings.max_items, 500);
    assert!(settings.link_previews, "settings saved before the field existed read as on");
    let old = s.list(&Query::default()).unwrap();
    assert_eq!(old.len(), 2);
    assert!(old.iter().all(|c| c.link_preview.is_none()));
    let link = old.iter().find(|c| c.kind == ClipKind::Link).unwrap().id;
    assert!(s.queue_link_preview(link, 3).unwrap());
    // The existing index still matches the new view (no rebuild needed).
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("INSERT INTO clips_fts (clips_fts, rank) VALUES ('integrity-check', 1)", []).unwrap();
    s.set_title(link, Some("renamed")).unwrap();
    assert!(s.delete(link).unwrap());
    conn.execute("INSERT INTO clips_fts (clips_fts, rank) VALUES ('integrity-check', 1)", []).unwrap();
}

#[test]
fn cleared_previews_leave_no_trace_on_disk() {
    let tmp = TempDir::new();
    let db = tmp.path().join("satchel.db");
    let mut s = Store::open(&db).unwrap();
    for i in 0..20 {
        s.insert(clip(&format!("filler {i}")), i).unwrap();
    }
    let id = match s.insert(NewClip { kind: ClipKind::Link, ..clip("https://www.example.com/x") }, 50).unwrap() {
        satchel_core::InsertOutcome::Inserted(c) => c.id,
        other => panic!("{other:?}"),
    };
    s.queue_link_preview(id, 0).unwrap();
    s.claim_link_preview(0, 0).unwrap();
    let title = "quokkapreviewtitle";
    let image = b"pngbytesquokkaimage".to_vec();
    s.finish_link_preview(
        id,
        satchel_core::LinkPreviewOutcome::Ready(satchel_core::FetchedPreview {
            title: Some(title.into()),
            site: "Example".into(),
            image: Some((satchel_core::LinkImageKind::Photo, image.clone())),
        }),
        0,
    )
    .unwrap();
    assert!(contains(&file_bytes(tmp.path()), title.as_bytes()), "sanity: on disk");
    assert_eq!(s.clear_link_previews().unwrap(), 1);
    let bytes = file_bytes(tmp.path());
    assert!(!contains(&bytes, title.as_bytes()), "cleared title left on disk");
    assert!(!contains(&bytes, &image), "cleared image left on disk");
}

/// Turns a current database back into what schema version 2 left.
const UNDO_VERSION_3: &str = r#"
            DROP TRIGGER clip_data_after_insert;
            DROP TRIGGER clips_before_delete;
            DROP TRIGGER clips_after_title_update;
            DROP TRIGGER link_previews_after_title_update;
            DROP VIEW clips_fts_source;
            DROP TABLE link_preview_images;
            DROP TABLE link_previews;
            CREATE VIEW clips_fts_source AS
                SELECT c.id AS id, COALESCE(d.search_body, d.text, '') AS body, c.title AS title, c.app_name AS app
                FROM clips c JOIN clip_data d ON d.clip_id = c.id;
            CREATE TRIGGER clip_data_after_insert AFTER INSERT ON clip_data BEGIN
                INSERT INTO clips_fts (rowid, body, title, app)
                SELECT new.clip_id, COALESCE(new.search_body, new.text, ''), c.title, c.app_name
                FROM clips c WHERE c.id = new.clip_id;
            END;
            CREATE TRIGGER clips_before_delete BEFORE DELETE ON clips BEGIN
                INSERT INTO clips_fts (clips_fts, rowid, body, title, app)
                SELECT 'delete', old.id, COALESCE(d.search_body, d.text, ''), old.title, old.app_name
                FROM clip_data d WHERE d.clip_id = old.id;
            END;
            CREATE TRIGGER clips_after_title_update AFTER UPDATE OF title ON clips BEGIN
                INSERT INTO clips_fts (clips_fts, rowid, body, title, app)
                SELECT 'delete', old.id, COALESCE(d.search_body, d.text, ''), old.title, old.app_name
                FROM clip_data d WHERE d.clip_id = old.id;
                INSERT INTO clips_fts (rowid, body, title, app)
                SELECT new.id, COALESCE(d.search_body, d.text, ''), new.title, new.app_name
                FROM clip_data d WHERE d.clip_id = new.id;
            END;
"#;
