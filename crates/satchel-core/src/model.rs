//! Data types shared by the store, the Tauri commands and (via serde, in
//! camelCase) the front end.

use serde::{Deserialize, Serialize};

pub type ClipId = i64;
pub type PinboardId = i64;
/// Milliseconds since the Unix epoch.
pub type Millis = i64;

/// Hard caps applied before anything is stored. `MAX_TEXT_BYTES` also caps
/// the HTML and RTF flavors of a text clip (an oversized flavor is dropped,
/// the text kept).
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
/// Largest image kept, measured as the stored PNG. Big enough for a full
/// 6K Retina screenshot or a 12 MP photo converted from JPEG/HEIC; larger
/// images are skipped and the shelf says so (they are never truncated or
/// downscaled).
pub const MAX_IMAGE_BYTES: u64 = 64_000_000;
/// Maximum characters kept in `ClipSummary::preview`.
pub const PREVIEW_CHARS: usize = 300;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Text,
    Link,
    Code,
    Color,
    Image,
    File,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceApp {
    /// Bundle identifier on macOS, executable name elsewhere.
    pub id: String,
    pub name: String,
}

/// What the clipboard watcher hands to the store. `kind` is decided by the
/// caller using `classify` (text-like payloads) or the payload type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewClip {
    pub kind: ClipKind,
    pub text: Option<String>,
    pub html: Option<String>,
    /// RTF flavor of a text clip (TextEdit, Notes, Pages... write RTF rather
    /// than HTML). Pasted with the text unless pasting as plain text.
    pub rtf: Option<Vec<u8>>,
    /// PNG-encoded image data.
    pub image_png: Option<Vec<u8>>,
    /// PNG thumbnail, longest side at most 320 px, made by the caller.
    pub thumbnail_png: Option<Vec<u8>>,
    pub file_paths: Vec<String>,
    pub source_app: Option<SourceApp>,
}

/// Lightweight row for lists. Never carries full content or image bytes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClipSummary {
    pub id: ClipId,
    pub kind: ClipKind,
    /// User-given title, if any.
    pub title: Option<String>,
    /// At most `PREVIEW_CHARS` characters of the text (file paths for files,
    /// normalized hex for colors, pixel dimensions such as `1200 × 800` for
    /// images).
    pub preview: String,
    pub source_app: Option<SourceApp>,
    pub created_at: Millis,
    pub last_used_at: Millis,
    pub byte_size: u64,
    pub has_thumbnail: bool,
    pub pinboard_ids: Vec<PinboardId>,
    /// The fetched preview of a link clip, when "Show link previews" is on
    /// and the page could be read. `None` otherwise.
    pub link_preview: Option<LinkPreview>,
}

/// What a link's page says about itself, cached in the store.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinkPreview {
    /// Page title (`og:title`, `twitter:title` or `<title>`), cleaned.
    pub title: Option<String>,
    /// Site name (`og:site_name`), else the host without `www.`.
    pub site: String,
    /// What `link_preview_image` returns, if anything.
    pub image: Option<LinkImageKind>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LinkImageKind {
    /// The page's preview image, scaled for the card.
    Photo,
    /// The site's icon (no preview image).
    Icon,
}

/// A fetched page, ready to cache (`Store::finish_link_preview`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedPreview {
    pub title: Option<String>,
    pub site: String,
    /// PNG bytes and what they show.
    pub image: Option<(LinkImageKind, Vec<u8>)>,
}

/// How a link preview attempt ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkPreviewOutcome {
    Ready(FetchedPreview),
    /// A transient failure (offline, timeout, 5xx, 429): try again at
    /// `retry_at`, unless attempts are used up.
    Retry { retry_at: Millis },
    /// Not retried: the page refused, is not HTML, is too large, or its
    /// address is not public.
    Failed,
    /// Never fetched: the link failed the privacy checks, or its app is
    /// excluded.
    Skipped,
}

/// A link clip whose preview is due, claimed by one worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPreviewJob {
    pub clip_id: ClipId,
    pub url: String,
    pub source_app_id: Option<String>,
    /// Attempts made before this one.
    pub attempts: u32,
}

/// Full payload, fetched only when pasting or previewing one clip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipContent {
    pub id: ClipId,
    pub kind: ClipKind,
    pub text: Option<String>,
    pub html: Option<String>,
    pub rtf: Option<Vec<u8>>,
    pub image_png: Option<Vec<u8>>,
    pub file_paths: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Pinboard {
    pub id: PinboardId,
    pub name: String,
    /// `#RRGGBB`.
    pub color: String,
    pub position: i64,
}

/// Keyset-paginated list query. Results are newest first by
/// `(last_used_at, id)`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Query {
    /// Full-text search over text, title and source app name.
    pub text: Option<String>,
    pub kind: Option<ClipKind>,
    /// Restrict to one pinboard. `None` means the whole history.
    pub pinboard_id: Option<PinboardId>,
    /// Clamped to 1..=200 by the store; 0 means the default of 50.
    pub limit: u32,
    /// Return rows strictly older than this `(last_used_at, id)` cursor.
    pub before: Option<(Millis, ClipId)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Unpinned clips beyond this count are pruned, oldest first.
    pub max_items: u32,
    /// Unpinned clips older than this many days are pruned. `None` keeps them.
    pub retention_days: Option<u32>,
    /// Source app ids that are never captured (password managers etc.).
    pub excluded_app_ids: Vec<String>,
    pub capture_paused: bool,
    pub capture_images: bool,
    /// Global shortcut that toggles the shelf, in Tauri accelerator syntax.
    pub shelf_shortcut: String,
    /// Paste into the frontmost app on selection instead of only copying.
    pub paste_on_select: bool,
    /// What a normal paste does with a text clip's formatting (HTML, RTF):
    /// false keeps it, true pastes plain text. The other paste action
    /// (Shift+Return, the second button) does the opposite. Off by default,
    /// including for settings saved before it existed.
    pub paste_plain_text: bool,
    /// Fetch the title and image of copied web links (the only feature that
    /// uses the network). On by default, including for settings saved before
    /// it existed; links that look private or one-time are never fetched.
    pub link_previews: bool,
    /// Ask GitHub Releases for a newer Satchel at launch and every 6 hours
    /// (the installed Satchel only; Satchel Dev never checks). On by
    /// default, including for settings saved before it existed. Off: no
    /// background request at all; "Check for Updates…" still works.
    pub check_for_updates: bool,
    /// Open Satchel at login (a login item, macOS 13+, the installed app in
    /// a stable location only). `None`: not decided yet, which means on: the
    /// first launch that can register does so, then stores the result,
    /// including for settings saved before it existed. `Some`: the last
    /// known state; the system's Login Items list wins over it and it is
    /// never registered again on its own. Stored only by
    /// `Store::save_open_at_login`; `save_settings` leaves it as it is.
    pub open_at_login: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_items: 2000,
            retention_days: Some(90),
            excluded_app_ids: crate::privacy::default_excluded_app_ids(),
            capture_paused: false,
            capture_images: true,
            shelf_shortcut: "CommandOrControl+Shift+V".to_string(),
            paste_on_select: true,
            paste_plain_text: false,
            link_previews: true,
            check_for_updates: true,
            open_at_login: None,
        }
    }
}

/// Result of `Store::insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertOutcome {
    /// A new row was created.
    Inserted(ClipSummary),
    /// Identical content already existed; it was moved to the top.
    Bumped(ClipSummary),
}
