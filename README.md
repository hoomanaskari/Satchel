# Satchel

Open source, cross-platform clipboard manager built with Tauri 2.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design, contracts and priorities.

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
