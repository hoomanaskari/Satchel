# Building and releasing Satchel

There are two Satchel apps, built from the same code:

- **Satchel**, the one people install. Only `pnpm release` makes it.
- **Satchel Dev**, the one you build while working on Satchel: `pnpm app` and `pnpm tauri dev`.

They are separate apps, so you can run Satchel Dev next to your installed Satchel and try a change
without touching the clips you use.

| | Satchel | Satchel Dev |
|---|---|---|
| Made by | `pnpm release` | `pnpm app`, `pnpm tauri dev` |
| Config | `src-tauri/tauri.conf.json` as it is | the same, with `src-tauri/tauri.dev.conf.json` laid over it |
| Bundle identifier | `app.satchel.desktop` | `app.satchel.desktop.dev` |
| Name (Finder, menu bar, System Settings) | Satchel | Satchel Dev |
| History and settings | `~/Library/Application Support/app.satchel.desktop/` | `~/Library/Application Support/app.satchel.desktop.dev/` |
| Accessibility permission | its own entry, "Satchel" | its own entry, "Satchel Dev" |
| Shelf shortcut on a new history | ⇧⌘V | ⌥⇧⌘V |
| App icon | `src-tauri/icons/Satchel.icon` | the same with a blue DEV band (`src-tauri/icons/dev/`) |
| Menu bar icon | follows the menu bar (black or white) | blue |
| Signing | Developer ID, hardened runtime, notarized and stapled | the same Developer ID certificate, not notarized |
| Packaging | DMG and its SHA-256 checksum | `Satchel Dev.app` only |

## What you need

- macOS with Xcode (for `actool` and Icon Composer, which compile the app icon) and its command
  line tools.
- Rust (stable), Node 24 and pnpm (`pnpm install` once).
- For signed builds: a **Developer ID Application** certificate in your login Keychain. Check with
  `security find-identity -v -p codesigning`. Without one, `pnpm app` signs ad-hoc (see below).
- For releases only: `brew install create-dmg`, the notary login (see "Once, before the first
  release") and cargo-about (see "Third-party notices").

## Satchel Dev

```bash
pnpm tauri dev
```

Runs Satchel Dev with the front end served by Vite, reloading as you edit. `pnpm tauri dev` always
adds `tauri.dev.conf.json` (`scripts/tauri.ts`); every other `pnpm tauri` command goes through
unchanged.

```bash
pnpm app
```

Builds `target/release/bundle/macos/Satchel Dev.app`, an optimized, signed Satchel Dev you can
copy anywhere and keep running. It is signed with `APPLE_SIGNING_IDENTITY` when that is set,
otherwise with the Developer ID Application certificate on this Mac that lasts longest
(`scripts/signing-identity.sh`), otherwise ad-hoc. macOS ties the Accessibility permission (needed
to paste into other apps) to the identifier and the signature: with the same certificate every
build keeps the permission; an ad-hoc build looks like a new app each time, so you grant it again
after every build.

Debug builds (`pnpm tauri dev`) also honor `SATCHEL_DATA_DIR=/some/folder` for a throwaway
history.

### Running both at once

Satchel Dev and an installed Satchel can run at the same time:

- **History and settings** are in different folders (the folder name is the bundle identifier).
- **Copies are not shared.** Each app records its own clipboard history from what you copy. When
  you paste from one, the other does not record that paste as a new clip: both mark what they
  write with the same `app.satchel.desktop.self` pasteboard type, which both skip.
- **Shortcut.** A new Satchel Dev history starts with ⌥⇧⌘V, so ⇧⌘V keeps opening your installed
  Satchel and ⌥⇧⌘V opens Satchel Dev. You can change either in its Settings; keep them different.
  macOS lets two apps register the same shortcut, so if both use one, that key opens both shelves.
  A Satchel Dev history made before this rule, or copied from Satchel's folder, keeps the shortcut
  saved in it. When the saved shortcut cannot be registered at all, each app falls back to its own
  default (⇧⌘V for Satchel, ⌥⇧⌘V for Satchel Dev).
- **One copy of each.** macOS runs one copy per bundle identifier when apps are opened from Finder
  or with `open`; Satchel Dev has its own identifier, so it never takes the place of Satchel.
- **Menu bar.** Satchel Dev's menu bar icon is blue and its menu says "Show Satchel Dev" and "Quit
  Satchel Dev".
- **Accessibility.** System Settings lists "Satchel Dev" separately; turn it on for each app you
  paste from.

## The app icon

Satchel's icon is an Icon Composer document, `src-tauri/icons/Satchel.icon`. Tauri compiles it into
`Contents/Resources/Assets.car` and names it in `Info.plist` (`CFBundleIconName`), which is what
macOS 26 and later show; a plain `.icns` would be shown shrunk inside a system tile. `icon.icns`
and the PNGs are still bundled for older macOS.

Satchel Dev's icon lives in `src-tauri/icons/dev/`. Do not edit it by hand: after changing
`Satchel.icon`, redraw it and commit the result.

```bash
sh scripts/dev-icon.sh
```

It copies `Satchel.icon`, draws a blue DEV band on its picture (`scripts/dev-icon.swift`) and
exports `icon.icns` and the PNGs from it with Icon Composer's `ictool`.

## Versions

Satchel's version is kept in `package.json`, `src-tauri/tauri.conf.json`,
`src-tauri/Cargo.toml` and `crates/satchel-core/Cargo.toml`. Change all of them, and `Cargo.lock`,
with one command:

```bash
pnpm set-version 0.2.0
```

## Third-party notices

Satchel.app carries `THIRD-PARTY-NOTICES.txt`, the licences and copyright notices of the
third-party code inside it. It is checked in at the root of the repository, bundled into
`Contents/Resources/` of both Satchel and Satchel Dev (`bundle.resources` in `tauri.conf.json`),
and shown in Settings under "Acknowledgements…". Regenerate it whenever `Cargo.lock` or
`pnpm-lock.yaml` changes, and commit the result:

```bash
pnpm notices
```

It needs cargo-about, at the version named in `scripts/notices/config.json`:

```bash
cargo install cargo-about --locked --features cli --version 0.9.3
```

What it covers:

- **Rust crates** compiled into the app binary for the Mac targets in `scripts/notices/about.toml`:
  `cargo tree -e normal,no-proc-macro`, so build scripts, proc-macros, the crates only they use and
  dev dependencies are left out. cargo-about finds each crate's licence files and copyright notices
  in its sources (offline, from Cargo's download cache). A crate that ships no licence file gets the
  standard text with the authors it lists.
- **npm packages** whose code is in the built web assets, as the bundler reports them
  (`scripts/lib/bundle-report.js`, a Vite plugin that does nothing unless `pnpm notices` asks), for
  both the Satchel and Satchel Dev builds. Build tools that leave no code in the bundle are not
  listed. Bundler-generated helpers and code copied into another package are named in
  `scripts/notices/config.json`; the script stops when the bundle has one it doesn't know.

`accepted` in `scripts/notices/about.toml` is the licence allow-list for both: a package whose
licence it cannot satisfy stops generation. MPL-2.0 packages are listed with a link to their source.

```bash
pnpm notices --check
```

fails when the file is not what `pnpm notices` would write now. `pnpm release` runs it before
building and checks that the built app contains the same file.

## Releases

### Once, before the first release

1. **The Developer ID certificate** for the team that signs Satchel must be in the Keychain (see
   above).
2. **A notary login**, saved in the Keychain. Apple checks every release with it.
   1. At [account.apple.com](https://account.apple.com), open Sign-In and Security, then
      App-Specific Passwords, and make one called "Satchel notarize".
   2. Save it (Terminal asks for the password; paste the app-specific one):

      ```bash
      xcrun notarytool store-credentials satchel-notary --apple-id YOUR_APPLE_ID --team-id YOUR_TEAM_ID
      ```

   The release script uses the profile named `satchel-notary`; set `SATCHEL_NOTARY_PROFILE` to use
   another one you already have. Nothing about the login is stored in this repository.

### Each release

1. Set the version (`pnpm set-version 0.2.0`), write what's new in
   `docs/release-notes/0.2.0.md`, commit and push to `main`.
2. Run:

   ```bash
   pnpm release
   ```

   It stops unless the tree is clean, on `main`, the same as `origin/main`, the version has no tag
   yet, the config is the installed Satchel's (not Satchel Dev's), a Developer ID certificate and
   the notary login are present. Then it:

   - runs `pnpm notices --check`, `pnpm check`, `pnpm test`, `cargo test` and `cargo clippy`,
   - builds `Satchel.app` from `tauri.conf.json` in its own Cargo folder, `target/production`, so
     it never replaces a Satchel Dev build or a Satchel that is running from `target/release`,
   - checks the built app: identifier `app.satchel.desktop`, name Satchel, the version,
     `THIRD-PARTY-NOTICES.txt` inside it, a Developer ID signature with the hardened runtime and a
     secure timestamp,
   - sends it to Apple to notarize, staples the ticket and asks Gatekeeper to assess it,
   - makes the DMG, signs it, notarizes and staples it,
   - writes `SHA256SUMS.txt`.

   The result is in `target/satchel-release/<version>/`:

   ```
   Satchel_<version>_aarch64.dmg
   SHA256SUMS.txt
   ```

3. Tag the release (`git tag v0.2.0 && git push origin v0.2.0`), create a GitHub release for the
   tag with `docs/release-notes/0.2.0.md` as its text, and attach the DMG and `SHA256SUMS.txt`.

To rehearse without notarizing (no notary login needed, and the tree may have changes):

```bash
pnpm release --local
```

That does everything except notarization and stapling, into `target/satchel-release/local/<version>/`.
Its DMG is signed but Gatekeeper refuses it on other Macs.

### Checking a release by hand

```bash
codesign --verify --deep --strict --verbose=2 target/production/release/bundle/macos/Satchel.app
spctl --assess --type execute --verbose target/production/release/bundle/macos/Satchel.app
xcrun stapler validate target/satchel-release/0.2.0/Satchel_0.2.0_aarch64.dmg
spctl --assess --type open --context context:primary-signature --verbose target/satchel-release/0.2.0/Satchel_0.2.0_aarch64.dmg
shasum -a 256 -c target/satchel-release/0.2.0/SHA256SUMS.txt
```

### Not included

- **Updates.** Satchel has no updater: it makes no network request except link previews. A new
  version is installed by downloading the new DMG.
- **Intel Macs.** The release is built for the Mac that builds it (Apple silicon: `aarch64`).
- **Windows and Linux** packaging.
