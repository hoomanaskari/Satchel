//! Performance budget: search and the first page over 10,000 clips must stay
//! well inside 30 ms (docs/ARCHITECTURE.md).
//!
//! Debug builds are not representative, so this is ignored by default. Run:
//!
//! ```text
//! cargo test -p satchel-core --release --test perf -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use satchel_core::{ClipKind, InsertOutcome, NewClip, Query, SourceApp, Store};

const CLIPS: usize = 10_000;
const BUDGET: Duration = Duration::from_millis(30);
const RUNS: usize = 25;

/// xorshift64*: deterministic data without a `rand` dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn vocabulary(rng: &mut Rng) -> Vec<String> {
    const SYL: &[&str] = &[
        "ka", "lo", "mi", "ter", "on", "sa", "vel", "dor", "pri", "an", "es", "tu", "ri", "go",
        "ne", "bal", "cor", "fi", "ly", "um", "wor", "hel", "zen", "qua",
    ];
    (0..3_000)
        .map(|_| {
            (0..2 + rng.below(3))
                .map(|_| SYL[rng.below(SYL.len())])
                .collect()
        })
        .collect()
}

fn clip(rng: &mut Rng, words: &[String], i: usize) -> NewClip {
    let n = 3 + rng.below(120);
    let mut text: String = (0..n)
        .map(|_| words[rng.below(words.len())].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let kind = match i % 10 {
        0 => {
            text = format!("https://example.com/{}/{i}", words[rng.below(words.len())]);
            ClipKind::Link
        }
        1 => {
            text = format!(
                "fn f{i}() {{\n    let x = {};\n    x + 1\n}}",
                rng.below(1000)
            );
            ClipKind::Code
        }
        _ => {
            text.push_str(&format!(" #{i}"));
            ClipKind::Text
        }
    };
    let apps = [
        ("com.apple.Safari", "Safari"),
        ("com.apple.mail", "Mail"),
        ("com.microsoft.VSCode", "Code"),
    ];
    let (id, name) = apps[i % apps.len()];
    NewClip {
        kind,
        text: Some(text),
        html: None,
        rtf: None,
        image_png: None,
        thumbnail_png: None,
        file_paths: vec![],
        source_app: Some(SourceApp {
            id: id.into(),
            name: name.into(),
        }),
    }
}

fn time<T>(label: &str, mut f: impl FnMut() -> T) -> Duration {
    let cold = Instant::now();
    f();
    let cold = cold.elapsed();
    let mut samples: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            t.elapsed()
        })
        .collect();
    samples.sort();
    let median = samples[RUNS / 2];
    let max = *samples.last().unwrap().max(&cold);
    println!("{label:<44} cold {cold:>10.3?}  median {median:>10.3?}  max {max:>10.3?}");
    assert!(max < BUDGET, "{label}: {max:?} exceeds {BUDGET:?}");
    max
}

#[test]
#[ignore = "timing test; run in release: cargo test -p satchel-core --release --test perf -- --ignored"]
fn search_and_first_page_over_10k_clips_fit_the_budget() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("satchel-core-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = Store::open(&dir.join("satchel.db")).unwrap();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let words = vocabulary(&mut rng);
    let board = store.create_pinboard("Perf", "#336699").unwrap();
    let started = Instant::now();
    for i in 0..CLIPS {
        let out = store.insert(clip(&mut rng, &words, i), i as i64).unwrap();
        let (InsertOutcome::Inserted(c) | InsertOutcome::Bumped(c)) = out;
        if i % 50 == 0 {
            store.pin(c.id, board.id).unwrap();
        }
    }
    let insert_total = started.elapsed();
    println!(
        "inserted {CLIPS} clips in {insert_total:.2?} ({:.3?} per clip)",
        insert_total / CLIPS as u32
    );
    assert_eq!(
        store
            .list(&Query {
                limit: 200,
                ..Query::default()
            })
            .unwrap()
            .len(),
        200
    );

    let page = |q: Query| store.list(&q).unwrap();
    let search = |t: &str| Query {
        text: Some(t.into()),
        ..Query::default()
    };

    time("list: first page (50)", || page(Query::default()));
    time("list: first page (200)", || {
        page(Query {
            limit: 200,
            ..Query::default()
        })
    });
    let mid = page(Query {
        limit: 200,
        ..Query::default()
    })
    .last()
    .map(|c| (c.last_used_at, c.id));
    time("list: page after cursor", || {
        page(Query {
            before: mid,
            ..Query::default()
        })
    });
    time("list: kind = link", || {
        page(Query {
            kind: Some(ClipKind::Link),
            ..Query::default()
        })
    });
    time("list: pinboard", || {
        page(Query {
            pinboard_id: Some(board.id),
            ..Query::default()
        })
    });

    let rare = words[17].clone();
    time("search: one char prefix \"k\"", || page(search("k")));
    time("search: two char prefix \"wo\"", || page(search("wo")));
    time("search: common word prefix \"hel\"", || page(search("hel")));
    time(&format!("search: rare word \"{rare}\""), || {
        page(search(&rare))
    });
    time("search: three words", || {
        page(search(&format!("{} {} {}", words[1], words[2], words[3])))
    });
    time("search: no match", || page(search("zzzzqqq")));
    time("search: app name \"safari\"", || page(search("safari")));
    time("search + kind + pinboard", || {
        page(Query {
            text: Some("ka".into()),
            kind: Some(ClipKind::Text),
            pinboard_id: Some(board.id),
            ..Query::default()
        })
    });
    let first = page(search("ka"));
    let cursor = first.last().map(|c| (c.last_used_at, c.id));
    time("search: second page", || {
        page(Query {
            before: cursor,
            ..search("ka")
        })
    });

    // Write paths the shelf waits on (paste bumps, delete, rename).
    let victims: Vec<i64> = store
        .list(&Query {
            limit: 200,
            ..Query::default()
        })
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    let mut k = 0;
    let mut now = CLIPS as i64;
    time("write: touch (paste bump)", || {
        now += 1;
        store.touch(victims[k % victims.len()], now).unwrap();
        k += 1;
    });
    time("write: set title", || {
        store
            .set_title(victims[k % victims.len()], Some("renamed clip"))
            .unwrap();
        k += 1;
    });
    time("write: insert new clip", || {
        now += 1;
        store.insert(clip(&mut rng, &words, k), now).unwrap();
        k += 1;
    });
    let mut d = 0;
    time("write: delete one clip (incl. WAL truncate)", || {
        assert!(store.delete(victims[d]).unwrap());
        d += 1;
    });

    drop(store);
    let size: u64 = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    println!("database files: {:.1} MiB", size as f64 / (1024.0 * 1024.0));
    let _ = std::fs::remove_dir_all(&dir);
}
