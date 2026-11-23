//! The global shortcut that toggles the shelf.
//!
//! Parsing and the swap rules live here as plain functions so they are tested
//! without the OS; `TauriShortcuts` is the thin adapter over the official
//! global-shortcut plugin.

use std::str::FromStr;

use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

use crate::error::{CmdResult, invalid};

pub const MAX_ACCELERATOR_CHARS: usize = 64;

/// Parses an accelerator and rejects shortcuts that would swallow ordinary
/// typing: a plain or Shift-only key is refused unless it is a function key.
pub fn parse(accelerator: &str) -> CmdResult<Shortcut> {
    let text = accelerator.trim();
    if text.is_empty() || text.chars().count() > MAX_ACCELERATOR_CHARS {
        return Err(invalid("shortcut"));
    }
    let shortcut = Shortcut::from_str(text).map_err(|_| invalid("shortcut"))?;
    let strong = Modifiers::ALT | Modifiers::CONTROL | Modifiers::SUPER | Modifiers::META;
    if !shortcut.mods.intersects(strong) && !is_function_key(shortcut.key) {
        return Err(invalid("shortcut needs Command, Control or Option"));
    }
    Ok(shortcut)
}

/// Validated, trimmed accelerator string (what gets stored in `Settings`).
pub fn normalize(accelerator: &str) -> CmdResult<String> {
    parse(accelerator)?;
    Ok(accelerator.trim().to_string())
}

fn is_function_key(code: Code) -> bool {
    use Code::*;
    matches!(
        code,
        F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12 | F13 | F14 | F15 | F16
            | F17 | F18 | F19 | F20 | F21 | F22 | F23 | F24
    )
}

/// What the app needs from the OS hotkey service.
pub trait ShortcutRegistrar: Send + Sync {
    fn register(&self, accelerator: &str) -> CmdResult<()>;
    fn unregister(&self, accelerator: &str);
}

/// Moves the shelf toggle from `old` to `new`. The new shortcut is registered
/// first so a failure (taken by another app, invalid) leaves the old one
/// working.
pub fn swap(reg: &dyn ShortcutRegistrar, old: &str, new: &str) -> CmdResult<()> {
    let new_parsed = parse(new)?;
    if let Ok(old_parsed) = parse(old)
        && old_parsed == new_parsed
    {
        return Ok(());
    }
    reg.register(new)?;
    reg.unregister(old);
    Ok(())
}

pub struct TauriShortcuts<R: tauri::Runtime> {
    pub app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> ShortcutRegistrar for TauriShortcuts<R> {
    fn register(&self, accelerator: &str) -> CmdResult<()> {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        let shortcut = parse(accelerator)?;
        self.app
            .global_shortcut()
            .register(shortcut)
            .map_err(|_| "shortcut is unavailable (used by another app?)".to_string())
    }

    fn unregister(&self, accelerator: &str) {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(shortcut) = parse(accelerator) {
            let _ = self.app.global_shortcut().unregister(shortcut);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn default_shortcut_parses() {
        let s = parse("CommandOrControl+Shift+V").unwrap();
        assert_eq!(s.key, Code::KeyV);
        assert!(s.mods.contains(Modifiers::SHIFT));
    }

    #[test]
    fn rejects_typing_keys_and_garbage() {
        for bad in ["V", "Shift+V", "", "   ", "Ctrl+", "Ctrl+Shift+V+A", "Hyper+Q", "Ctrl+NoSuchKey"] {
            assert!(parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(parse(&format!("Ctrl+{}", "A".repeat(70))).is_err());
    }

    #[test]
    fn accepts_modified_and_function_keys() {
        for good in ["Alt+Space", "Control+Backquote", "F13", "Shift+F5", "Cmd+Option+P"] {
            assert!(parse(good).is_ok(), "{good:?} should be accepted");
        }
    }

    #[test]
    fn normalize_trims() {
        assert_eq!(normalize("  Alt+Space ").unwrap(), "Alt+Space");
    }

    #[derive(Default)]
    struct FakeReg {
        calls: Mutex<Vec<String>>,
        refuse: Option<&'static str>,
    }

    impl ShortcutRegistrar for FakeReg {
        fn register(&self, accelerator: &str) -> CmdResult<()> {
            self.calls.lock().unwrap().push(format!("register {accelerator}"));
            if self.refuse == Some(accelerator) {
                Err("taken".into())
            } else {
                Ok(())
            }
        }
        fn unregister(&self, accelerator: &str) {
            self.calls.lock().unwrap().push(format!("unregister {accelerator}"));
        }
    }

    #[test]
    fn swap_registers_new_before_dropping_old() {
        let reg = FakeReg::default();
        swap(&reg, "CommandOrControl+Shift+V", "Alt+Space").unwrap();
        assert_eq!(
            *reg.calls.lock().unwrap(),
            vec!["register Alt+Space", "unregister CommandOrControl+Shift+V"]
        );
    }

    #[test]
    fn swap_keeps_old_when_new_is_taken() {
        let reg = FakeReg { refuse: Some("Alt+Space"), ..FakeReg::default() };
        assert!(swap(&reg, "CommandOrControl+Shift+V", "Alt+Space").is_err());
        assert_eq!(*reg.calls.lock().unwrap(), vec!["register Alt+Space"]);
    }

    #[test]
    fn swap_is_a_no_op_for_equivalent_shortcuts() {
        let reg = FakeReg::default();
        swap(&reg, "CommandOrControl+Shift+V", "commandorcontrol+shift+v").unwrap();
        assert!(reg.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn swap_rejects_invalid_new_without_touching_os() {
        let reg = FakeReg::default();
        assert!(swap(&reg, "CommandOrControl+Shift+V", "V").is_err());
        assert!(reg.calls.lock().unwrap().is_empty());
    }
}
