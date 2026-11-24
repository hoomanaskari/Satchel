// Every app command is declared here so it is denied unless a capability
// grants it (see `capabilities/`).
const COMMANDS: &[&str] = &[
    "list_clips",
    "get_clip_text",
    "get_clip_thumbnail",
    "get_link_image",
    "paste_clip",
    "paste_clips",
    "copy_clip",
    "delete_clip",
    "set_clip_title",
    "clear_history",
    "list_pinboards",
    "create_pinboard",
    "rename_pinboard",
    "delete_pinboard",
    "pin_clip",
    "unpin_clip",
    "get_settings",
    "save_settings",
    "set_capture_paused",
    "get_login_item",
    "set_open_at_login",
    "hide_shelf",
    "open_settings",
    "close_settings",
    "open_accessibility_settings",
    "list_apps",
    "resolve_apps",
    "get_app_icon",
    "choose_app",
    "get_third_party_notices",
    "get_update_status",
    "check_for_updates",
    "install_update",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
