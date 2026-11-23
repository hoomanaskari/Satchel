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
  import { isMac } from "$lib/platform";
  import type { Settings } from "$lib/types";

  interface Props {
    settings: Settings | null;
    onsave: (s: Settings) => Promise<void>;
    onpause: (paused: boolean) => Promise<void>;
    onclear: () => Promise<void>;
  }
  let { settings, onsave, onpause, onclear }: Props = $props();

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
  let newApp = $state("");
  let confirmClear = $state(false);

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

  function addApp() {
    if (!draft) return;
    draft.excludedAppIds = normalizeExcluded([...draft.excludedAppIds, newApp]);
    newApp = "";
  }

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
    <p class="muted">Loading settings…</p>
  {:else}
    <div class="grid">
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
      </fieldset>

      <fieldset class="apps">
        <legend>Never capture from</legend>
        <ul aria-label="Excluded apps">
          {#each draft.excludedAppIds as id (id)}
            <li>
              <span>{id}</span>
              <button
                type="button"
                class="small"
                aria-label="Stop excluding {id}"
                onclick={() => draft && (draft.excludedAppIds = draft.excludedAppIds.filter((x) => x !== id))}>Remove</button
              >
            </li>
          {:else}
            <li class="muted">No excluded apps.</li>
          {/each}
        </ul>
        <div class="row">
          <label class="visually-hidden" for="new-app">App identifier to exclude</label>
          <input
            id="new-app"
            type="text"
            placeholder={isMac ? "com.example.app" : "app.exe"}
            bind:value={newApp}
            onkeydown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                addApp();
              }
            }}
          />
          <button type="button" class="small" disabled={!newApp.trim()} onclick={addApp}>Add</button>
        </div>
      </fieldset>
    </div>

    <div class="actions">
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

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 12px;
    height: 100%;
    padding: 6px 20px 14px;
    font-size: 13px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 14px;
    flex: 1;
    min-height: 0;
    overflow: auto;
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
  input[type="text"],
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
  input[type="text"] {
    flex: 1;
    min-width: 0;
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
  .apps ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 96px;
    overflow: auto;
  }
  .apps li {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .apps li span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--mono);
    font-size: 12px;
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
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .status {
    color: var(--sec);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
</style>
