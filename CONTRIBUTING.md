# Contributing to Satchel

Thanks for wanting to help. Satchel is small and early, so the most useful things right now are bug reports and fixes with tests.

## Before you start

- **Bugs and ideas:** open an [issue](https://github.com/hoomanaskari/Satchel/issues) first, so we can agree on the fix before you spend time on it.
- **Security problems:** please don't open an issue. See [SECURITY.md](SECURITY.md).
- **Platforms:** Satchel is built and tested on macOS. The Windows and Linux code paths are stubs for now; help there is welcome, but talk to us in an issue first.

## Setting up

How to install the tools, run Satchel in development and build the app is in [docs/BUILDING.md](docs/BUILDING.md).

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) before changing anything big. It has the rules Satchel keeps, in this order: security and privacy, then speed, then features.

## How we work

1. **Test first.** Write the test, see it fail for the right reason, then write the code that makes it pass.
2. **Run the checks before you open a pull request:**

   ```bash
   cargo test --workspace
   ```

   ```bash
   pnpm check
   ```

   ```bash
   pnpm test
   ```

3. **Keep the privacy rules.** No new network access, no logging of clip content, no new Tauri plugins or permissions without a good reason. Clip content is untrusted: render it as text only.
4. **Keep it small.** Explain any new crate or npm package in the pull request.

## Pull requests

- One change per pull request, with a title that says what it does for people.
- Say how you tested it, and on which macOS version.
- Screenshots for anything you can see.

By contributing, you agree that your work is shared under the [MIT licence](LICENSE).
