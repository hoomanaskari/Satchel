//! Error strings returned over IPC.
//!
//! Every message is a fixed string chosen here. Nothing from the inner error is
//! forwarded, so clip content, file paths or SQL fragments can never reach the
//! front end (or a log) through an error.

use satchel_core::Error as CoreError;

pub const NOT_FOUND: &str = "not found";
pub const INVALID_INPUT: &str = "invalid input";
pub const TOO_LARGE: &str = "clip too large";
pub const STORAGE: &str = "storage error";
pub const INTERNAL: &str = "internal error";
pub const CLIPBOARD: &str = "could not write the clipboard";
/// Prefix the UI can match to show the Accessibility explanation.
pub const ACCESSIBILITY_PREFIX: &str = "accessibility-permission-required";

/// The Accessibility error, naming this app as System Settings lists it
/// ("Satchel" or "Satchel Dev").
pub fn accessibility() -> String {
    let name = crate::app_identity::name();
    format!(
        "{ACCESSIBILITY_PREFIX}: {name} copied the clip but needs Accessibility access to paste it. \
         Press the paste shortcut yourself, or allow {name} in System Settings > Privacy & Security > \
         Accessibility."
    )
}

pub type CmdResult<T> = Result<T, String>;

/// Maps a core error to a content-free message.
pub fn store_error(err: CoreError) -> String {
    match err {
        CoreError::NotFound => NOT_FOUND,
        CoreError::Invalid(_) => INVALID_INPUT,
        CoreError::TooLarge { .. } => TOO_LARGE,
        CoreError::Db(_) | CoreError::Io(_) => STORAGE,
    }
    .to_string()
}

pub fn invalid(what: &str) -> String {
    format!("{INVALID_INPUT}: {what}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_errors_never_carry_inner_text() {
        let secret = "hunter2-password";
        let cases = [
            CoreError::Invalid(secret.to_string()),
            CoreError::Io(std::io::Error::other(secret)),
            CoreError::NotFound,
            CoreError::TooLarge { size: 10, limit: 5 },
        ];
        for case in cases {
            let msg = store_error(case);
            assert!(!msg.contains(secret), "{msg}");
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn mapping_is_specific() {
        assert_eq!(store_error(CoreError::NotFound), NOT_FOUND);
        assert_eq!(store_error(CoreError::Invalid("x".into())), INVALID_INPUT);
        assert_eq!(store_error(CoreError::TooLarge { size: 2, limit: 1 }), TOO_LARGE);
        assert_eq!(store_error(CoreError::Io(std::io::Error::other("x"))), STORAGE);
    }

    #[test]
    fn invalid_names_the_field_only() {
        assert_eq!(invalid("id"), "invalid input: id");
    }
}
