# Satchel

Open source, cross-platform clipboard manager built with Tauri 2.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design, contracts and priorities.

## What it keeps

Text (with its HTML/RTF formatting, so "Paste" and "Paste as Plain Text" differ), links,
code, colors (hex/rgb/hsl text, and colors copied from color wells), images (screenshots,
Preview, Photos, browsers' "Copy Image"; PNG, TIFF, JPEG, HEIC, WebP, GIF, BMP, stored as
PNG) and files (one or many files or folders from Finder; an image file copied in Finder is a
file). Limits: 2 MiB of text, 64 MB / 100 megapixels per image; a copy over a limit is not
kept and the shelf says so. Copies marked concealed or transient (password managers) and
copies from excluded apps are never kept. See "Clip kinds" in the architecture doc.

## Privacy and the network

Satchel keeps your history in a local database readable only by you: no accounts, no sync, no
telemetry, no update checks. The one use of the network is **link previews, which are on by
default**: Satchel fetches each web link you copy, once, in the background, from that link's own
site, to show the page's title and image on the card. That site sees the link and your IP
address. Links already in your history are fetched the same way, slowly (one every 15 seconds,
newest first), not all at once.

Satchel never fetches: links copied in excluded apps (password managers by default) or marked
concealed/transient (those are never kept at all); links that look like sign-in, one-time,
signed or account links (tokens, keys, signatures, password resets, magic links, invites,
unsubscribe, OAuth/redirect links, random-looking IDs); links with a user name or password;
IP addresses, local names and non-standard ports; and nothing that resolves or redirects to a
private, loopback or link-local address. It fetches over HTTPS only, sends no cookies, runs no
page scripts, strips tracking parameters and caps time and size.

To turn it off: Settings > Shelf > uncheck **Show link previews** and Save. Satchel then makes no
network requests at all and deletes the saved previews. Details: "Link previews" in the
architecture doc.

## Develop

```bash
pnpm install
pnpm tauri dev
```

## Build the app

```bash
pnpm app
```

Builds `target/release/bundle/macos/Satchel.app`, signed with your Developer ID Application
certificate (or `APPLE_SIGNING_IDENTITY`), so the Accessibility permission Satchel needs for
pasting lasts from one build to the next. Without a certificate it signs ad hoc, and macOS asks
for the permission again after every build.

## Test

```bash
cargo test --workspace
pnpm test
```
