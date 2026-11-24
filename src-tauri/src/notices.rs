//! The third-party licence notices shipped inside the app (`THIRD-PARTY-NOTICES.txt` at the repo
//! root, written by `pnpm notices` and bundled into `Contents/Resources` by `tauri.conf.json`).
//! The settings window shows them as plain text through `get_third_party_notices`.

use std::io::Read;
use std::path::Path;

use crate::error::{self, CmdResult};

/// Name of the file, at the repo root and in the app's resources.
pub const FILE: &str = "THIRD-PARTY-NOTICES.txt";

/// Far above the real file (about 0.25 MB), so a damaged bundle can't make the window load
/// something huge.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The notices at `path`, as UTF-8 text. Errors are fixed strings (no path).
pub fn read(path: &Path) -> CmdResult<String> {
    let file = std::fs::File::open(path).map_err(|_| error::NOT_FOUND.to_string())?;
    let mut text = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut text).map_err(|_| error::INTERNAL.to_string())?;
    if text.len() as u64 > MAX_BYTES {
        return Err(error::TOO_LARGE.to_string());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_file() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(FILE)
    }

    #[test]
    fn the_generated_notices_are_in_the_repo_and_readable() {
        let text = read(&repo_file()).unwrap();
        assert!(text.starts_with("THIRD-PARTY SOFTWARE NOTICES FOR SATCHEL"));
        // Spot checks: a crate under each licence that needs its text shipped.
        for needle in ["Mozilla Public License 2.0 (MPL-2.0)", "webpki-roots", "Apache License", "svelte"] {
            assert!(text.contains(needle), "{needle} missing from {FILE}");
        }
    }

    #[test]
    fn both_apps_bundle_the_notices_as_a_resource() {
        // tauri.dev.conf.json does not set `bundle.resources`, so Satchel Dev inherits this.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["bundle"]["resources"][format!("../{FILE}")], FILE);
        let dev: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.dev.conf.json")).unwrap();
        assert!(dev["bundle"].get("resources").is_none());
    }

    #[test]
    fn missing_or_oversized_files_give_fixed_errors() {
        let dir = std::env::temp_dir().join(format!("satchel-notices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir.join("absent.txt")).unwrap_err(), error::NOT_FOUND);
        let big = dir.join("big.txt");
        std::fs::write(&big, vec![b'a'; MAX_BYTES as usize + 1]).unwrap();
        assert_eq!(read(&big).unwrap_err(), error::TOO_LARGE);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
