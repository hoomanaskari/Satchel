# Satchel architecture and contracts

Satchel is an open source (MIT), cross-platform clipboard manager built with
Tauri 2. It keeps a searchable history of everything you copy, lets you pin
clips to named pinboards, and pastes any clip into the frontmost app from a
shelf that slides over the screen. Target feature set: parity with the Paste
app, minus cloud sync (out of scope for the first version).

Platform order: macOS first, then Windows, then Linux (X11 first; Wayland is
best-effort). All platform-specific code sits behind `cfg` gates with a
compiling fallback on the other platforms.

## Priorities, in order

1. **Security and privacy.** Clipboard history contains passwords and secrets.
2. **Performance.** Lowest idle RAM and CPU, fastest response.
3. Feature parity.

### Security rules (non-negotiable)

- No network access of any kind. No telemetry, no update pings, no remote
  fonts or scripts. Do not add HTTP, shell, opener or filesystem plugins.
- Strict CSP in `tauri.conf.json`: `default-src 'self'`; no `unsafe-eval`; no
  remote origins. Images load only from the app's own scheme/IPC.
- Tauri capabilities are minimal and scoped to the windows that need them:
  one file per window in `src-tauri/capabilities/` (`shelf.json`,
  `settings.json`), each granting only the commands that window's page calls.
  Every app command is listed in `src-tauri/build.rs`, so it is denied unless
  a capability grants it.
- Clip content is untrusted input. The UI renders it as text only: never
  `{@html}`, `innerHTML`, or a clip's HTML flavor in the DOM.
- All SQL is parameterized. Database file and its directory are user-only
  (0600 / 0700 on Unix).
- `privacy::should_capture` gates every clipboard change before content is
  read. Concealed / transient / auto-generated pasteboard markers and the
  excluded-app list are honored. When in doubt, do not capture.
- Size caps (`MAX_TEXT_BYTES`, `MAX_IMAGE_BYTES`) are enforced before storing.
- No `unsafe` without a `// SAFETY:` comment; none in `satchel-core`.
- Secrets never go to logs. Do not log clip content at any level.

### Performance budgets

- Idle: ~0% CPU. No timers in the web view while the shelf is hidden. The
  clipboard watcher is the only periodic work: on macOS a `changeCount` poll
  (no allocation when unchanged, 250–500 ms interval); on Windows the
  clipboard format listener (event driven); on Linux X11 XFixes events.
- Shelf appears in under 100 ms from the shortcut: the window is created once
  at startup and shown/hidden, never recreated.
- Search over 10,000 clips returns in under 30 ms (SQLite FTS5).
- Lists carry `ClipSummary` only (preview capped at 300 chars); full content
  and image bytes are fetched one clip at a time. The shelf windows its list
  (renders only visible cards) and pages with the keyset cursor.
- Thumbnails are generated once at capture and stored.
- Small dependency tree. Justify every new crate or npm package.

## Layout and ownership

```
crates/satchel-core/   pure Rust: models, classify, privacy, store   (core session)
src-tauri/             Tauri app: watcher, paste, hotkey, tray,
                       windows, IPC commands                          (platform session)
src/                   Svelte 5 + TypeScript front end                (UI session)
docs/design/           the approved design prototypes (reference only)
```

Each session edits only its own area. `Cargo.toml` at the root and
`docs/ARCHITECTURE.md` belong to the conductor; ask before changing them.
Each area's own manifest (`crates/satchel-core/Cargo.toml`,
`src-tauri/Cargo.toml`, `package.json`) belongs to that area's session.

## Core API

The public signatures in `crates/satchel-core/src/*.rs` are the contract.
Types serialize in camelCase.

Behaviors callers rely on:

- `Settings.max_items == 0` means no limit. Pruning counts unpinned clips
  only and measures age from `last_used_at`.
- `touch`, `set_title`, `rename_pinboard`, `delete_pinboard` and `pin` return
  `NotFound` for missing ids; `pin` and `unpin` are idempotent.
- `Store::open` forces its parent directory to 0700, so it must be given
  Satchel's own data directory.
- Search matches word prefixes, ANDed; input with no words does not filter.
- Windows: `CanIncludeInClipboardHistory` opts out only when its value is 0,
  but `should_capture` sees names only and treats its presence as an opt-out.
  The watcher must pass that format name only when the value is 0.

## IPC contract (front end ⇄ Rust)

Commands (`invoke` names, snake_case; argument keys camelCase):

| Command | Arguments | Returns |
|---|---|---|
| `list_clips` | `query: Query` | `ClipSummary[]` |
| `get_clip_text` | `id` | `string \| null` (full text, for preview/edit) |
| `get_clip_thumbnail` | `id` | raw PNG bytes (`ArrayBuffer`) |
| `paste_clip` | `id, plainText: boolean` | `void` — hides the shelf, writes the clipboard, pastes into the previously frontmost app, bumps the clip |
| `paste_clips` | `ids: number[], plainText: boolean` | `void` — pastes several clips in order (multi-select / paste stack) |
| `copy_clip` | `id, plainText: boolean` | `void` — writes the clipboard without pasting |
| `delete_clip` | `id` | `boolean` |
| `set_clip_title` | `id, title: string \| null` | `void` |
| `clear_history` | — | `number` (removed count) |
| `list_pinboards` | — | `Pinboard[]` |
| `create_pinboard` | `name, color` | `Pinboard` |
| `rename_pinboard` | `id, name` | `void` |
| `delete_pinboard` | `id` | `void` |
| `pin_clip` | `clipId, pinboardId` | `void` |
| `unpin_clip` | `clipId, pinboardId` | `void` |
| `get_settings` | — | `Settings` |
| `save_settings` | `settings: Settings` | `void` |
| `set_capture_paused` | `paused: boolean` | `void` |
| `hide_shelf` | — | `void` |
| `open_settings` | — | `void` — hides the shelf (focus is not handed back) and opens or focuses the settings window |
| `close_settings` | — | `void` — closes the settings window (Escape / Cmd+W inside it) |

Errors are returned as a string message (never containing clip content).

Events emitted by Rust:

| Event | Payload | When |
|---|---|---|
| `clips-changed` | `{ reason: "inserted" \| "bumped" \| "deleted" \| "cleared" \| "updated", id?: number }` | history changed |
| `pinboards-changed` | — | pinboards or membership changed |
| `shelf-shown` | — | the shelf was just shown; UI resets search, selects the first clip and focuses the search field |
| `settings-changed` | `Settings` | settings saved (sent to every window: the shelf picks up changes made in the settings window) |

## Windows

- `shelf`: borderless, transparent, always-on-top, full-width panel docked to
  the bottom of the active screen, hidden at startup, skip taskbar. Must not
  activate the app or steal focus from the frontmost app more than needed to
  take keyboard input; on hide, focus returns to the previous app so the paste
  lands there.
- `settings`: a regular titled window (680×520 logical, not resizable,
  centered) loading the `/settings` route (prerendered as `settings.html`).
  Created on demand and destroyed on close, unlike the shelf: it is rarely
  open and its web view's memory is given back. Single instance: opening it
  again shows and focuses the existing one. Opened from the tray, the
  shelf's More menu and Cmd/Ctrl+, in the shelf. Satchel stays an accessory
  app, so opening it activates Satchel (needed for a normal window to take
  keyboard input); on close, focus goes back to the app that was in front
  when it opened, unless the user has already switched elsewhere. Closing it
  never quits Satchel.
- Capabilities: `shelf` may use the clip, pinboard and paste commands plus
  `get_settings`, `set_capture_paused`, `hide_shelf` and `open_settings`;
  `settings` may use only `get_settings`, `save_settings`,
  `set_capture_paused`, `clear_history` and `close_settings`. Both may
  listen to events. Neither gets plugin or core window permissions.
- Tray / menu bar icon with: Show Satchel, Settings…, Pause Capturing, Clear
  History, Quit. No dock icon on macOS (accessory activation policy).

## Shelf behavior (matches the prototype in `docs/design/`)

- Opens with the global shortcut (default `CommandOrControl+Shift+V`).
- Search field focused on open; typing filters; scope buttons filter by kind;
  tabs switch between History and each pinboard.
- Left/Right arrows move the selection; Return pastes; Shift+Return pastes as
  plain text; Cmd/Ctrl+1…9 pastes the nth visible clip; Cmd/Ctrl+C copies
  without pasting; Cmd/Ctrl+P pins; Cmd/Ctrl+Backspace deletes; Space toggles
  a larger preview; Shift/Cmd-click multi-selects; Escape closes.
- Cards show source app, kind, relative time, and a preview by kind (text,
  link, code in monospace, color swatch, image thumbnail, file names).

## Testing

Test-driven: write the failing test first, then the code. Core logic lives in
pure functions and in `Store` against an in-memory database so tests need no
OS or UI. Platform code keeps OS calls behind small traits so the logic around
them is tested with fakes. Front-end logic (state, filtering, keyboard
handling, formatting) lives in plain TypeScript modules tested with Vitest,
with the IPC layer mocked.

Commands: `cargo test -p satchel-core`, `cargo test -p satchel`, `pnpm test`.
