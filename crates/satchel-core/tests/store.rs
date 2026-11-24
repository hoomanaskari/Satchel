//! Behavior of `Store` through its public API.

use satchel_core::{
    ClipKind, Error, InsertOutcome, MAX_IMAGE_BYTES, MAX_TEXT_BYTES, NewClip, PREVIEW_CHARS, Query,
    Settings, SourceApp, Store,
};

fn text_clip(text: &str) -> NewClip {
    NewClip {
        kind: satchel_core::classify::classify_text(text),
        text: Some(text.to_string()),
        html: None,
        rtf: None,
        image_png: None,
        thumbnail_png: None,
        file_paths: vec![],
        source_app: Some(SourceApp {
            id: "com.apple.Safari".into(),
            name: "Safari".into(),
        }),
    }
}

fn image_clip(bytes: Vec<u8>, thumb: Option<Vec<u8>>) -> NewClip {
    NewClip {
        kind: ClipKind::Image,
        text: None,
        html: None,
        rtf: None,
        image_png: Some(bytes),
        thumbnail_png: thumb,
        file_paths: vec![],
        source_app: None,
    }
}

fn file_clip(paths: &[&str]) -> NewClip {
    NewClip {
        kind: ClipKind::File,
        text: None,
        html: None,
        rtf: None,
        image_png: None,
        thumbnail_png: None,
        file_paths: paths.iter().map(|p| p.to_string()).collect(),
        source_app: Some(SourceApp {
            id: "com.apple.finder".into(),
            name: "Finder".into(),
        }),
    }
}

fn store() -> Store {
    Store::open_in_memory().expect("open in memory")
}

fn insert(s: &mut Store, clip: NewClip, now: i64) -> satchel_core::ClipSummary {
    match s.insert(clip, now).expect("insert") {
        InsertOutcome::Inserted(c) | InsertOutcome::Bumped(c) => c,
    }
}

fn ids(s: &Store, q: &Query) -> Vec<i64> {
    s.list(q).expect("list").into_iter().map(|c| c.id).collect()
}

fn search(s: &Store, text: &str) -> Vec<i64> {
    ids(
        s,
        &Query {
            text: Some(text.into()),
            ..Query::default()
        },
    )
}

// ------------------------------------------------------------- insert

#[test]
fn empty_store_lists_nothing() {
    assert!(store().list(&Query::default()).unwrap().is_empty());
}

#[test]
fn insert_returns_a_summary() {
    let mut s = store();
    let out = s
        .insert(text_clip("  hello\n\n\n\nworld  "), 1_000)
        .unwrap();
    let InsertOutcome::Inserted(c) = out else {
        panic!("expected Inserted")
    };
    assert!(c.id > 0);
    assert_eq!(c.kind, ClipKind::Text);
    assert_eq!(c.title, None);
    assert_eq!(c.preview, "hello\n\nworld");
    assert_eq!(
        c.source_app,
        Some(SourceApp {
            id: "com.apple.Safari".into(),
            name: "Safari".into()
        })
    );
    assert_eq!(c.created_at, 1_000);
    assert_eq!(c.last_used_at, 1_000);
    assert_eq!(c.byte_size, "  hello\n\n\n\nworld  ".len() as u64);
    assert!(!c.has_thumbnail);
    assert!(c.pinboard_ids.is_empty());
    assert_eq!(s.summary(c.id).unwrap(), Some(c));
}

#[test]
fn preview_is_capped() {
    let mut s = store();
    let c = insert(&mut s, text_clip(&"é".repeat(5_000)), 1);
    assert_eq!(c.preview.chars().count(), PREVIEW_CHARS);
}

#[test]
fn identical_content_bumps_instead_of_duplicating() {
    let mut s = store();
    let a = insert(&mut s, text_clip("same"), 1_000);
    let b = insert(&mut s, text_clip("other"), 2_000);
    let out = s.insert(text_clip("same"), 3_000).unwrap();
    let InsertOutcome::Bumped(bumped) = out else {
        panic!("expected Bumped")
    };
    assert_eq!(bumped.id, a.id);
    assert_eq!(bumped.created_at, 1_000);
    assert_eq!(bumped.last_used_at, 3_000);
    assert_eq!(ids(&s, &Query::default()), vec![a.id, b.id]);
}

#[test]
fn same_bytes_as_different_payloads_are_different_clips() {
    let mut s = store();
    let t = insert(&mut s, text_clip("/tmp/a"), 1);
    let f = insert(&mut s, file_clip(&["/tmp/a"]), 2);
    assert_ne!(t.id, f.id);
    let f2 = insert(&mut s, file_clip(&["/tmp/a", "/tmp/b"]), 3);
    assert_ne!(f.id, f2.id);
}

#[test]
fn size_caps_are_enforced() {
    let mut s = store();
    let big = "x".repeat(MAX_TEXT_BYTES as usize + 1);
    assert!(matches!(
        s.insert(text_clip(&big), 1),
        Err(Error::TooLarge { .. })
    ));
    let ok = "x".repeat(MAX_TEXT_BYTES as usize);
    assert!(s.insert(text_clip(&ok), 1).is_ok());

    let img = vec![0u8; MAX_IMAGE_BYTES as usize + 1];
    assert!(matches!(
        s.insert(image_clip(img, None), 1),
        Err(Error::TooLarge { .. })
    ));

    let long_path = "p".repeat(MAX_TEXT_BYTES as usize + 1);
    assert!(matches!(
        s.insert(file_clip(&[&long_path]), 1),
        Err(Error::TooLarge { .. })
    ));
    assert_eq!(s.list(&Query::default()).unwrap().len(), 1);
}

#[test]
fn oversized_html_is_dropped_but_text_kept() {
    let mut s = store();
    let mut clip = text_clip("plain");
    clip.html = Some("h".repeat(MAX_TEXT_BYTES as usize + 1));
    let c = insert(&mut s, clip, 1);
    let content = s.content(c.id).unwrap().unwrap();
    assert_eq!(content.text.as_deref(), Some("plain"));
    assert_eq!(content.html, None);
}

#[test]
fn oversized_thumbnail_is_dropped() {
    let mut s = store();
    let c = insert(
        &mut s,
        image_clip(vec![1, 2, 3], Some(vec![0; 4 * 1024 * 1024])),
        1,
    );
    assert!(!c.has_thumbnail);
    assert_eq!(s.thumbnail(c.id).unwrap(), None);
}

#[test]
fn malformed_clips_are_rejected() {
    let mut s = store();
    let mut no_text = text_clip("x");
    no_text.text = None;
    assert!(matches!(s.insert(no_text, 1), Err(Error::Invalid(_))));
    assert!(matches!(s.insert(text_clip(""), 1), Err(Error::Invalid(_))));
    let mut no_image = image_clip(vec![1], None);
    no_image.image_png = None;
    assert!(matches!(s.insert(no_image, 1), Err(Error::Invalid(_))));
    assert!(matches!(
        s.insert(image_clip(vec![], None), 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        s.insert(file_clip(&[]), 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        s.insert(file_clip(&["a\0b"]), 1),
        Err(Error::Invalid(_))
    ));
    assert!(s.list(&Query::default()).unwrap().is_empty());
}

#[test]
fn previews_by_kind() {
    let mut s = store();
    let mut color = text_clip("  rgb(255, 0, 0) ");
    color.kind = ClipKind::Color;
    assert_eq!(insert(&mut s, color, 1).preview, "#FF0000");
    assert_eq!(
        insert(&mut s, image_clip(vec![9; 10], Some(vec![1])), 2).preview,
        ""
    );
    let f = insert(
        &mut s,
        file_clip(&["/Users/me/Documents/report.pdf", "C:\\data\\notes.txt"]),
        3,
    );
    assert_eq!(f.preview, "report.pdf\nnotes.txt");
}

// ------------------------------------------------------------ content

#[test]
fn content_round_trips() {
    let mut s = store();
    let mut clip = text_clip("hello");
    clip.html = Some("<b>hello</b>".into());
    let t = insert(&mut s, clip, 1);
    let c = s.content(t.id).unwrap().unwrap();
    assert_eq!(c.id, t.id);
    assert_eq!(c.kind, ClipKind::Text);
    assert_eq!(c.text.as_deref(), Some("hello"));
    assert_eq!(c.html.as_deref(), Some("<b>hello</b>"));
    assert_eq!(c.image_png, None);
    assert!(c.file_paths.is_empty());

    let png = vec![0x89, b'P', b'N', b'G', 0, 1, 2];
    let i = insert(&mut s, image_clip(png.clone(), Some(vec![7, 7])), 2);
    assert!(i.has_thumbnail);
    assert_eq!(i.byte_size, png.len() as u64);
    assert_eq!(s.content(i.id).unwrap().unwrap().image_png, Some(png));
    assert_eq!(s.thumbnail(i.id).unwrap(), Some(vec![7, 7]));
    assert_eq!(s.thumbnail(t.id).unwrap(), None);

    let paths = ["/tmp/with\nnewline", "/tmp/日本語 file.txt"];
    let f = insert(&mut s, file_clip(&paths), 3);
    assert_eq!(s.content(f.id).unwrap().unwrap().file_paths, paths);

    assert_eq!(s.content(9_999).unwrap(), None);
    assert_eq!(s.summary(9_999).unwrap(), None);
    assert_eq!(s.thumbnail(9_999).unwrap(), None);
}

// --------------------------------------------------------------- list

#[test]
fn list_pages_newest_first_with_keyset_cursor() {
    let mut s = store();
    let mut expected = Vec::new();
    for i in 0..130 {
        // Groups of three share a timestamp to exercise the id tie-break.
        let c = insert(&mut s, text_clip(&format!("clip {i}")), 1_000 + i / 3);
        expected.push(c.id);
    }
    expected.reverse();

    let mut seen = Vec::new();
    let mut q = Query {
        limit: 50,
        ..Query::default()
    };
    loop {
        let page = s.list(&q).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 50);
        let last = page.last().unwrap();
        q.before = Some((last.last_used_at, last.id));
        seen.extend(page.into_iter().map(|c| c.id));
    }
    assert_eq!(seen, expected);
}

#[test]
fn limit_is_clamped() {
    let mut s = store();
    for i in 0..250 {
        insert(&mut s, text_clip(&format!("n{i}")), i);
    }
    assert_eq!(
        s.list(&Query {
            limit: 0,
            ..Query::default()
        })
        .unwrap()
        .len(),
        50
    );
    assert_eq!(
        s.list(&Query {
            limit: 1,
            ..Query::default()
        })
        .unwrap()
        .len(),
        1
    );
    assert_eq!(
        s.list(&Query {
            limit: 5_000,
            ..Query::default()
        })
        .unwrap()
        .len(),
        200
    );
}

#[test]
fn list_filters_by_kind() {
    let mut s = store();
    let link = insert(&mut s, text_clip("https://example.com"), 1);
    insert(&mut s, text_clip("plain words"), 2);
    let img = insert(&mut s, image_clip(vec![1], None), 3);
    assert_eq!(
        ids(
            &s,
            &Query {
                kind: Some(ClipKind::Link),
                ..Query::default()
            }
        ),
        vec![link.id]
    );
    assert_eq!(
        ids(
            &s,
            &Query {
                kind: Some(ClipKind::Image),
                ..Query::default()
            }
        ),
        vec![img.id]
    );
    assert!(
        ids(
            &s,
            &Query {
                kind: Some(ClipKind::Color),
                ..Query::default()
            }
        )
        .is_empty()
    );
}

// ------------------------------------------------------------- search

#[test]
fn search_matches_words_prefixes_case_and_diacritics() {
    let mut s = store();
    let hello = insert(&mut s, text_clip("Hello brave new World"), 1);
    let cafe = insert(&mut s, text_clip("Meet at the café tomorrow"), 2);
    let url = insert(&mut s, text_clip("https://github.com/tauri-apps/tauri"), 3);

    assert_eq!(search(&s, "hello"), vec![hello.id]);
    assert_eq!(search(&s, "HEL"), vec![hello.id]);
    assert_eq!(search(&s, "wor hel"), vec![hello.id]);
    assert_eq!(search(&s, "hello cafe"), Vec::<i64>::new());
    assert_eq!(search(&s, "cafe"), vec![cafe.id]);
    assert_eq!(search(&s, "café"), vec![cafe.id]);
    assert_eq!(search(&s, "github.com"), vec![url.id]);
    assert_eq!(search(&s, "tauri-apps"), vec![url.id]);
    assert_eq!(search(&s, "zzz"), Vec::<i64>::new());
}

#[test]
fn search_covers_title_app_name_and_file_names() {
    let mut s = store();
    let a = insert(&mut s, text_clip("alpha"), 1);
    let f = insert(&mut s, file_clip(&["/Users/me/Quarterly Report.pdf"]), 2);
    s.set_title(a.id, Some("Shipping address")).unwrap();
    assert_eq!(search(&s, "shipping"), vec![a.id]);
    assert_eq!(search(&s, "safari"), vec![a.id]);
    assert_eq!(search(&s, "finder"), vec![f.id]);
    assert_eq!(search(&s, "quarterly"), vec![f.id]);
}

#[test]
fn search_input_cannot_break_or_inject_fts_syntax() {
    let mut s = store();
    let a = insert(&mut s, text_clip("title: AND OR NOT near"), 1);
    for q in [
        "\"",
        "\"\"",
        "*",
        "**",
        "a*b",
        "AND",
        "OR",
        "NOT",
        "NEAR(",
        "near(a b)",
        "title:foo",
        "body:x",
        "-",
        "^",
        "(",
        ")",
        "'",
        "a\0b",
        "{body title}: x",
        "\u{301}",
        "+",
        "a OR b",
        "NOT a",
        "x\"y",
        "^start",
        ":",
        "(((",
        "\\",
        "%",
        "_",
        "\u{345}",
        "a\u{345}",
        "²",
        "Ⅻ",
        "ǅ",
        "a\u{200d}b",
        "🙂",
        "日本語",
        "\u{feff}",
        "a\u{300}",
        "\u{e000}",
    ] {
        assert!(
            s.list(&Query {
                text: Some(q.into()),
                ..Query::default()
            })
            .is_ok(),
            "query {q:?} errored"
        );
    }
    // Operators are matched as plain words.
    assert_eq!(search(&s, "AND"), vec![a.id]);
    assert_eq!(search(&s, "title: near"), vec![a.id]);
    let long = "word ".repeat(10_000);
    assert!(
        s.list(&Query {
            text: Some(long),
            ..Query::default()
        })
        .is_ok()
    );
}

#[test]
fn blank_or_punctuation_only_search_does_not_filter() {
    let mut s = store();
    insert(&mut s, text_clip("one"), 1);
    insert(&mut s, text_clip("two"), 2);
    assert_eq!(search(&s, "").len(), 2);
    assert_eq!(search(&s, "   ").len(), 2);
    assert_eq!(search(&s, "!!!").len(), 2);
}

#[test]
fn search_combines_with_kind_pinboard_and_cursor() {
    let mut s = store();
    let board = s.create_pinboard("Work", "#112233").unwrap();
    let mut expected = Vec::new();
    for i in 0..60 {
        let c = insert(&mut s, text_clip(&format!("report number {i}")), 100 + i);
        if i % 2 == 0 {
            s.pin(c.id, board.id).unwrap();
            expected.push(c.id);
        }
    }
    insert(&mut s, text_clip("https://report.example.com"), 500);
    expected.reverse();

    let base = Query {
        text: Some("report".into()),
        kind: Some(ClipKind::Text),
        pinboard_id: Some(board.id),
        limit: 10,
        before: None,
    };
    let mut seen = Vec::new();
    let mut q = base.clone();
    loop {
        let page = s.list(&q).unwrap();
        if page.is_empty() {
            break;
        }
        let last = page.last().unwrap();
        q.before = Some((last.last_used_at, last.id));
        seen.extend(page.iter().map(|c| c.id));
    }
    assert_eq!(seen, expected);
}

// ------------------------------------------------------ touch / title

#[test]
fn touch_moves_a_clip_to_the_top() {
    let mut s = store();
    let a = insert(&mut s, text_clip("a"), 1);
    let b = insert(&mut s, text_clip("b"), 2);
    s.touch(a.id, 3).unwrap();
    assert_eq!(ids(&s, &Query::default()), vec![a.id, b.id]);
    assert_eq!(s.summary(a.id).unwrap().unwrap().last_used_at, 3);
    assert!(matches!(s.touch(9_999, 4), Err(Error::NotFound)));
}

#[test]
fn titles_set_clear_and_validate() {
    let mut s = store();
    let a = insert(&mut s, text_clip("body text"), 1);
    s.set_title(a.id, Some("  My Title  ")).unwrap();
    assert_eq!(
        s.summary(a.id).unwrap().unwrap().title.as_deref(),
        Some("My Title")
    );
    assert_eq!(search(&s, "title"), vec![a.id]);
    s.set_title(a.id, Some("Renamed")).unwrap();
    assert!(search(&s, "title").is_empty());
    assert_eq!(
        search(&s, "body"),
        vec![a.id],
        "body stays indexed after a title change"
    );
    s.set_title(a.id, Some("   ")).unwrap();
    assert_eq!(s.summary(a.id).unwrap().unwrap().title, None);
    s.set_title(a.id, Some("x")).unwrap();
    s.set_title(a.id, None).unwrap();
    assert_eq!(s.summary(a.id).unwrap().unwrap().title, None);
    assert!(matches!(
        s.set_title(a.id, Some(&"t".repeat(10_000))),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        s.set_title(9_999, Some("t")),
        Err(Error::NotFound)
    ));
}

// ---------------------------------------------------- delete / clear

#[test]
fn delete_removes_everything_about_a_clip() {
    let mut s = store();
    let board = s.create_pinboard("B", "#000000").unwrap();
    let a = insert(&mut s, image_clip(vec![1, 2], Some(vec![3])), 1);
    s.set_title(a.id, Some("secretword")).unwrap();
    s.pin(a.id, board.id).unwrap();
    assert!(s.delete(a.id).unwrap());
    assert!(!s.delete(a.id).unwrap());
    assert_eq!(s.summary(a.id).unwrap(), None);
    assert_eq!(s.content(a.id).unwrap(), None);
    assert_eq!(s.thumbnail(a.id).unwrap(), None);
    assert!(search(&s, "secretword").is_empty());
    assert!(
        ids(
            &s,
            &Query {
                pinboard_id: Some(board.id),
                ..Query::default()
            }
        )
        .is_empty()
    );
    // The same content can be captured again as a new clip.
    assert!(matches!(
        s.insert(image_clip(vec![1, 2], None), 2).unwrap(),
        InsertOutcome::Inserted(_)
    ));
}

#[test]
fn clear_history_keeps_pinned_clips() {
    let mut s = store();
    let board = s.create_pinboard("Keep", "#FFFFFF").unwrap();
    let kept = insert(&mut s, text_clip("pinned thing"), 1);
    s.pin(kept.id, board.id).unwrap();
    for i in 0..5 {
        insert(&mut s, text_clip(&format!("gone {i}")), 10 + i);
    }
    assert_eq!(s.clear_history().unwrap(), 5);
    assert_eq!(ids(&s, &Query::default()), vec![kept.id]);
    assert!(search(&s, "gone").is_empty());
    assert_eq!(search(&s, "pinned"), vec![kept.id]);
    assert_eq!(s.clear_history().unwrap(), 0);
}

// -------------------------------------------------------------- prune

#[test]
fn prune_applies_max_items_to_unpinned_clips() {
    let mut s = store();
    let board = s.create_pinboard("P", "#123456").unwrap();
    let old_pinned = insert(&mut s, text_clip("old pinned"), 0);
    s.pin(old_pinned.id, board.id).unwrap();
    let mut all = Vec::new();
    for i in 1..=10 {
        all.push(insert(&mut s, text_clip(&format!("c{i}")), i).id);
    }
    let settings = Settings {
        max_items: 3,
        retention_days: None,
        ..Settings::default()
    };
    assert_eq!(s.prune(&settings, 100).unwrap(), 7);
    let mut expect: Vec<i64> = all[7..].iter().rev().copied().collect();
    expect.push(old_pinned.id);
    assert_eq!(ids(&s, &Query::default()), expect);
    assert_eq!(s.prune(&settings, 100).unwrap(), 0);
}

#[test]
fn prune_applies_retention_days() {
    let mut s = store();
    const DAY: i64 = 86_400_000;
    let now = 100 * DAY;
    let old = insert(&mut s, text_clip("old"), now - 31 * DAY);
    let fresh = insert(&mut s, text_clip("fresh"), now - 29 * DAY);
    let board = s.create_pinboard("P", "#123456").unwrap();
    let old_pinned = insert(&mut s, text_clip("old pinned"), now - 365 * DAY);
    s.pin(old_pinned.id, board.id).unwrap();

    let keep_all = Settings {
        max_items: 0,
        retention_days: None,
        ..Settings::default()
    };
    assert_eq!(s.prune(&keep_all, now).unwrap(), 0);

    let thirty = Settings {
        max_items: 0,
        retention_days: Some(30),
        ..Settings::default()
    };
    assert_eq!(s.prune(&thirty, now).unwrap(), 1);
    assert_eq!(s.summary(old.id).unwrap(), None);
    assert!(s.summary(fresh.id).unwrap().is_some());
    assert!(s.summary(old_pinned.id).unwrap().is_some());
}

// ---------------------------------------------------------- pinboards

#[test]
fn pinboards_crud() {
    let mut s = store();
    assert!(s.pinboards().unwrap().is_empty());
    let a = s.create_pinboard("  Work ", "#aabbcc").unwrap();
    let b = s.create_pinboard("Personal", "rgb(1, 2, 3)").unwrap();
    assert_eq!(a.name, "Work");
    assert_eq!(a.color, "#AABBCC");
    assert_eq!(b.color, "#010203");
    assert!(b.position > a.position);
    assert_eq!(s.pinboards().unwrap(), vec![a.clone(), b.clone()]);

    s.rename_pinboard(a.id, "Job").unwrap();
    assert_eq!(s.pinboards().unwrap()[0].name, "Job");
    assert!(matches!(
        s.rename_pinboard(9_999, "x"),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        s.rename_pinboard(a.id, "  "),
        Err(Error::Invalid(_))
    ));

    assert!(matches!(
        s.create_pinboard("", "#000000"),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        s.create_pinboard(&"n".repeat(1_000), "#000000"),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        s.create_pinboard("Bad", "blue"),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        s.create_pinboard("Bad", "#00000080"),
        Err(Error::Invalid(_))
    ));

    s.delete_pinboard(a.id).unwrap();
    assert_eq!(s.pinboards().unwrap(), vec![b]);
    assert!(matches!(s.delete_pinboard(a.id), Err(Error::NotFound)));
}

#[test]
fn pinning_and_membership() {
    let mut s = store();
    let a = s.create_pinboard("A", "#000001").unwrap();
    let b = s.create_pinboard("B", "#000002").unwrap();
    let clip = insert(&mut s, text_clip("pin me"), 1);
    let other = insert(&mut s, text_clip("not me"), 2);

    s.pin(clip.id, a.id).unwrap();
    s.pin(clip.id, a.id).unwrap(); // idempotent
    s.pin(clip.id, b.id).unwrap();
    let mut boards = s.summary(clip.id).unwrap().unwrap().pinboard_ids;
    boards.sort();
    assert_eq!(boards, vec![a.id, b.id]);
    let listed = s.list(&Query::default()).unwrap();
    let row = listed.iter().find(|c| c.id == clip.id).unwrap();
    assert_eq!(row.pinboard_ids.len(), 2);
    assert!(
        listed
            .iter()
            .find(|c| c.id == other.id)
            .unwrap()
            .pinboard_ids
            .is_empty()
    );

    assert_eq!(
        ids(
            &s,
            &Query {
                pinboard_id: Some(a.id),
                ..Query::default()
            }
        ),
        vec![clip.id]
    );
    s.unpin(clip.id, a.id).unwrap();
    s.unpin(clip.id, a.id).unwrap(); // idempotent
    assert!(
        ids(
            &s,
            &Query {
                pinboard_id: Some(a.id),
                ..Query::default()
            }
        )
        .is_empty()
    );
    assert_eq!(
        s.summary(clip.id).unwrap().unwrap().pinboard_ids,
        vec![b.id]
    );

    assert!(matches!(s.pin(9_999, a.id), Err(Error::NotFound)));
    assert!(matches!(s.pin(clip.id, 9_999), Err(Error::NotFound)));

    // Deleting a pinboard keeps its clips in the history.
    s.delete_pinboard(b.id).unwrap();
    assert!(s.summary(clip.id).unwrap().unwrap().pinboard_ids.is_empty());
    assert_eq!(ids(&s, &Query::default()), vec![other.id, clip.id]);
}

// ----------------------------------------------------------- settings

#[test]
fn window_state_round_trips_apart_from_settings() {
    let mut s = store();
    assert_eq!(s.window_state("settings").unwrap(), None);
    s.save_window_state("settings", "{\"x\":1}").unwrap();
    s.save_window_state("settings", "{\"x\":2}").unwrap();
    assert_eq!(s.window_state("settings").unwrap().as_deref(), Some("{\"x\":2}"));
    assert_eq!(s.window_state("other").unwrap(), None);
    // Not a setting, and saving settings leaves it alone.
    assert_eq!(s.settings().unwrap(), Settings::default());
    s.save_settings(&Settings { max_items: 7, ..Settings::default() }).unwrap();
    assert_eq!(s.window_state("settings").unwrap().as_deref(), Some("{\"x\":2}"));
    assert!(s.save_window_state("settings", &"x".repeat(5000)).is_err());
}

#[test]
fn settings_default_until_saved_then_round_trip() {
    let mut s = store();
    assert_eq!(s.settings().unwrap(), Settings::default());
    let custom = Settings {
        max_items: 42,
        retention_days: None,
        excluded_app_ids: vec!["b.second".into(), "a.first".into()],
        capture_paused: true,
        capture_images: false,
        shelf_shortcut: "Alt+Space".into(),
        paste_on_select: false,
        link_previews: true,
        check_for_updates: false,
    };
    s.save_settings(&custom).unwrap();
    assert_eq!(s.settings().unwrap(), custom);

    let empty_list = Settings {
        excluded_app_ids: vec![],
        retention_days: Some(7),
        ..custom
    };
    s.save_settings(&empty_list).unwrap();
    assert_eq!(s.settings().unwrap(), empty_list);
}

#[test]
fn update_checks_are_on_by_default_and_round_trip() {
    assert!(Settings::default().check_for_updates);
    let mut s = store();
    s.save_settings(&Settings { check_for_updates: false, ..Settings::default() }).unwrap();
    assert!(!s.settings().unwrap().check_for_updates);
    // Settings saved without the field (Satchel 0.1.0): see store_file.rs.
}

// ------------------------------------------------- images and rich text

/// Signature + IHDR of a `w` x `h` PNG: all the store reads of an image.
fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    png.extend_from_slice(&w.to_be_bytes());
    png.extend_from_slice(&h.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png
}

#[test]
fn images_show_their_dimensions_and_are_found_by_search() {
    let mut s = store();
    let img = insert(&mut s, image_clip(png_header(1920, 1080), Some(vec![1])), 1);
    assert_eq!(img.preview, "1920 \u{d7} 1080");
    insert(&mut s, text_clip("an unrelated note"), 2);
    let q = |t: &str| Query { text: Some(t.into()), ..Query::default() };
    assert_eq!(ids(&s, &q("image")), vec![img.id]);
    assert_eq!(ids(&s, &q("1920")), vec![img.id]);
    assert_eq!(
        ids(&s, &Query { kind: Some(ClipKind::Image), ..q("image") }),
        vec![img.id]
    );
}

#[test]
fn rich_text_keeps_rtf_and_html_and_the_latest_formatting_wins() {
    let mut s = store();
    let plain = insert(&mut s, text_clip("hello"), 1);
    assert_eq!(s.content(plain.id).unwrap().unwrap().rtf, None);
    // The same text copied again, bold this time: same clip, new formatting.
    let mut bold = text_clip("hello");
    bold.html = Some("<b>hello</b>".into());
    bold.rtf = Some(b"{\\rtf1 \\b hello}".to_vec());
    let again = s.insert(bold, 2).unwrap();
    assert!(matches!(again, InsertOutcome::Bumped(ref c) if c.id == plain.id));
    let c = s.content(plain.id).unwrap().unwrap();
    assert_eq!(c.html.as_deref(), Some("<b>hello</b>"));
    assert_eq!(c.rtf.as_deref(), Some(&b"{\\rtf1 \\b hello}"[..]));
    // Copied once more without formatting: the formatting is kept.
    insert(&mut s, text_clip("hello"), 3);
    assert!(s.content(plain.id).unwrap().unwrap().rtf.is_some());
}

#[test]
fn rich_flavors_are_dropped_when_oversized_or_not_text() {
    let mut s = store();
    let mut big = text_clip("plain");
    big.rtf = Some(vec![b'r'; MAX_TEXT_BYTES as usize + 1]);
    let c = insert(&mut s, big, 1);
    assert_eq!(s.content(c.id).unwrap().unwrap().rtf, None);
    let mut img = image_clip(png_header(2, 2), None);
    img.rtf = Some(b"{\\rtf1 x}".to_vec());
    img.html = Some("<img>".into());
    let i = insert(&mut s, img, 2);
    let c = s.content(i.id).unwrap().unwrap();
    assert_eq!((c.rtf, c.html), (None, None));
}

// ------------------------------------------------------------ link previews

mod link_previews {
    use super::*;
    use satchel_core::{FetchedPreview, LinkImageKind, LinkPreview, LinkPreviewOutcome, store::LINK_PREVIEW_ATTEMPTS};

    fn link(s: &mut Store, url: &str, now: i64) -> i64 {
        let c = insert(s, text_clip(url), now);
        assert_eq!(c.kind, ClipKind::Link);
        c.id
    }

    fn page(title: &str) -> FetchedPreview {
        FetchedPreview { title: Some(title.into()), site: "Example".into(), image: Some((LinkImageKind::Photo, vec![1, 2, 3])) }
    }

    fn summary(s: &Store, id: i64) -> satchel_core::ClipSummary {
        s.summary(id).unwrap().unwrap()
    }

    #[test]
    fn a_link_is_queued_claimed_and_cached_once() {
        let mut s = store();
        let id = link(&mut s, "https://www.example.com/a", 1);
        assert_eq!(summary(&s, id).link_preview, None);
        assert!(s.queue_link_preview(id, 10).unwrap());
        assert!(!s.queue_link_preview(id, 11).unwrap(), "queued once");
        assert_eq!(s.next_link_preview_at().unwrap(), Some(10));
        assert!(s.claim_link_preview(9, 1000).unwrap().is_none(), "not due yet");

        let job = s.claim_link_preview(10, 1000).unwrap().unwrap();
        assert_eq!(job.clip_id, id);
        assert_eq!(job.url, "https://www.example.com/a");
        assert_eq!(job.source_app_id.as_deref(), Some("com.apple.Safari"));
        assert_eq!(job.attempts, 0);
        assert!(s.claim_link_preview(10, 1000).unwrap().is_none(), "leased to the first worker");

        assert!(s.finish_link_preview(id, LinkPreviewOutcome::Ready(page("A page")), 20).unwrap());
        assert_eq!(
            summary(&s, id).link_preview,
            Some(LinkPreview { title: Some("A page".into()), site: "Example".into(), image: Some(LinkImageKind::Photo) })
        );
        assert_eq!(s.link_preview_image(id).unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(s.next_link_preview_at().unwrap(), None);
        // Copying the same link again does not fetch again.
        let again = link(&mut s, "https://www.example.com/a", 30);
        assert_eq!(again, id);
        assert!(!s.queue_link_preview(id, 30).unwrap());
        assert!(s.list(&Query::default()).unwrap()[0].link_preview.is_some());
    }

    #[test]
    fn only_link_clips_are_queued() {
        let mut s = store();
        let t = insert(&mut s, text_clip("just some words"), 1).id;
        assert!(!s.queue_link_preview(t, 1).unwrap());
        assert!(!s.queue_link_preview(999, 1).unwrap());
        assert!(s.claim_link_preview(i64::MAX, 1).unwrap().is_none());
    }

    #[test]
    fn transient_failures_back_off_then_give_up() {
        let mut s = store();
        let id = link(&mut s, "https://www.example.com/flaky", 1);
        s.queue_link_preview(id, 0).unwrap();
        let mut now = 0;
        for attempt in 0..LINK_PREVIEW_ATTEMPTS {
            let job = s.claim_link_preview(now, 100).unwrap().expect("due");
            assert_eq!(job.attempts, attempt);
            now += 1000;
            assert!(!s.finish_link_preview(id, LinkPreviewOutcome::Retry { retry_at: now }, now).unwrap());
        }
        assert!(s.claim_link_preview(i64::MAX, 1).unwrap().is_none(), "given up");
        assert_eq!(summary(&s, id).link_preview, None);
        assert!(!s.queue_link_preview(id, now).unwrap(), "a failed link is not queued again");
    }

    #[test]
    fn failed_and_skipped_are_final_and_invisible() {
        let mut s = store();
        let a = link(&mut s, "https://www.example.com/a", 1);
        let b = link(&mut s, "https://www.example.com/b", 2);
        s.queue_link_preview(a, 0).unwrap();
        s.queue_link_preview(b, 0).unwrap();
        let j = s.claim_link_preview(0, 10).unwrap().unwrap();
        s.finish_link_preview(j.clip_id, LinkPreviewOutcome::Failed, 0).unwrap();
        let j = s.claim_link_preview(0, 10).unwrap().unwrap();
        s.finish_link_preview(j.clip_id, LinkPreviewOutcome::Skipped, 0).unwrap();
        assert!(s.claim_link_preview(i64::MAX, 1).unwrap().is_none());
        assert_eq!(summary(&s, a).link_preview, None);
        assert_eq!(summary(&s, b).link_preview, None);
        assert_eq!(s.link_preview_image(a).unwrap(), None);
        // A late result for a finished row changes nothing.
        assert!(!s.finish_link_preview(a, LinkPreviewOutcome::Ready(page("late")), 5).unwrap());
        assert_eq!(summary(&s, a).link_preview, None);
    }

    #[test]
    fn a_lease_that_runs_out_is_claimed_again() {
        let mut s = store();
        let id = link(&mut s, "https://www.example.com/a", 1);
        s.queue_link_preview(id, 0).unwrap();
        s.claim_link_preview(0, 100).unwrap().unwrap();
        assert!(s.claim_link_preview(99, 100).unwrap().is_none());
        assert_eq!(s.claim_link_preview(100, 100).unwrap().unwrap().clip_id, id);
    }

    #[test]
    fn previews_are_searchable_by_title() {
        let mut s = store();
        let id = link(&mut s, "https://www.example.com/p/42", 1);
        s.queue_link_preview(id, 0).unwrap();
        s.claim_link_preview(0, 10).unwrap();
        s.finish_link_preview(id, LinkPreviewOutcome::Ready(page("Zebracorn Sightings")), 0).unwrap();
        assert_eq!(search(&s, "zebrac"), vec![id]);
        assert_eq!(search(&s, "example"), vec![id], "the URL still matches");
        s.set_title(id, Some("Mine")).unwrap();
        assert_eq!(search(&s, "zebracorn mine"), vec![id], "own title and page title");
        assert_eq!(s.clear_link_previews().unwrap(), 1);
        assert!(search(&s, "zebracorn").is_empty());
        assert_eq!(search(&s, "mine"), vec![id]);
    }

    #[test]
    fn previews_live_and_die_with_their_clip() {
        let mut s = store();
        let ready = |s: &mut Store, url: &str, now: i64| {
            let id = link(s, url, now);
            s.queue_link_preview(id, 0).unwrap();
            let j = s.claim_link_preview(i64::MAX - 1, 0).unwrap().unwrap();
            assert_eq!(j.clip_id, id);
            s.finish_link_preview(id, LinkPreviewOutcome::Ready(page(url)), now).unwrap();
            id
        };
        let a = ready(&mut s, "https://www.example.com/a", 1);
        let b = ready(&mut s, "https://www.example.com/b", 2);
        let c = ready(&mut s, "https://www.example.com/c", 3);
        assert!(s.delete(a).unwrap());
        assert_eq!(s.link_preview_image(a).unwrap(), None);
        let board = s.create_pinboard("Keep", "#000000").unwrap();
        s.pin(c, board.id).unwrap();
        assert_eq!(s.clear_history().unwrap(), 1);
        assert_eq!(s.link_preview_image(b).unwrap(), None);
        assert!(s.link_preview_image(c).unwrap().is_some(), "pinned clip keeps its preview");
        // Re-copying a deleted link makes a new clip, which may be fetched.
        let a2 = link(&mut s, "https://www.example.com/a", 9);
        assert!(s.queue_link_preview(a2, 9).unwrap());
        // Retention pruning removes previews too.
        s.unpin(c, board.id).unwrap();
        s.prune(&Settings { retention_days: Some(1), ..Settings::default() }, 10 * 86_400_000).unwrap();
        assert_eq!(s.link_preview_image(c).unwrap(), None);
        assert_eq!(s.next_link_preview_at().unwrap(), None);
    }

    #[test]
    fn clearing_removes_cache_and_queue() {
        let mut s = store();
        let a = link(&mut s, "https://www.example.com/a", 1);
        let b = link(&mut s, "https://www.example.com/b", 2);
        s.queue_link_preview(a, 0).unwrap();
        s.queue_link_preview(b, 0).unwrap();
        s.claim_link_preview(0, 0).unwrap();
        s.finish_link_preview(a, LinkPreviewOutcome::Ready(page("A")), 0).unwrap();
        assert_eq!(s.clear_link_previews().unwrap(), 2);
        assert_eq!(s.link_preview_image(a).unwrap(), None);
        assert_eq!(summary(&s, a).link_preview, None);
        assert_eq!(s.next_link_preview_at().unwrap(), None);
        // A result arriving after the clear is dropped.
        assert!(!s.finish_link_preview(b, LinkPreviewOutcome::Ready(page("B")), 0).unwrap());
        assert_eq!(summary(&s, b).link_preview, None);
        // Turned on again later, the link can be queued again.
        assert!(s.queue_link_preview(a, 5).unwrap());
    }

    #[test]
    fn stored_text_and_images_are_capped() {
        let mut s = store();
        let id = link(&mut s, "https://www.example.com/a", 1);
        s.queue_link_preview(id, 0).unwrap();
        s.claim_link_preview(0, 0).unwrap();
        let big = FetchedPreview {
            title: Some("t".repeat(10_000)),
            site: "s".repeat(10_000),
            image: Some((LinkImageKind::Icon, vec![0; 600 * 1024])),
        };
        s.finish_link_preview(id, LinkPreviewOutcome::Ready(big), 0).unwrap();
        let p = summary(&s, id).link_preview.unwrap();
        assert!(p.title.unwrap().chars().count() <= 201);
        assert!(p.site.chars().count() <= 81);
        assert_eq!(p.image, None, "oversized image dropped");
        assert_eq!(s.link_preview_image(id).unwrap(), None);
    }

    #[test]
    fn the_setting_defaults_on_and_round_trips() {
        let mut s = store();
        assert!(s.settings().unwrap().link_previews);
        s.save_settings(&Settings { link_previews: false, ..Settings::default() }).unwrap();
        assert!(!s.settings().unwrap().link_previews);
    }

    #[test]
    fn backfill_queues_older_links_newest_first_one_at_a_time() {
        let mut s = store();
        let old = link(&mut s, "https://www.example.com/old", 1);
        insert(&mut s, text_clip("plain words"), 2);
        let newer = link(&mut s, "https://www.example.com/newer", 3);
        let fresh = link(&mut s, "https://www.example.com/fresh", 4);
        s.queue_link_preview(fresh, 4).unwrap();
        assert_eq!(s.queue_link_backfill(10).unwrap(), Some(newer));
        assert_eq!(s.queue_link_backfill(11).unwrap(), Some(old));
        assert_eq!(s.queue_link_backfill(12).unwrap(), None, "every link has a row");
        let due: Vec<i64> = std::iter::from_fn(|| s.claim_link_preview(100, 1000).unwrap().map(|j| j.clip_id)).collect();
        assert_eq!(due, vec![fresh, newer, old]);
        // Cleared (setting off), the links are backfill candidates again.
        s.clear_link_previews().unwrap();
        assert_eq!(s.queue_link_backfill(20).unwrap(), Some(fresh));
    }
}
