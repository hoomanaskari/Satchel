//! Events sent to the front end (see the IPC contract).

use serde::Serialize;

use satchel_core::{ClipId, ClipKind, Settings};

pub const CLIPS_CHANGED: &str = "clips-changed";
pub const PINBOARDS_CHANGED: &str = "pinboards-changed";
pub const SHELF_SHOWN: &str = "shelf-shown";
pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const CAPTURE_SKIPPED: &str = "capture-skipped";

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeReason {
    Inserted,
    Bumped,
    Deleted,
    Cleared,
    Updated,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ClipsChanged {
    pub reason: ChangeReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<ClipId>,
}

/// Sent when a copy was deliberately not kept because it is over a size cap,
/// so the shelf can say so instead of the clip silently missing. Carries no
/// content, only the kind and the cap.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSkipped {
    pub kind: ClipKind,
    pub limit_bytes: u64,
}

/// Where state changes are announced. The Tauri implementation emits events
/// (and keeps the tray in sync); tests record them.
pub trait EventSink: Send + Sync {
    fn clips_changed(&self, reason: ChangeReason, id: Option<ClipId>);
    fn pinboards_changed(&self);
    fn settings_changed(&self, settings: &Settings);
    /// A copy of `kind` was over the cap `limit_bytes` and was not stored.
    fn capture_skipped(&self, kind: ClipKind, limit_bytes: u64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_changed_payload_matches_contract() {
        let with_id = ClipsChanged { reason: ChangeReason::Inserted, id: Some(7) };
        assert_eq!(
            serde_json::to_string(&with_id).unwrap(),
            r#"{"reason":"inserted","id":7}"#
        );
        let without = ClipsChanged { reason: ChangeReason::Cleared, id: None };
        assert_eq!(serde_json::to_string(&without).unwrap(), r#"{"reason":"cleared"}"#);
    }

    #[test]
    fn capture_skipped_payload_matches_contract() {
        let p = CaptureSkipped { kind: ClipKind::Image, limit_bytes: 64 };
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"kind":"image","limitBytes":64}"#);
    }
}
