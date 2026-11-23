// The settings window loads this route directly (`WebviewUrl::App("settings")`),
// so it is emitted as its own `settings.html` shell instead of relying on the
// SPA fallback. With `ssr = false` (root layout) the shell holds no content;
// the page renders on the client like the shelf.
export const prerender = true;
