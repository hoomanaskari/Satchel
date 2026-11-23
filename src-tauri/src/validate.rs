//! Validation of every value that arrives over IPC. The web view is treated as
//! untrusted: a compromised or buggy front end must not be able to push
//! oversized or malformed values into the store.

use satchel_core::{ClipId, PinboardId, Query, Settings};

use crate::error::{CmdResult, invalid};
use crate::shortcut;

pub const MAX_IDS_PER_CALL: usize = 100;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_PINBOARD_NAME_CHARS: usize = 64;
pub const MAX_QUERY_CHARS: usize = 512;
pub const MAX_QUERY_LIMIT: u32 = 200;
pub const MIN_MAX_ITEMS: u32 = 10;
pub const MAX_MAX_ITEMS: u32 = 100_000;
pub const MAX_RETENTION_DAYS: u32 = 3650;
pub const MAX_EXCLUDED_APPS: usize = 500;
pub const MAX_APP_ID_CHARS: usize = 255;

/// One app id as stored in the excluded-app list (trimmed).
pub fn app_id(raw: &str) -> CmdResult<String> {
    let id = raw.trim();
    if id.is_empty() || id.chars().count() > MAX_APP_ID_CHARS || has_control_chars(id) {
        return Err(invalid("appId"));
    }
    Ok(id.to_string())
}

/// Up to `MAX_EXCLUDED_APPS` app ids, order kept (answers line up with the
/// request).
pub fn app_ids(raw: &[String]) -> CmdResult<Vec<String>> {
    if raw.len() > MAX_EXCLUDED_APPS {
        return Err(invalid("appIds"));
    }
    raw.iter().map(|id| app_id(id)).collect()
}

pub fn id(value: i64) -> CmdResult<i64> {
    if value > 0 {
        Ok(value)
    } else {
        Err(invalid("id"))
    }
}

pub fn clip_id(value: ClipId) -> CmdResult<ClipId> {
    id(value)
}

pub fn pinboard_id(value: PinboardId) -> CmdResult<PinboardId> {
    id(value)
}

/// Non-empty, bounded list of valid ids. Order is kept (paste order);
/// duplicates are allowed because pasting the same clip twice is legitimate.
pub fn clip_ids(values: &[ClipId]) -> CmdResult<Vec<ClipId>> {
    if values.is_empty() || values.len() > MAX_IDS_PER_CALL {
        return Err(invalid("ids"));
    }
    values.iter().map(|&v| clip_id(v)).collect()
}

fn has_control_chars(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Trims; an empty title clears it.
pub fn title(value: Option<String>) -> CmdResult<Option<String>> {
    let Some(raw) = value else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_TITLE_CHARS || has_control_chars(trimmed) {
        return Err(invalid("title"));
    }
    Ok(Some(trimmed.to_string()))
}

pub fn pinboard_name(value: &str) -> CmdResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_PINBOARD_NAME_CHARS
        || has_control_chars(trimmed)
    {
        return Err(invalid("name"));
    }
    Ok(trimmed.to_string())
}

/// `#RRGGBB`, normalized to uppercase.
pub fn color(value: &str) -> CmdResult<String> {
    let v = value.trim();
    let ok = v.len() == 7
        && v.starts_with('#')
        && v[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if ok {
        Ok(v.to_ascii_uppercase())
    } else {
        Err(invalid("color"))
    }
}

pub fn query(mut q: Query) -> CmdResult<Query> {
    if let Some(text) = &q.text {
        if text.chars().count() > MAX_QUERY_CHARS {
            return Err(invalid("query text"));
        }
        let trimmed = text.trim();
        q.text = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
    if q.limit > MAX_QUERY_LIMIT {
        return Err(invalid("limit"));
    }
    if let Some(pb) = q.pinboard_id {
        pinboard_id(pb)?;
    }
    if let Some((at, before_id)) = q.before
        && (at < 0 || before_id <= 0)
    {
        return Err(invalid("cursor"));
    }
    Ok(q)
}

/// Validates and normalizes settings coming from the UI.
pub fn settings(mut s: Settings) -> CmdResult<Settings> {
    // 0 means "no limit" (core contract).
    if s.max_items != 0 && !(MIN_MAX_ITEMS..=MAX_MAX_ITEMS).contains(&s.max_items) {
        return Err(invalid("maxItems"));
    }
    if let Some(days) = s.retention_days
        && !(1..=MAX_RETENTION_DAYS).contains(&days)
    {
        return Err(invalid("retentionDays"));
    }
    if s.excluded_app_ids.len() > MAX_EXCLUDED_APPS {
        return Err(invalid("excludedAppIds"));
    }
    let mut ids: Vec<String> = Vec::with_capacity(s.excluded_app_ids.len());
    for raw in &s.excluded_app_ids {
        let id = app_id(raw).map_err(|_| invalid("excludedAppIds"))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    s.excluded_app_ids = ids;
    s.shelf_shortcut = shortcut::normalize(&s.shelf_shortcut)?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::sample_settings;

    #[test]
    fn app_ids_are_trimmed_bounded_and_free_of_control_characters() {
        assert_eq!(app_id(" com.apple.TextEdit "), Ok("com.apple.TextEdit".into()));
        assert!(app_id("   ").is_err());
        assert!(app_id("a\nb").is_err());
        assert!(app_id(&"x".repeat(MAX_APP_ID_CHARS + 1)).is_err());
        assert_eq!(app_ids(&["a.b".into(), " c.d".into()]), Ok(vec!["a.b".into(), "c.d".into()]));
        assert!(app_ids(&vec!["a.b".to_string(); MAX_EXCLUDED_APPS + 1]).is_err());
        assert!(app_ids(&["a.b".into(), "".into()]).is_err());
    }

    #[test]
    fn ids_must_be_positive() {
        assert_eq!(id(1), Ok(1));
        assert!(id(0).is_err());
        assert!(id(-5).is_err());
    }

    #[test]
    fn id_lists_are_bounded_and_keep_order() {
        assert!(clip_ids(&[]).is_err());
        assert_eq!(clip_ids(&[3, 1, 3]), Ok(vec![3, 1, 3]));
        assert!(clip_ids(&[1, 0]).is_err());
        let many: Vec<i64> = (1..=101).collect();
        assert!(clip_ids(&many).is_err());
        assert!(clip_ids(&many[..100]).is_ok());
    }

    #[test]
    fn titles_are_trimmed_bounded_and_clearable() {
        assert_eq!(title(None), Ok(None));
        assert_eq!(title(Some("   ".into())), Ok(None));
        assert_eq!(title(Some("  Hello ".into())), Ok(Some("Hello".into())));
        assert!(title(Some("a\u{0}b".into())).is_err());
        assert!(title(Some("line\nbreak".into())).is_err());
        assert!(title(Some("x".repeat(201))).is_err());
        assert!(title(Some("é".repeat(200))).is_ok());
    }

    #[test]
    fn pinboard_names() {
        assert_eq!(pinboard_name(" Work "), Ok("Work".into()));
        assert!(pinboard_name("").is_err());
        assert!(pinboard_name("  ").is_err());
        assert!(pinboard_name(&"n".repeat(65)).is_err());
        assert!(pinboard_name("tab\there").is_err());
    }

    #[test]
    fn colors_are_six_digit_hex() {
        assert_eq!(color("#ff9f0a"), Ok("#FF9F0A".into()));
        assert!(color("ff9f0a").is_err());
        assert!(color("#fff").is_err());
        assert!(color("#gggggg").is_err());
        assert!(color("#ff9f0a00").is_err());
        assert!(color("#ÿÿÿ").is_err());
    }

    #[test]
    fn queries_are_bounded_and_normalized() {
        let q = query(Query {
            text: Some("  foo ".into()),
            limit: 50,
            ..Query::default()
        })
        .unwrap();
        assert_eq!(q.text.as_deref(), Some("foo"));

        let blank = query(Query {
            text: Some("   ".into()),
            ..Query::default()
        })
        .unwrap();
        assert_eq!(blank.text, None);

        assert!(query(Query { text: Some("x".repeat(513)), ..Query::default() }).is_err());
        assert!(query(Query { limit: 201, ..Query::default() }).is_err());
        assert!(query(Query { limit: 0, ..Query::default() }).is_ok());
        assert!(query(Query { pinboard_id: Some(0), ..Query::default() }).is_err());
        assert!(query(Query { before: Some((10, 0)), ..Query::default() }).is_err());
        assert!(query(Query { before: Some((-1, 4)), ..Query::default() }).is_err());
        assert!(query(Query { before: Some((10, 4)), ..Query::default() }).is_ok());
    }

    #[test]
    fn settings_ranges() {
        assert!(settings(sample_settings()).is_ok());
        assert!(settings(Settings { max_items: 0, ..sample_settings() }).is_ok(), "0 = no limit");
        assert!(settings(Settings { max_items: 5, ..sample_settings() }).is_err());
        assert!(settings(Settings { max_items: 100_001, ..sample_settings() }).is_err());
        assert!(settings(Settings { retention_days: Some(0), ..sample_settings() }).is_err());
        assert!(settings(Settings { retention_days: None, ..sample_settings() }).is_ok());
        assert!(settings(Settings { retention_days: Some(3651), ..sample_settings() }).is_err());
    }

    #[test]
    fn settings_excluded_apps_are_trimmed_and_deduplicated() {
        let s = settings(Settings {
            excluded_app_ids: vec![" a.b ".into(), "a.b".into(), "c.d".into()],
            ..sample_settings()
        })
        .unwrap();
        assert_eq!(s.excluded_app_ids, vec!["a.b".to_string(), "c.d".to_string()]);
        assert!(settings(Settings { excluded_app_ids: vec!["".into()], ..sample_settings() }).is_err());
        assert!(
            settings(Settings {
                excluded_app_ids: (0..501).map(|i| format!("app{i}")).collect(),
                ..sample_settings()
            })
            .is_err()
        );
    }

    #[test]
    fn settings_shortcut_must_be_safe() {
        assert!(settings(Settings { shelf_shortcut: "V".into(), ..sample_settings() }).is_err());
        assert!(settings(Settings { shelf_shortcut: "nonsense".into(), ..sample_settings() }).is_err());
        let s = settings(Settings { shelf_shortcut: " Alt+Space ".into(), ..sample_settings() }).unwrap();
        assert_eq!(s.shelf_shortcut, "Alt+Space");
    }
}
