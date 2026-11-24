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

- No network access except link previews, which are on by default and
  run only while "Show link previews" is on (see "Link previews" below);
  turned off, Satchel makes no request at all. No telemetry, no update pings, no remote fonts or
  scripts. The web view never reaches the network: Rust fetches and hands
  over bytes. Do not add HTTP, shell, opener or filesystem plugins.
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
  A copy over a cap is not stored and the shelf says so (`capture-skipped`);
  nothing is truncated or downscaled.
- No `unsafe` without a `// SAFETY:` comment; none in `satchel-core`.
- Secrets never go to logs. Do not log clip content at any level.

### Performance budgets

- Idle: ~0% CPU. No timers in the web view while the shelf is hidden. The
  clipboard watcher is the only periodic work: on macOS a `changeCount` poll
  (no allocation when unchanged, 250–500 ms interval); on Windows the
  clipboard format listener (event driven); on Linux X11 XFixes events.
- Shelf appears in under 100 ms from the shortcut: the window is created once
  at startup and shown/hidden, never recreated. On macOS it is ordered in and
  made key at once (typing and Return work immediately), then slides up from
  the bottom edge: the slide starts `shelf::SHOW_DELAY` (50 ms) later, once
  WebKit has repainted the page, and takes `SHOW_SECONDS` (220 ms, ease-out
  `cubic-bezier(0.25, 0.8, 0.35, 1)`). Dismissing (Escape, the shortcut, a
  click outside or in its margin) slides it down in `HIDE_SECONDS` (160 ms,
  ease-in) without blocking. A paste hides it at once, so focus hand-back and
  the ⌘V keystroke are as fast as without animation. "Reduce motion" turns
  both slides off.
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

## Clip kinds (what is captured, and how it pastes back)

The shelf's filters are All, Text, Links, Code, Images, Colors and Files.
On macOS the reader (`src-tauri/src/clipboard/macos.rs`) reduces each
clipboard change to one representation; type names come from the
pasteboard, nothing is read before the privacy gate.

| Kind | Captured from | Stored | Pastes back as |
|---|---|---|---|
| Text | `public.utf8-plain-text`, with `public.html` and `public.rtf` when present (browsers write HTML; TextEdit, Notes, Pages, Mail, Xcode, Terminal write RTF) | text + HTML + RTF (each ≤ `MAX_TEXT_BYTES`; an oversized flavor is dropped, the text kept). Copying the same text again with formatting replaces the stored formatting | text + HTML + RTF; "Paste as Plain Text" writes the text only |
| Links, Code, Colors | text classified by `classify_text` (a whole-text URL; code heuristics; `#RGB[A]`, `#RRGGBB[AA]`, `rgb()/rgba()/hsl()/hsla()`). Colors also from an NSColor on the pasteboard (color wells, the color panel, pickers), kept as sRGB `#RRGGBB[AA]` | as Text; colors previewed as normalized hex | their text (and formatting, as Text) |
| Images | PNG as is; TIFF, JPEG, HEIC/HEIF, WebP, GIF (first frame) and BMP converted to PNG. Screenshots to the clipboard, Preview, Photos, Pages/Keynote image copies, browsers' "Copy Image" | full-size PNG (≤ `MAX_IMAGE_BYTES` = 64 MB and ≤ 100 MP) + a 320 px thumbnail made once at capture; preview `W × H` | the full PNG (never the thumbnail), plus a TIFF for older apps when the PNG is ≤ 16 MB; "plain" is ignored |
| Files | `public.file-url` items whose files exist (Finder: one or many files, folders) | absolute paths (≤ 1000, each ≤ 4096 bytes) | file URLs (Finder pastes the files, Mail and chat apps attach them); "Paste as Plain Text" writes the paths, one per line |

Precedence when one change carries several representations:

- Files that exist win over text and over a TIFF (Finder adds the file's
  icon as TIFF/ICNS). File URLs that do not exist (promised or
  already-deleted temporary files) are ignored and the change falls through
  to its image or text. Image data in any other format (PNG, JPEG, HEIC...)
  next to file URLs is what an app like Messages or Photos writes for a
  copied picture (plus the URL of its cached copy): the image wins.
- Text wins over an image (rich apps put a picture rendition next to copied
  text), except a browser's "Copy Image": the text is a single token next to
  `public.url` (Safari), or the HTML is only `<img>` markup with no visible
  text (Chrome, Firefox, Electron). Then the image is kept
  (`image_wins_over_text`).
- An image file copied in Finder is a File clip (the file pastes into
  Finder, Mail or a chat as itself); it is not also stored as an image.
- With "Capture images" off, images are skipped and a browser's "Copy
  Image" keeps its URL as text.

Not captured: PDF-only and RTFD-only (text with attachments without a plain
text flavor) clipboards, and app-private types. Concealed / transient /
auto-generated markers and excluded apps skip the change whatever its kind.
Satchel's own writes carry `app.satchel.desktop.self` and are recorded by
change count, so pasting never creates a new clip.

Limits, all deliberate: text, HTML and RTF ≤ 2 MiB each (`MAX_TEXT_BYTES`);
images ≤ 64 MB as PNG and ≤ 100 megapixels (a JPEG/HEIC photo is stored as
its PNG, which is larger than the original: a 12 MP photo is roughly 20–35
MB); TIFF/BMP read up to 4× the image cap before conversion. A text or image
copy over its cap is announced with `capture-skipped` and the shelf shows
"… was not kept: it is over Satchel's … limit." (right away when open,
otherwise the next time it opens). An image that is valid but too large to
thumbnail (an interlaced PNG whose frame is over 64 MB) is kept without a
thumbnail.

`SATCHEL_TRACE=1` logs, per clipboard change, the pasteboard type names and
the capture outcome (never content), to stderr.

## IPC contract (front end ⇄ Rust)

Commands (`invoke` names, snake_case; argument keys camelCase):

| Command | Arguments | Returns |
|---|---|---|
| `list_clips` | `query: Query` | `ClipSummary[]` |
| `get_clip_text` | `id` | `string \| null` (full text, for preview/edit) |
| `get_clip_thumbnail` | `id` | raw PNG bytes (`ArrayBuffer`) |
| `get_link_image` | `id` | raw PNG bytes of a link clip's cached preview image or site icon (`ArrayBuffer`); `not found` when it has none. Never touches the network |
| `paste_clip` | `id, plainText: boolean` | `void` — hides the shelf, writes the clipboard, pastes into the previously frontmost app, bumps the clip |
| `paste_clips` | `ids: number[], plainText: boolean` | `void` — pastes several clips in order (multi-select / paste stack) |
| | | Without the Accessibility permission both copy instead, leave the shelf up and fail with `accessibility-permission-required: …` so the shelf can explain |
| `open_accessibility_settings` | — | `void` — hides the shelf and opens System Settings > Privacy & Security > Accessibility |
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
| `save_settings` | `settings: Settings` | `void` — turning `linkPreviews` off deletes every cached preview (then `clips-changed` `updated`); turning it on wakes the preview workers |
| `set_capture_paused` | `paused: boolean` | `void` |
| `hide_shelf` | — | `void` |
| `open_settings` | — | `void` — hides the shelf (focus is not handed back) and opens or focuses the settings window |
| `close_settings` | — | `void` — closes the settings window (Escape / Cmd+W inside it) |
| `list_apps` | — | `AppInfo[]` (`{ id, name, running }`) — installed apps (macOS: `.app` bundles in `/Applications`, `/System/Applications`, `/System/Library/CoreServices/Applications`, `~/Applications`, one sub-folder deep) merged with running regular apps, one per id (case-insensitive), sorted by name |
| `resolve_apps` | `ids: string[]` | `(AppInfo \| null)[]` — the installed app for each id (Launch Services), in order; `null` when not on this machine |
| `get_app_icon` | `id` | raw PNG bytes (`ArrayBuffer`, 64 px square); `not found` when not installed. Rendered once and cached in Rust until the settings window closes. Used by the settings window's app list and by the shelf's cards (the clip's `sourceApp.id`) |
| `choose_app` | — | `AppInfo \| null` — native open panel restricted to applications (run by Rust on the main thread; no dialog plugin, no file access for the web view); `null` when cancelled |
| | | The four app commands do their work off the main thread. Other platforms return empty lists / `null` for now; adding by identifier still works |

Errors are returned as a string message (never containing clip content).

Events emitted by Rust:

| Event | Payload | When |
|---|---|---|
| `clips-changed` | `{ reason: "inserted" \| "bumped" \| "deleted" \| "cleared" \| "updated", id?: number }` | history changed (`updated` with an id also when a link preview arrives) |
| `pinboards-changed` | — | pinboards or membership changed |
| `shelf-shown` | — | the shelf was just shown; UI resets search, selects the first clip and focuses the search field |
| `settings-changed` | `Settings` | settings saved (sent to every window: the shelf picks up changes made in the settings window) |
| `capture-skipped` | `{ kind: ClipKind, limitBytes: number }` | a copy was over a size cap and not stored (no content in the payload); the shelf shows a notice |

## Windows

- `shelf`: borderless, transparent, always-on-top, full-width panel docked to
  the very bottom of the screen under the cursor (`shelf::display_under`),
  over the Dock like the Paste app: its frame comes from the screen's full
  frame, not the work area (`dock_rect`), it spans the full width also over a
  Dock on the left or right, and its window level (status) is above the
  Dock's. Only the top of the work area is used, so on a short screen it never
  covers the menu bar. 364 pt tall (`shelf::HEIGHT`: the 328 pt
  panel plus its 24 pt top and 12 pt bottom margins; mirrored as
  `--shelf-height` in `src/app.css`, which a Rust test checks), hidden at startup, skip taskbar. Must not
  activate the app or steal focus from the frontmost app more than needed to
  take keyboard input; on hide, focus returns to the previous app so the paste
  lands there.
- Shelf motion (macOS): AppKit's window animator moves the whole window
  (`platform::macos::animate_window`), so the native glass, the web content
  and the CSS shadow move as one surface. The window stays opaque during the
  slide (Liquid Glass ignores window alpha). A slide-in starts with the
  window 1 pt from fully hidden: that point is transparent shadow margin, but
  it makes macOS treat the window as on screen so WebKit repaints before the
  panel appears. `shelf::Motion` tracks shown / sliding out with a
  generation counter, so pressing the shortcut mid-slide reverses from where
  the window is and a stale slide-out never hides a re-shown shelf.
- Shelf background (macOS): the web view is transparent and the UI paints the
  rounded panel, inset `--panel-inset` (12 pt, `shelf::PANEL_INSET`) from the
  window's left, right and bottom edges and `--panel-top` (24 pt,
  `shelf::PANEL_TOP`) from its top. Those transparent margins hold the
  panel's CSS shadow, which is shaped to fade out inside them (a 12 pt rim on
  every side, a lift reaching ~22 pt up) because the window edge would cut
  anything beyond in a hard line; the sides and bottom stay at 12 pt so the
  panel keeps its place against the screen edges and the Dock. A click in a
  margin dismisses the shelf, like any click outside it. The panel's height is set from
  `--shelf-height`, not from its content, so it always has the native
  view's frame. Behind it sits exactly one native material, framed to that
  panel with the same corner radius and kept there by autoresizing:
  - macOS 26+: Liquid Glass, an `NSGlassEffectView` that Satchel creates
    itself as a sibling below the web view (`install_liquid_glass`). Chosen
    when the class exists at run time; `SATCHEL_NO_GLASS=1` forces the
    fallback. Not window-vibrancy's glass, which moves the web view into the
    glass view.
  - macOS 11–15: Tauri's window effects (window-vibrancy's
    `NSVisualEffectView`, `Active` state), inset by `inset_effect_view`.
  Both are created once with the window. Rust tells the page which one it has
  through an initialization script (`window.__SATCHEL_SHELF_BACKGROUND__ =
  "glass" | "blur" | "none"`, read by `shelfBackground()` in
  `src/lib/platform.ts`); over glass the panel tint is nearly clear, otherwise
  (blur, Windows, Linux, plain-browser mock mode) it stays semi-opaque.
- `settings`: a regular titled window loading the `/settings` route
  (prerendered as `settings.html`). Resizable between 600×400 and 1040×1000
  points, title bar included (760×600 on first open, centred); no zoom button and no
  full screen (`disallow_full_screen`). The form's sections scroll between
  the title bar and a pinned footer (Clear History…, Revert, Save), with a
  divider and a bottom fade while more content is out of view (overlay
  scroll bars hide when idle). Its frame (AppKit screen points, so a
  display's scale factor does not matter) is saved 500 ms after the last
  move or resize and again on close, by a writer thread
  (`window_frame::FrameSaver`), as JSON in the store's window state: the
  `settings` table under the key `window.settings`
  (`Store::window_state` / `save_window_state`), which `Settings`,
  `save_settings` and `settings-changed` never touch. On the next open
  (also after a restart) `window_frame::restore` checks it against the
  current displays: the size is clamped to the limits and to the display's
  work area; a frame still partly on a display is moved fully inside the
  work area of the display it overlaps most; one on no display (moved off
  screen, or its display unplugged) is centred on the display under the
  pointer, else the main one. Windows/Linux: not saved yet (TODO), always
  centred.
  Created on demand and destroyed on close, unlike the shelf: it is rarely
  open and its web view's memory is given back. Single instance: opening it
  again shows and focuses the existing one. Opened from the tray, the
  shelf's More menu and Cmd/Ctrl+, in the shelf. Satchel stays an accessory
  app, so opening it activates Satchel (needed for a normal window to take
  keyboard input); on close, focus goes back to the app that was in front
  when it opened, unless the user has already switched elsewhere. Closing it
  never quits Satchel.
- Capabilities: `shelf` may use the clip, pinboard and paste commands plus
  `get_settings`, `set_capture_paused`, `hide_shelf`, `open_settings`,
  `open_accessibility_settings`, `get_app_icon` (icons only: it cannot
  list or choose apps) and `get_link_image` (cached bytes only);
  `settings` may use only `get_settings`, `save_settings`,
  `set_capture_paused`, `clear_history`, `close_settings` and the app
  picker's `list_apps`, `resolve_apps`, `get_app_icon` and `choose_app`.
  Both may listen to events. Neither gets plugin or core window
  permissions.
- Excluded apps in Settings: the stored list is unchanged (bundle ids /
  executables, matched case-insensitively by `should_capture`). The window
  lists each excluded app that is installed here by icon and name
  (same-named copies merged into one row whose Remove removes all its ids),
  and entries the user added for apps that are not installed by identifier.
  Built-in defaults that are not installed (including every other-platform
  default) are not listed but stay stored and enforced, so an app installed
  later is already excluded; saving writes them back untouched. The window
  tells defaults apart with a copy of `DEFAULT_EXCLUDED_APP_IDS`
  (`BUILT_IN_DEFAULTS` in `src/lib/excluded-apps.ts`, kept equal by a Rust
  test). "Add App…" opens a searchable picker over `list_apps`, plus "Choose
  Other App…" (`choose_app`) and "Add by Identifier…".
- Tray / menu bar icon with: Show Satchel, Settings…, Pause Capturing, Clear
  History, Quit. No dock icon on macOS (accessory activation policy).
- Menu bar icon animation (`tray_anim.rs`, `platform/macos_tray.rs`): when
  capture stores a clip, new or a re-copy bumped to the top
  (`EventSink::clip_saved`; not for refused, paused, skipped, too-large or
  own writes, or a failed store) the satchel's flap
  lifts, a card fades in inside the handle and falls into the bag, and the
  flap drops shut with one small rebound (0.6 s). The glyph is drawn from
  code as a function of the pose (18 pt grid, so edges sit on whole pixels),
  rendered per frame into a template `NSImage` with 1x and 2x bitmaps and set
  on the tray's own `NSStatusBarButton` (reached through
  `with_inner_tray_icon` / `ns_status_item`) from an `NSTimer` in the common
  run loop modes that exists only while the animation runs. Clips during a
  run join it, rewind it seamlessly once the card is in, or queue at most
  one replay. Reduce Motion: no animation. Other platforms: no-op. The
  capture thread posts at most one hop to the main thread at a time.
  `SATCHEL_TRAY_SHEET=<png> cargo test -p satchel tray_sheet -- --ignored`
  writes every frame to a contact sheet for review.

## Shelf behavior (matches the prototype in `docs/design/`)

- Opens with the global shortcut (default `CommandOrControl+Shift+V`).
- Search field focused on open; typing filters; scope buttons filter by kind;
  tabs switch between History and each pinboard.
- Left/Right arrows move the selection; Return pastes; Shift+Return pastes as
  plain text; Cmd/Ctrl+1…9 pastes the nth visible clip; Cmd/Ctrl+C copies
  without pasting; Cmd/Ctrl+P pins; Cmd/Ctrl+Backspace deletes; Space toggles
  a larger preview; Shift/Cmd-click multi-selects; Escape closes.
- Cards show the source app (its real icon and name), kind, relative time,
  and a preview by kind (text, link, code in monospace, color swatch, image
  thumbnail with its pixel size and file size, file names). A link with a
  cached preview shows the page's image (or the site icon over the site
  name, or a link glyph over it) in the same 92 pt area an image thumbnail
  uses, then its title (the user's own title wins) and "site · host"; the
  full address is the card's tooltip and is in the larger preview. Until
  the image has loaded the area shows the site, at the same size, so
  nothing moves. Links without a preview look as they always did. The larger
  preview (Space) shows the image thumbnail with its dimensions. The icon comes from `get_app_icon` by the clip's
  `sourceApp.id` through one `AppIconCache` for the page (one `blob:` URL
  per app, shared by every card and the preview, kept while the shelf page
  lives), loaded only for cards on screen and never awaited before the
  shelf shows. A cached icon is in a card's first frame (no flicker on
  re-render or reopen); a loading one is a blank tile of the same size (no
  layout shift); with no source app or no icon (uninstalled, other
  platforms) the card keeps the coloured letter tile.
- Layout: one top bar over the card row, no footer. Left to right: search,
  pinboard tabs, kind filters, the notice, the item count, "Paste as Plain
  Text" and "Paste" (primary), and the More menu. Tab order follows it, then
  the selected card. The buttons say "Copy" when paste-on-select is off and
  "Paste N Clips" for a pasted multi-selection.
- The notice (role="alert", in place of the item count) appears only when
  something needs attention: an action failed, or a copy was too large to
  keep (both cleared by the next selection change); the missing
  Accessibility permission, with its "Open Accessibility Settings…" button;
  a failed list load. Confirmations ("Copied to the clipboard", "Pasted",
  "Deleted 2 clips", "… pinned to / removed from …") are never shown, since
  the action's visible result is the confirmation; they go to a visually
  hidden role="status" region for screen readers. The selection itself is
  not narrated: the cards and the buttons already say it.
- Narrow screens: the bar never wraps. Search, tabs and filters shrink (tabs
  and filters scroll) to their minimums; the count hides at ≤1180 px of bar,
  "… as Plain Text" becomes "Plain Text" at ≤1000 px, and that button leaves
  the bar at ≤860 px (still in the clip menu and on Shift+Return).

## Link previews

On by default (`Settings.link_previews`, stored as `link_previews`;
missing means on, so existing settings upgrade with it on: the user chose
previews by default, which is why the skip rules below are strict). With it
off Satchel makes no network request at all: nothing is queued, the workers
never claim a job, and the HTTP client is never called (tested in
`link_preview::tests::nothing_is_queued_or_fetched_while_the_setting_is_off`).

Implementation: a small Rust HTTPS client (`ureq` + rustls) in
`src-tauri/src/link_preview/`, not Apple's LinkPresentation.
`LPMetadataProvider` loads the page in WebKit with JavaScript and
subresources, uses the app's shared URL loading (cookies and caches), and
gives no control over redirects, resolved addresses, size or the user
agent; none of the rules below could be enforced with it.

When a link is fetched: a Link clip copied (or copied again) while the
setting is on is queued at once. Link clips already in the history (an
upgrade, or the setting turned back on) are backfilled: one is queued
every 15 s (`BACKFILL_EVERY_MS`), newest first, the first 15 s after
launch or after the setting is turned on, so there is never a burst of
requests. A queued clip is fetched once, in the background, by
one of `WORKERS` (2) threads that sleep on a condition variable until
there is work, never on the capture or shelf-show paths. Before fetching
the worker re-checks the setting, the excluded-apps list (the clip's
source app may have been excluded since) and `link::fetch_target`:

- Only `http`/`https`; `http` is upgraded to `https` (never sent in clear
  text). Over 2048 bytes: skipped.
- Skipped: user info in the URL; an IP address host; a non-default port;
  a single-label host or a local/reserved suffix (`localhost`, `.local`,
  `.lan`, `.internal`, `.home.arpa`, `.test`, `.onion`, ...); sign-in,
  account, mail and payment hosts (`login.`, `auth.`, `sso.`, `accounts.`,
  `id.`, `mail.`, `checkout.`, ...).
- Skipped as one-time / credential links: query parameters named like
  tokens, keys, signatures (incl. S3/GCS/Azure SAS), sessions, passwords,
  OTPs, nonces, codes, OAuth/redirect parameters (`redirect_uri`, `next`,
  `continue`, `state`, ...), e-mail addresses or phone numbers; a parameter
  value that is itself a URL or contains `@`; path segments such as
  `reset`, `password`, `login`, `verify`, `confirm`, `unsubscribe`,
  `invite`, `magic`, `oauth`, `callback`, `token`, `redeem`, `checkout`,
  `invoice`, `account`, `settings`; any segment, value or fragment that
  looks like a random identifier (a UUID, 16+ letters and digits mixed, or
  24+ mixed-case letters); a fragment with such parameters. When unsure,
  the link is not fetched. Tracking parameters (`utm_*`, `fbclid`,
  `gclid`, `si`, ...) and the fragment are removed before fetching.
- Network rules (`link_preview/http.rs`): every resolved address must be
  globally routable (`link::is_public_ip`: no loopback, private,
  link-local, CGNAT, multicast, documentation or reserved ranges, IPv6
  forms judged by their embedded IPv4) and is checked in the resolver, so
  the connection goes to the checked address; every redirect (at most 5)
  passes `link::redirect_target` (all of the above again, and never to
  `http`); 10 s per page including redirects, 10 s per image, 5 s to
  connect; pages must be `text/html`/`application/xhtml+xml` and only the
  first 512 KiB is read; images must be `image/*` (not SVG), at most
  5 MiB, not redirected, and PNG, JPEG, GIF, WebP or ICO by their first
  bytes, decoded by ImageIO with a 40 MP cap. No cookies (no cookie store
  is compiled in), no credentials, no `Referer`, no `Accept-Language`,
  environment proxies ignored, and the generic `Twitterbot/1.0` crawler
  user agent (sites send it a small page with the card metadata early;
  it identifies neither Satchel nor the Mac).
- Per page: one HTML request, then at most one successful image request
  (`og:image`, else the best `<link rel=icon>`, else `/favicon.ico`).
- Parsing (`link::parse_html`) reads `<head>` only: `og:title` /
  `twitter:title` / `<title>`, `og:site_name`, `og:image` /
  `twitter:image`, icons; ignores script, style, noscript and comments;
  never runs anything. Charset from the header, else `<meta charset>`,
  else UTF-8 (Latin-1/Windows-1252 understood; other encodings' non-ASCII
  text is dropped). Control and bidi-override characters are removed;
  titles are capped at 200 characters, site names at 80.
- Failures: offline, DNS, timeouts, 408/425/429/5xx are retried after
  1 and 4 minutes, then given up (3 attempts); anything else is final.

Stored (`link_previews`, `link_preview_images`, both cascading from
`clips`): status, title, site, image kind, attempts, next attempt, fetched
at, and one PNG (preview image ≤ 480 px, or icon ≤ 128 px; ≤ 512 KiB).
No URL is stored beyond the clip itself. The title is indexed for search
with the clip's own title. Previews are deleted with their clip (delete,
Clear History, retention pruning), and all of them, with the queue, when
the setting is turned off (`clear_link_previews`, secure delete + WAL
checkpoint). The shelf gets the preview in `ClipSummary.linkPreview` and
the image through `get_link_image`, as bytes turned into `blob:` URLs
like thumbnails; the CSP is unchanged.

`SATCHEL_TRACE=1` logs each preview's outcome and skip reason (never the
link, title or host).

## Testing

Test-driven: write the failing test first, then the code. Core logic lives in
pure functions and in `Store` against an in-memory database so tests need no
OS or UI. Platform code keeps OS calls behind small traits so the logic around
them is tested with fakes. Front-end logic (state, filtering, keyboard
handling, formatting) lives in plain TypeScript modules tested with Vitest,
with the IPC layer mocked.

Commands: `cargo test -p satchel-core`, `cargo test -p satchel`, `pnpm test`.
