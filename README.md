# Satchel

**A fast, private clipboard manager for macOS.** Free and open source (MIT).

Satchel keeps a searchable history of what you copy. Press a shortcut, and a shelf slides up from the bottom of the screen: pick a clip and it pastes into the app you were using. Your history stays on your Mac.

<!-- SCREENSHOT PLACEHOLDER: add a screenshot of the shelf here before release, e.g.
     ![The Satchel shelf](docs/images/shelf.png) -->

> **Status: early.** Satchel 0.1 is the first public version, for **macOS 11 (Big Sur) or later**.
> The code is laid out for Windows and Linux too, but those versions aren't built or tested yet.

## Download

1. Get the newest `Satchel_<version>_*.dmg` from [Releases](https://github.com/hoomanaskari/Satchel/releases/latest).
2. Open it and drag Satchel to Applications.
3. Open Satchel. It lives in the menu bar (a small satchel icon), not in the Dock.

## First run

1. **Open the shelf** with `⌘⇧V`. You can change the shortcut in Settings.
2. **Allow pasting.** To paste into other apps, Satchel needs the Accessibility permission. The first time you paste, Satchel copies the clip instead and shows **Open Accessibility Settings…**. Turn Satchel on in System Settings > Privacy & Security > Accessibility, then paste again.
3. **Check what's excluded.** Settings > Never capture from lists the apps Satchel ignores. Common password managers are on it already.

## What it does

- **Keeps what you copy:** text (with its formatting), links, code, colors, images (screenshots, Preview, Photos, browsers' Copy Image) and files from Finder.
- **A shelf over the Dock:** cards show the source app, the kind of clip, when you copied it and a preview. On macOS 26 the shelf uses Liquid Glass.
- **Search** as you type, and filter by kind: Text, Links, Code, Images, Colors, Files.
- **Paste from the keyboard:** arrow keys to choose, `Return` to paste, `⇧Return` to paste as plain text, `⌘1` to `⌘9` to paste one of the first nine clips, `⌘C` to copy without pasting, `Space` for a larger preview, `⌘⌫` to delete.
- **Paste several clips at once:** `⇧`- or `⌘`-click to select, then paste them in order.
- **Pinboards:** pin clips (`⌘P`) to named, colored pinboards. Pinned clips are never removed.
- **Titles:** rename a clip so it's easy to find again.
- **Link previews:** the page's title and image on link cards (see [Privacy](#your-privacy)).
- **Pause capturing** from the menu bar icon or Settings. The menu bar icon shows a short animation each time a clip is saved (none with Reduce Motion on).
- **History limits:** keep up to 2,000 clips and 90 days by default. Both are in Settings.

## Your privacy

Clipboard history holds passwords and other secrets, so Satchel is built to keep less, not more.

- **Your history stays on your Mac.** It's a local database in Satchel's own folder, readable only by your user account. It isn't encrypted beyond what macOS gives your disk (for example FileVault). There are no accounts, no sync, no analytics and no update checks.
- **Some copies are never kept:** anything copied in an excluded app, and anything an app marks as concealed or temporary (password managers do this). You can add or remove excluded apps in Settings > Never capture from.
- **Deleting means deleting.** Deleted clips are overwritten in the database, not just hidden.
- **Link previews are the only thing that uses the network, and they're on by default.** When you copy a web link, Satchel fetches that page once, in the background, from the link's own site, to show its title and image. That site sees the link and your IP address. Links already in your history are fetched slowly, one every 15 seconds.
- **Some links are never fetched:** links from excluded apps; sign-in, password-reset, one-time and other links that carry tokens or random-looking IDs; links with a user name or password; IP addresses, local names and unusual ports; and anything that leads to a private network address. Satchel fetches over HTTPS only, sends no cookies, runs no page scripts and removes tracking parameters.
- **To turn link previews off:** Settings > Shelf > uncheck **Show link previews**, then Save. Satchel then makes no network requests at all and deletes the saved previews.

The details are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), under "Security rules" and "Link previews".

## Building from source

See [docs/BUILDING.md](docs/BUILDING.md). Tests run with:

```bash
cargo test --workspace
```

```bash
pnpm test
```

## Contributing

Bug reports and fixes with tests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). For security problems, see [SECURITY.md](SECURITY.md).

## Licence

[MIT](LICENSE). Satchel uses open source libraries under their own licences (mostly MIT and Apache-2.0).
