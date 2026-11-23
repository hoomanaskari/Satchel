//! Platform-independent parts of the excluded-apps picker: finding app
//! bundles on disk, merging installed and running apps into one candidate
//! list, and the icon cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::platform::AppInfo;

/// Icon edge in pixels: 32 points at 2x, used for rows of 16–24 points.
pub const ICON_PX: u32 = 64;
/// Icons kept in memory. The settings window shows a few hundred at most;
/// the cache is also emptied whenever that window closes. The shelf asks for
/// one per source app on its cards and keeps those itself.
pub const ICON_CACHE_CAP: usize = 600;

/// Appends the bundles (directories named `*.{ext}`) found in `dir`,
/// looking `depth` levels deep. Bundles are not entered (an app inside an
/// app is a helper); hidden entries and symlinked folders are skipped, so
/// the walk is bounded.
pub fn collect_bundles(dir: &Path, depth: u8, ext: &str, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let is_bundle = Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case(ext));
        // `metadata` follows symlinks: an app linked into /Applications counts.
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.is_dir() {
            continue;
        }
        if is_bundle {
            out.push(path);
        } else if !entry.file_type().is_ok_and(|t| t.is_symlink()) {
            collect_bundles(&path, depth - 1, ext, out);
        }
    }
}

/// One entry per app id (case-insensitive, like capture matching), sorted
/// by name, then id. The first installed copy names the app; an app that is
/// also running is marked so. Empty ids are dropped.
pub fn merge(installed: Vec<AppInfo>, running: Vec<AppInfo>) -> Vec<AppInfo> {
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<AppInfo> = Vec::with_capacity(installed.len() + running.len());
    for app in installed.into_iter().chain(running) {
        let id = app.id.trim();
        if id.is_empty() {
            continue;
        }
        let key = id.to_lowercase();
        if let Some(&i) = by_id.get(&key) {
            out[i].running |= app.running;
            continue;
        }
        by_id.insert(key, out.len());
        let name = if app.name.trim().is_empty() { id.to_string() } else { app.name.trim().to_string() };
        out.push(AppInfo { id: id.to_string(), name, running: app.running });
    }
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.to_lowercase().cmp(&b.id.to_lowercase()))
    });
    out
}

/// Rendered icons by lowercased app id. Misses (`None`) are cached too, so
/// repeated lookups of an app that is not installed stay cheap.
#[derive(Default)]
pub struct IconCache {
    map: Mutex<HashMap<String, Option<Arc<Vec<u8>>>>>,
}

impl IconCache {
    pub fn get_or_render(&self, id: &str, render: impl FnOnce() -> Option<Vec<u8>>) -> Option<Arc<Vec<u8>>> {
        let key = id.to_lowercase();
        if let Some(hit) = self.lock().get(&key) {
            return hit.clone();
        }
        // Rendered without holding the lock: icons for different apps load
        // in parallel. Two racing requests for one app both render; the
        // results are identical.
        let icon = render().map(Arc::new);
        let mut map = self.lock();
        if map.len() >= ICON_CACHE_CAP {
            map.clear();
        }
        map.insert(key, icon.clone());
        icon
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<Arc<Vec<u8>>>>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, running: bool) -> AppInfo {
        AppInfo { id: id.into(), name: name.into(), running }
    }

    #[test]
    fn merge_deduplicates_by_id_case_insensitively_and_keeps_the_installed_name() {
        let merged = merge(
            vec![app("com.apple.Safari", "Safari", false), app("com.apple.safari", "Safari copy", false)],
            vec![app("COM.APPLE.SAFARI", "Safari (running)", true)],
        );
        assert_eq!(merged, vec![app("com.apple.Safari", "Safari", true)]);
    }

    #[test]
    fn merge_adds_running_apps_that_are_not_installed() {
        let merged = merge(vec![app("a.notes", "Notes", false)], vec![app("x.dmg", "From A Disk Image", true)]);
        assert_eq!(merged, vec![app("x.dmg", "From A Disk Image", true), app("a.notes", "Notes", false)]);
    }

    #[test]
    fn merge_sorts_by_name_ignoring_case_then_by_id() {
        let merged = merge(
            vec![app("z.b", "beta", false), app("b.a", "Alpha", false), app("a.a", "alpha", false)],
            vec![],
        );
        let ids: Vec<_> = merged.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["a.a", "b.a", "z.b"]);
    }

    #[test]
    fn merge_drops_empty_ids_and_names_nameless_apps_by_id() {
        let merged = merge(vec![app("  ", "Ghost", false), app(" x.y ", " ", false)], vec![]);
        assert_eq!(merged, vec![app("x.y", "x.y", false)]);
    }

    #[test]
    fn bundles_are_found_one_folder_deep_without_entering_bundles() {
        let root = std::env::temp_dir().join(format!("satchel-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [
            "One.app/Contents",
            "Utilities/Two.app",
            "Utilities/Deeper/Three.app",
            "Four.app/Contents/Helpers/Helper.app",
            ".Hidden.app",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("file.app"), b"not a bundle").unwrap();
        let mut found = Vec::new();
        collect_bundles(&root, 2, "app", &mut found);
        let names: Vec<_> = found.iter().map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["Four.app", "One.app", "Utilities/Two.app"]);
        let mut none = Vec::new();
        collect_bundles(&root.join("missing"), 2, "app", &mut none);
        assert!(none.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn icon_cache_renders_once_per_app_and_remembers_misses() {
        let cache = IconCache::default();
        let mut renders = 0;
        for id in ["a.b", "A.B"] {
            let icon = cache.get_or_render(id, || {
                renders += 1;
                Some(vec![1, 2, 3])
            });
            assert_eq!(icon.as_deref(), Some(&vec![1, 2, 3]));
        }
        assert_eq!(cache.get_or_render("gone", || None), None);
        assert_eq!(cache.get_or_render("gone", || panic!("miss is cached")), None);
        assert_eq!(renders, 1);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    /// The settings window hides built-in defaults that are not installed; it
    /// tells them apart from user-added entries with a copy of the core's
    /// defaults (`BUILT_IN_DEFAULTS` in `src/lib/excluded-apps.ts`).
    #[test]
    fn settings_window_copy_of_the_defaults_matches_the_core() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/excluded-apps.ts");
        let source = std::fs::read_to_string(path).expect("excluded-apps.ts");
        let start = source.find("export const BUILT_IN_DEFAULTS").expect("BUILT_IN_DEFAULTS");
        let body = &source[start..];
        let body = &body[body.find('[').unwrap() + 1..body.find("];").unwrap()];
        let copy: Vec<String> = body
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('"'))
            .map(|l| l.trim_end_matches(',').trim_matches('"').to_string())
            .collect();
        assert_eq!(copy, satchel_core::privacy::default_excluded_app_ids());
    }

    #[test]
    fn icon_cache_is_bounded() {
        let cache = IconCache::default();
        for i in 0..ICON_CACHE_CAP + 5 {
            cache.get_or_render(&format!("app{i}"), || Some(vec![0]));
        }
        assert!(cache.len() <= ICON_CACHE_CAP);
    }
}
