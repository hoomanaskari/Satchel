//! Which Satchel this is: the installed app or Satchel Dev.
//!
//! Both are built from the same code. `pnpm release` builds Satchel from
//! `tauri.conf.json` as it is; `pnpm app` and `pnpm tauri dev` lay
//! `tauri.dev.conf.json` over it, which changes the product name and the
//! bundle identifier. Everything keyed to the identifier then differs by
//! itself (data folder, WebKit data, the Accessibility grant, Launch
//! Services); this module carries the rest: the names the user reads (menu
//! bar, window titles, the Accessibility explanation), the nspasteboard.org
//! source marker, the menu bar tint and Satchel Dev's own default shortcut,
//! so it can run next to an installed Satchel.
//!
//! [`init`] runs once at startup with the bundled config's values. Until
//! then (and in unit tests) everything answers for the installed Satchel.

use std::sync::OnceLock;

use satchel_core::Settings;

/// The installed Satchel's bundle identifier and name (`tauri.conf.json`).
pub const PRODUCTION_ID: &str = "app.satchel.desktop";
pub const PRODUCTION_NAME: &str = "Satchel";

/// Satchel Dev's identifiers end with this (`tauri.dev.conf.json`).
const DEV_SUFFIX: &str = ".dev";

/// Satchel Dev's shelf shortcut when it starts with a new history: ⌥⇧⌘V,
/// one key away from the installed Satchel's ⇧⌘V, so both can run at once
/// and each shortcut opens its own shelf. Changeable in Settings like any
/// shortcut.
pub const DEV_SHORTCUT: &str = "CommandOrControl+Alt+Shift+V";

struct Identity {
    id: String,
    name: String,
}

static IDENTITY: OnceLock<Identity> = OnceLock::new();

/// Records the bundle identifier and product name from the app's config.
pub fn init(id: &str, name: &str) {
    let _ = IDENTITY.set(Identity { id: id.to_string(), name: name.to_string() });
}

/// The bundle identifier (`app.satchel.desktop` or `app.satchel.desktop.dev`).
pub fn id() -> &'static str {
    IDENTITY.get().map_or(PRODUCTION_ID, |i| i.id.as_str())
}

/// The name people see: "Satchel" or "Satchel Dev".
pub fn name() -> &'static str {
    IDENTITY.get().map_or(PRODUCTION_NAME, |i| i.name.as_str())
}

/// True for Satchel Dev.
pub fn is_dev() -> bool {
    is_dev_id(id())
}

pub fn is_dev_id(id: &str) -> bool {
    id.ends_with(DEV_SUFFIX)
}

/// The shortcut this Satchel starts with, and falls back to when the saved
/// one cannot be registered.
pub fn default_shortcut() -> String {
    default_shortcut_for(id())
}

/// Only the installed Satchel starts with ⇧⌘V; every other build (Satchel
/// Dev, the update rehearsal's "Satchel Rehearsal") starts with ⌥⇧⌘V, so
/// it never opens alongside the installed Satchel's shelf.
fn default_shortcut_for(id: &str) -> String {
    if id == PRODUCTION_ID { Settings::default().shelf_shortcut } else { DEV_SHORTCUT.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_for_the_installed_satchel_until_initialized() {
        // `init` is never called in unit tests.
        assert_eq!(id(), PRODUCTION_ID);
        assert_eq!(name(), PRODUCTION_NAME);
        assert!(!is_dev());
        assert_eq!(default_shortcut(), Settings::default().shelf_shortcut);
    }

    #[test]
    fn satchel_dev_has_its_own_default_shortcut() {
        assert!(is_dev_id("app.satchel.desktop.dev"));
        assert!(!is_dev_id(PRODUCTION_ID));
        assert_eq!(default_shortcut_for("app.satchel.desktop.dev"), DEV_SHORTCUT);
        assert_eq!(default_shortcut_for("app.satchel.desktop.rehearsal"), DEV_SHORTCUT);
        assert_eq!(default_shortcut_for(PRODUCTION_ID), Settings::default().shelf_shortcut);
        assert_ne!(DEV_SHORTCUT, Settings::default().shelf_shortcut);
        assert_eq!(crate::shortcut::normalize(DEV_SHORTCUT).unwrap(), DEV_SHORTCUT);
    }

    #[test]
    fn matches_the_tauri_configs() {
        let read = |path: &str| -> serde_json::Value {
            let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/").to_string() + path).unwrap();
            serde_json::from_str(&text).unwrap()
        };
        let prod = read("tauri.conf.json");
        let dev = read("tauri.dev.conf.json");
        assert_eq!(prod["identifier"], PRODUCTION_ID);
        assert_eq!(prod["productName"], PRODUCTION_NAME);
        let dev_id = dev["identifier"].as_str().unwrap();
        assert!(is_dev_id(dev_id) && dev_id.starts_with(PRODUCTION_ID));
        assert_eq!(dev["productName"], "Satchel Dev");
    }

    #[test]
    fn only_the_installed_satchel_has_an_update_feed() {
        let read = |path: &str| -> serde_json::Value {
            let text = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            serde_json::from_str(&text).unwrap()
        };
        let updater = &read("tauri.conf.json")["plugins"]["updater"];
        assert_eq!(
            updater["endpoints"],
            serde_json::json!(["https://github.com/hoomanaskari/Satchel/releases/latest/download/latest.json"])
        );
        assert_eq!(updater["requireSignedVersion"], true);
        assert!(updater["pubkey"].as_str().is_some_and(|k| k.len() > 100));
        let keys: Vec<&String> = updater.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3, "no dangerous or downgrade options: {keys:?}");
        assert_eq!(read("tauri.dev.conf.json")["plugins"]["updater"]["endpoints"], serde_json::json!([]));
    }
}
