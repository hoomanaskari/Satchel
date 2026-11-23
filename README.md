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
