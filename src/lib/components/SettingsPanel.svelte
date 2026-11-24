<script lang="ts">
  // Minimal settings form for every `Settings` field. Hosted by the settings
  // window (`/settings`), which owns closing (Escape, Cmd/Ctrl+W).
  import {
    acceleratorFromEvent,
    formatAccelerator,
    MAX_ITEMS_RANGE,
    normalizeExcluded,
    retentionOptions,
    validateSettings,
  } from "$lib/settings-form";
  import { addExcluded, excludedView, removeExcluded, type AppDirectory, type ExcludedRow } from "$lib/excluded-apps";
  import { APP_NAME } from "$lib/app-name";
  import { isMac } from "$lib/platform";
  import type { AppInfo, Settings, UpdateStatus } from "$lib/types";
  import { percent, updateLine } from "$lib/updates";
  import AppIcon from "./AppIcon.svelte";
  import AppPicker from "./AppPicker.svelte";
  import Notices from "./Notices.svelte";

  interface Props {
    settings: Settings | null;
    onsave: (s: Settings) => Promise<void>;
    onpause: (paused: boolean) => Promise<void>;
    onclear: () => Promise<void>;
    /** Names, icons and the picker for the excluded-apps list. */
    apps: AppDirectory;
    /** The bundled third-party licence notices, for "Acknowledgements…". */
    notices: () => Promise<string>;
    /** Where updates stand, and the two things the user can do about them. */
    updates: { status: UpdateStatus | null; check: () => Promise<void>; install: () => Promise<void> };
  }
  let { settings, onsave, onpause, onclear, apps, notices, updates }: Props = $props();

  // Updates: Rust checks, downloads, verifies and installs; this shows where
  // it stands. `now` is taken when the status changes (no timer).
  const update = $derived(updates.status);
  const updateNow = $derived(update ? Date.now() : 0);
  const updateBusy = $derived(
    !!update && (update.checking || update.phase.state === "downloading" || update.phase.state === "installing"),
  );
  let updateError = $state("");
  async function runUpdate(action: () => Promise<void>) {
    updateError = "";
    try {
      await action();
    } catch (err) {
      updateError = err instanceof Error ? err.message : String(err);
    }
  }

  // A local draft; re-seeded when the saved settings change underneath.
  let draft = $state<Settings | null>(null);
  let seededFrom: Settings | null = null;
  $effect.pre(() => {
    if (settings && settings !== seededFrom) {
      seededFrom = settings;
      draft = { ...settings, excludedAppIds: [...settings.excludedAppIds] };
    }
  });

  const errors = $derived(draft ? validateSettings(draft) : {});
  const valid = $derived(Object.keys(errors).length === 0);
  const dirty = $derived(
    !!draft &&
      !!settings &&
      (draft.maxItems !== settings.maxItems ||
        draft.retentionDays !== settings.retentionDays ||
        draft.captureImages !== settings.captureImages ||
        draft.pasteOnSelect !== settings.pasteOnSelect ||
        draft.linkPreviews !== settings.linkPreviews ||
        draft.checkForUpdates !== settings.checkForUpdates ||
        draft.shelfShortcut !== settings.shelfShortcut ||
        draft.excludedAppIds.join("\n") !== settings.excludedAppIds.join("\n")),
  );

  let form = $state<HTMLFormElement | null>(null);
  let focused = false;
  $effect(() => {
    if (draft && form && !focused) {
      focused = true;
      form.querySelector<HTMLElement>("input, select, button")?.focus();
    }
  });

  let message = $state("");
  let saving = $state(false);
  let recording = $state(false);
  let confirmClear = $state(false);
  let pickerOpen = $state(false);
  let addButton = $state<HTMLButtonElement | null>(null);
  let noticesOpen = $state(false);
  let noticesButton = $state<HTMLButtonElement | null>(null);

  function closeNotices() {
    noticesOpen = false;
    queueMicrotask(() => noticesButton?.focus());
  }

  // The body scrolls between the title bar and the pinned footer. Overlay
  // scroll bars hide when idle, so a divider (and a fade at the bottom) shows
  // whenever content runs past either edge.
  let body = $state<HTMLDivElement | null>(null);
  let moreAbove = $state(false);
  let moreBelow = $state(false);
  function measureScroll() {
    if (!body) return;
    moreAbove = body.scrollTop > 0;
    moreBelow = body.scrollTop + body.clientHeight < body.scrollHeight - 1;
  }
  $effect(() => {
    if (!body) return;
    const observer = new ResizeObserver(measureScroll);
    observer.observe(body);
    for (const section of body.children) observer.observe(section);
    measureScroll();
    return () => observer.disconnect();
  });

  // Installed app (or null) for each excluded id, keyed by lowercased id.
  // Ids not resolved yet are fetched in one call whenever the list changes.
  let resolved = $state.raw(new Map<string, AppInfo | null>());
  let resolving = $state(true);
  $effect(() => {
    const ids = draft?.excludedAppIds ?? [];
    const missing = ids.filter((id) => !resolved.has(id.toLowerCase()));
    if (missing.length === 0) {
      if (draft) resolving = false;
      return;
    }
    let live = true;
    apps
      .resolveApps(missing)
      .catch(() => missing.map(() => null))
      .then((found) => {
        if (!live) return;
        const next = new Map(resolved);
        missing.forEach((id, i) => next.set(id.toLowerCase(), found[i] ?? null));
        resolved = next;
      });
    return () => (live = false);
  });
  // Built-in defaults that are not installed stay stored (and enforced) but
  // are not listed; saving writes the whole draft list back, so they survive.
  const rows = $derived(draft ? excludedView(draft.excludedAppIds, resolved) : []);

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (!draft || !valid || !settings) return;
    saving = true;
    message = "";
    try {
      // capturePaused is applied immediately by its own control; keep the saved value.
      await onsave({ ...draft, capturePaused: settings.capturePaused, excludedAppIds: normalizeExcluded(draft.excludedAppIds) });
      message = "Saved";
    } catch (err) {
      message = err instanceof Error ? err.message : String(err);
    } finally {
      saving = false;
    }
  }

  function revert() {
    if (settings) draft = { ...settings, excludedAppIds: [...settings.excludedAppIds] };
    message = "";
  }

  function recordKey(e: KeyboardEvent) {
    if (!recording || !draft) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      recording = false;
      return;
    }
    const acc = acceleratorFromEvent(e, isMac);
    if (acc) {
      draft.shelfShortcut = acc;
      recording = false;
    }
  }

  function addApp(id: string, app: AppInfo | null) {
    if (!draft) return;
    if (app) resolved = new Map(resolved).set(id.toLowerCase(), app);
    draft.excludedAppIds = addExcluded(draft.excludedAppIds, id);
    closePicker();
    message = `${app?.name ?? id} will be excluded when you save.`;
  }

  function closePicker() {
    pickerOpen = false;
    queueMicrotask(() => addButton?.focus());
  }

  function removeRow(row: ExcludedRow) {
    if (!draft) return;
    draft.excludedAppIds = removeExcluded(draft.excludedAppIds, row.ids);
    message = "";
  }

  const rowLabel = (row: ExcludedRow) => row.name ?? row.ids[0];
  const removeLabel = (row: ExcludedRow) =>
    `Stop excluding ${rowLabel(row)}` + (row.ids.length > 1 ? ` (${row.ids.length} identifiers)` : row.name ? ` (${row.ids[0]})` : "");

  function retentionValue(e: Event) {
    if (!draft) return;
    const v = (e.currentTarget as HTMLSelectElement).value;
    draft.retentionDays = v === "forever" ? null : Number(v);
  }

  async function clear() {
    confirmClear = false;
    try {
      await onclear();
      message = "History cleared";
    } catch (err) {
      message = err instanceof Error ? err.message : String(err);
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form
  bind:this={form}
  inert={pickerOpen || noticesOpen}
  class="settings"
  aria-label="Settings"
  onsubmit={save}
  onkeydown={(e) => {
    // Escape first backs out of the inline Clear History confirmation.
    if (e.key === "Escape" && confirmClear) {
      e.preventDefault();
      confirmClear = false;
    }
  }}
>
  {#if !draft}
    <p class="muted loading">Loading settings…</p>
  {:else}
    <div class="grid" bind:this={body} onscroll={measureScroll} class:more-above={moreAbove} class:more-below={moreBelow}>
      <fieldset>
        <legend>History</legend>
        <label class="row">
          <span>Keep up to</span>
          <input
            type="number"
            min={MAX_ITEMS_RANGE.min}
            max={MAX_ITEMS_RANGE.max}
            step="1"
            bind:value={draft.maxItems}
            aria-invalid={!!errors.maxItems}
            aria-describedby={errors.maxItems ? "err-max" : undefined}
          />
          <span>clips</span>
        </label>
        {#if errors.maxItems}<p class="error" id="err-max">{errors.maxItems}</p>{/if}
        <label class="row">
          <span>Remove clips after</span>
          <select value={draft.retentionDays === null ? "forever" : String(draft.retentionDays)} onchange={retentionValue}>
            {#each retentionOptions(draft.retentionDays) as o (o.label)}
              <option value={o.value === null ? "forever" : String(o.value)}>{o.label}</option>
            {/each}
          </select>
        </label>
        <p class="muted">Pinned clips are always kept.</p>
        <label class="check"><input type="checkbox" bind:checked={draft.captureImages} /> Capture images</label>
        <label class="check">
          <input
            type="checkbox"
            checked={settings?.capturePaused ?? false}
            onchange={(e) => onpause((e.currentTarget as HTMLInputElement).checked)}
          />
          Pause capturing
        </label>
      </fieldset>

      <fieldset>
        <legend>Shelf</legend>
        <div class="row">
          <span id="shortcut-label">Open shelf with</span>
          <button
            type="button"
            class="shortcut"
            class:recording
            aria-labelledby="shortcut-label"
            aria-describedby="shortcut-hint"
            onclick={() => (recording = !recording)}
            onkeydown={recordKey}
            onblur={() => (recording = false)}
          >
            {recording ? "Type shortcut…" : formatAccelerator(draft.shelfShortcut, isMac)}
          </button>
        </div>
        <p class="muted" id="shortcut-hint">
          {recording ? "Press the new shortcut, or Escape to cancel." : "Click to record a new shortcut."}
        </p>
        {#if errors.shelfShortcut}<p class="error">{errors.shelfShortcut}</p>{/if}
        <label class="check">
          <input type="checkbox" bind:checked={draft.pasteOnSelect} />
          Paste into the active app
        </label>
        <p class="muted">When off, choosing a clip only copies it.</p>
        <label class="check">
          <input type="checkbox" bind:checked={draft.linkPreviews} aria-describedby="link-previews-hint" />
          Show link previews
        </label>
        <p class="muted" id="link-previews-hint">
          On by default: Satchel fetches each web link you copy once from its site (which sees the link and your IP address)
          to show its title and image. Sign-in, one-time and private-network links are never fetched. Turn off to stop
          fetching and delete saved previews.
        </p>
      </fieldset>

      <fieldset class="updates">
        <legend>Updates</legend>
        {#if update}
          <div class="row">
            <span class="version">{APP_NAME} {update.currentVersion}</span>
            <span class="spacer"></span>
            {#if update.supported}
              <button type="button" class="small" disabled={updateBusy} onclick={() => runUpdate(updates.check)}>
                {update.checking ? "Checking…" : "Check Now"}
              </button>
            {/if}
          </div>
          <p class="update-line" class:failed={update.phase.state === "failed"} role="status">{updateLine(update, updateNow)}</p>
          {#if update.phase.state === "downloading"}
            <progress max="100" value={percent(update.phase)} aria-label="Download progress"></progress>
          {:else if update.phase.state === "available" || update.phase.state === "failed"}
            <div class="row">
              <button type="button" class="small primary" onclick={() => runUpdate(updates.install)}>
                {update.phase.state === "failed" ? "Try Again" : `Update to ${APP_NAME} ${update.phase.version}`}
              </button>
            </div>
          {/if}
          {#if updateError}<p class="error">{updateError}</p>{/if}
          {#if update.supported}
            <label class="check">
              <input type="checkbox" bind:checked={draft.checkForUpdates} aria-describedby="updates-hint" />
              Check for updates automatically
            </label>
            <p class="muted" id="updates-hint">
              Asks GitHub for the newest version when {APP_NAME} starts and every 6 hours. Nothing about you or your clips is
              sent. Updates install only when you choose, after their signature is checked. Off: no automatic checks.
            </p>
          {/if}
        {:else}
          <p class="muted">Loading…</p>
        {/if}
      </fieldset>

      <fieldset class="apps">
        <legend>Never capture from</legend>
        <p class="muted">Satchel ignores everything copied in these apps.</p>
        {#if resolving}
          <p class="muted">Loading apps…</p>
        {:else}
          <ul aria-label="Excluded apps">
            {#each rows as row (row.key)}
              {@render appRow(row)}
            {:else}
              <li class="muted empty">No excluded apps on this {isMac ? "Mac" : "computer"}.</li>
            {/each}
          </ul>
        {/if}
        <div class="row">
          <button type="button" class="small" bind:this={addButton} onclick={() => (pickerOpen = true)}>Add App…</button>
        </div>
      </fieldset>

      <p class="about">
        <button type="button" class="quiet" bind:this={noticesButton} onclick={() => (noticesOpen = true)}>Acknowledgements…</button>
      </p>
    </div>

    <div class="actions" class:divided={moreBelow}>
      {#if confirmClear}
        <span>Delete all unpinned clips?</span>
        <button type="button" class="danger" onclick={clear}>Clear History</button>
        <button type="button" onclick={() => (confirmClear = false)}>Keep</button>
      {:else}
        <button type="button" class="danger-quiet" onclick={() => (confirmClear = true)}>Clear History…</button>
      {/if}
      <span class="status" role="status">{message}</span>
      <span class="spacer"></span>
      <button type="button" disabled={!dirty} onclick={revert}>Revert</button>
      <button type="submit" class="primary" disabled={!dirty || !valid || saving}>Save</button>
    </div>
  {/if}
</form>

{#snippet appRow(row: ExcludedRow)}
  <li>
    <AppIcon icons={apps.icons} id={row.iconId} label={rowLabel(row)} size={20} />
    <span class="app-text">
      {#if row.name}
        <span class="app-name">{row.name}</span>
      {:else}
        <span class="app-id">{row.ids[0]}</span>
      {/if}
      {#if row.detail}<span class="app-detail" title={row.detail}>{row.detail}</span>{/if}
    </span>
    <button type="button" class="small" aria-label={removeLabel(row)} onclick={() => removeRow(row)}>Remove</button>
  </li>
{/snippet}

{#if pickerOpen && draft}
  <AppPicker {apps} excluded={draft.excludedAppIds} onadd={addApp} onclose={closePicker} />
{/if}

{#if noticesOpen}
  <Notices load={notices} onclose={closeNotices} />
{/if}

<style>
  .settings {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    font-size: 13px;
  }
  .loading {
    margin: 0;
    padding: 16px 20px;
  }
  /* The scrolling body: two columns (the window's minimum width fits them),
     one below that as a fallback. Rows keep their natural height. */
  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    align-content: start;
    gap: 14px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 16px 20px;
    scroll-padding-block: 16px;
    border-top: 1px solid transparent;
  }
  @media (max-width: 559px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .grid.more-above {
    border-top-color: var(--hair);
  }
  .grid.more-below {
    -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 28px), transparent);
    mask-image: linear-gradient(to bottom, #000 calc(100% - 28px), transparent);
  }
  fieldset {
    margin: 0;
    border: none;
    border-radius: 14px;
    background: var(--card);
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  legend {
    float: left;
    font-weight: 600;
    padding: 0;
    margin-bottom: 4px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  input[type="number"],
  select {
    height: 26px;
    border: none;
    border-radius: 7px;
    background: var(--field);
    padding: 0 8px;
    -webkit-user-select: text;
    user-select: text;
  }
  input[type="number"] {
    width: 90px;
  }
  input[aria-invalid="true"] {
    box-shadow: inset 0 0 0 1px var(--danger);
  }
  .muted {
    margin: -4px 0 0;
    color: var(--sec);
    font-size: 11.5px;
  }
  .error {
    margin: -4px 0 0;
    color: var(--danger);
    font-size: 11.5px;
  }
  .apps {
    grid-column: 1 / -1;
  }
  .apps ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: 4px 16px;
  }
  .apps li {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 28px;
    min-width: 0;
  }
  .apps li.empty {
    margin: 0;
    min-height: 0;
  }
  .app-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    line-height: 1.2;
  }
  .app-name,
  .app-id,
  .app-detail {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .app-id,
  .app-detail {
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--sec);
  }
  .app-id {
    font-size: 11.5px;
  }
  button {
    height: 28px;
    padding: 0 14px;
    border: none;
    border-radius: 14px;
    background: var(--field);
    font-weight: 500;
  }
  button:hover:not(:disabled) {
    background: var(--field-hover);
  }
  button:disabled {
    opacity: 0.45;
  }
  .small {
    height: 24px;
    padding: 0 10px;
    font-size: 12px;
  }
  .shortcut {
    min-width: 110px;
    font-family: inherit;
  }
  .shortcut.recording {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .primary {
    background: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .primary:hover:not(:disabled) {
    background: var(--accent);
    filter: brightness(1.08);
  }
  .danger {
    background: var(--danger);
    color: #fff;
  }
  .danger-quiet {
    color: var(--danger);
  }
  /* A quiet line under the sections, out of the way of the settings. */
  .about {
    grid-column: 1 / -1;
    margin: -6px 0 0;
    display: flex;
    justify-content: flex-end;
  }
  .quiet {
    height: 22px;
    padding: 0 8px;
    background: transparent;
    color: var(--sec);
    font-size: 11.5px;
    font-weight: 400;
  }
  .quiet:hover:not(:disabled) {
    background: transparent;
    color: var(--label);
  }
  /* Pinned under the scrolling body. */
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    padding: 10px 20px 14px;
    border-top: 1px solid transparent;
  }
  .actions.divided {
    border-top-color: var(--hair);
  }
  .status {
    color: var(--sec);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .version {
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }
  .update-line {
    margin: 0;
    font-size: 12px;
    color: var(--sec);
    font-variant-numeric: tabular-nums;
  }
  .update-line.failed {
    color: var(--danger);
  }
  progress {
    width: 100%;
    height: 6px;
    accent-color: var(--accent);
  }
</style>
