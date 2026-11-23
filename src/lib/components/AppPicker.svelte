<script lang="ts">
  // "Add App…" dialog of the settings window: a searchable list of installed
  // and running apps (icon + name), the native chooser for any other app
  // bundle, and adding a raw identifier as a last resort. Escape closes this
  // dialog only; the window closes on the next Escape.
  import { onMount } from "svelte";
  import AppIcon from "./AppIcon.svelte";
  import Icon from "./Icon.svelte";
  import { filterCandidates, identifierError, isExcluded, moveActive, type AppDirectory } from "$lib/excluded-apps";
  import { isMac } from "$lib/platform";
  import type { AppInfo } from "$lib/types";

  interface Props {
    apps: AppDirectory;
    /** The draft's excluded ids: those apps are marked and cannot be picked again. */
    excluded: string[];
    /** Adds the app (or bare identifier) to the draft. The dialog closes itself afterwards. */
    onadd: (id: string, app: AppInfo | null) => void;
    onclose: () => void;
  }
  let { apps, excluded, onadd, onclose }: Props = $props();

  let all = $state.raw<AppInfo[] | null>(null);
  let loadError = $state("");
  let query = $state("");
  let active = $state(-1);
  let byId = $state(false);
  let identifier = $state("");
  let idTouched = $state(false);
  let note = $state("");
  let choosing = $state(false);
  let dialog = $state<HTMLElement | null>(null);
  let search = $state<HTMLInputElement | null>(null);
  let idInput = $state<HTMLInputElement | null>(null);

  const results = $derived(all ? filterCandidates(all, query, excluded) : []);
  const idError = $derived(identifierError(identifier, excluded));

  onMount(() => {
    search?.focus();
    apps
      .listApps()
      .then((list) => (all = list))
      .catch((e) => {
        all = [];
        loadError = e instanceof Error ? e.message : String(e);
      });
  });

  // A new result list selects its first pickable app.
  $effect(() => {
    active = moveActive(results, -1, 1);
  });

  $effect(() => {
    if (active < 0) return;
    document.getElementById(`picker-opt-${active}`)?.scrollIntoView({ block: "nearest" });
  });

  function pick(app: AppInfo & { excluded: boolean }) {
    if (app.excluded) return;
    onadd(app.id, app);
  }

  function listKeys(e: KeyboardEvent) {
    if (e.isComposing) return;
    const step =
      e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : e.key === "PageDown" ? 8 : e.key === "PageUp" ? -8 : 0;
    if (step) {
      e.preventDefault();
      let next = active;
      for (let i = 0; i < Math.abs(step); i++) next = moveActive(results, next, Math.sign(step));
      active = next;
    } else if (e.key === "Enter") {
      e.preventDefault();
      const app = results[active];
      if (app) pick(app);
    }
  }

  async function chooseOther() {
    note = "";
    choosing = true;
    try {
      const app = await apps.chooseApp();
      if (!app) return;
      if (isExcluded(excluded, app.id)) note = `${app.name} is already excluded.`;
      else onadd(app.id, app);
    } catch (e) {
      note = e instanceof Error ? e.message : String(e);
    } finally {
      choosing = false;
    }
  }

  function showIdentifier() {
    byId = true;
    queueMicrotask(() => idInput?.focus());
  }

  function addIdentifier() {
    idTouched = true;
    if (idError) return;
    onadd(identifier.trim(), null);
  }

  function dialogKeys(e: KeyboardEvent) {
    if (e.key === "Escape" && !e.isComposing) {
      // Claimed here so the window's Escape handler leaves the window open.
      e.preventDefault();
      e.stopPropagation();
      onclose();
    } else if (e.key === "Tab" && dialog) {
      // Keep Tab inside the dialog.
      const focusable = [...dialog.querySelectorAll<HTMLElement>("input, button:not(:disabled)")];
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div
    class="picker"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-labelledby="picker-title"
    bind:this={dialog}
    onkeydown={dialogKeys}
  >
    <h2 id="picker-title">Add an App to Never Capture From</h2>
    <div class="search">
      <Icon name="search" size={13} />
      <input
        bind:this={search}
        bind:value={query}
        type="text"
        role="combobox"
        aria-label="Search apps"
        aria-expanded="true"
        aria-controls="picker-list"
        aria-autocomplete="list"
        aria-activedescendant={active >= 0 ? `picker-opt-${active}` : undefined}
        placeholder="Search apps"
        autocomplete="off"
        spellcheck="false"
        onkeydown={listKeys}
      />
    </div>

    <ul id="picker-list" role="listbox" aria-label="Apps" aria-busy={all === null}>
      {#each results as app, i (app.id)}
        <!-- svelte-ignore a11y_click_events_have_key_events (the search field drives the list: arrows + Return) -->
        <li
          id="picker-opt-{i}"
          role="option"
          aria-selected={i === active}
          aria-disabled={app.excluded}
          class:active={i === active}
          class:excluded={app.excluded}
          onclick={() => pick(app)}
          onmousemove={() => !app.excluded && active !== i && (active = i)}
        >
          <AppIcon icons={apps.icons} id={app.id} label={app.name} size={22} />
          <span class="name">{app.name}</span>
          {#if app.excluded}
            <span class="tag"><Icon name="check" size={11} /> Excluded</span>
          {:else if app.running}
            <span class="tag">Running</span>
          {/if}
        </li>
      {/each}
    </ul>
    <p class="status" role="status">
      {#if all === null}
        Looking for apps…
      {:else if loadError}
        Could not list apps: {loadError}
      {:else if all.length === 0}
        No apps found. Choose one or add it by identifier.
      {:else if results.length === 0}
        No app matches “{query.trim()}”.
      {:else}
        {results.length} {results.length === 1 ? "app" : "apps"}
      {/if}
      {#if note}<span class="note">{note}</span>{/if}
    </p>

    {#if byId}
      <div class="by-id">
        <label for="picker-identifier">{isMac ? "Bundle identifier" : "App identifier"}</label>
        <input
          id="picker-identifier"
          bind:this={idInput}
          bind:value={identifier}
          type="text"
          placeholder={isMac ? "com.example.app" : "app.exe"}
          autocomplete="off"
          spellcheck="false"
          aria-invalid={idTouched && !!idError}
          aria-describedby={idTouched && idError ? "picker-identifier-error" : undefined}
          oninput={() => (idTouched = false)}
          onkeydown={(e) => {
            if (e.key === "Enter" && !e.isComposing) {
              e.preventDefault();
              addIdentifier();
            }
          }}
        />
        <button type="button" class="small" disabled={!identifier.trim()} onclick={addIdentifier}>Add</button>
      </div>
      {#if idTouched && idError}<p class="error" id="picker-identifier-error">{idError}</p>{/if}
    {/if}

    <div class="footer">
      {#if isMac}
        <button type="button" disabled={choosing} onclick={chooseOther}>Choose Other App…</button>
      {/if}
      {#if !byId}
        <button type="button" class="link" onclick={showIdentifier}>Add by Identifier…</button>
      {/if}
      <span class="spacer"></span>
      <button type="button" onclick={onclose}>Cancel</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.18);
  }
  @media (prefers-color-scheme: dark) {
    .backdrop {
      background: rgba(0, 0, 0, 0.4);
    }
  }
  .picker {
    width: min(440px, calc(100vw - 40px));
    height: min(420px, calc(100vh - 40px));
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px;
    border-radius: 16px;
    background: var(--panel-solid);
    box-shadow:
      0 0 0 0.5px var(--hair),
      0 18px 50px rgba(0, 0, 0, 0.28);
    font-size: 13px;
  }
  .picker:focus {
    outline: none;
  }
  h2 {
    margin: 0 0 2px;
    font-size: 13px;
    font-weight: 600;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 8px;
    border-radius: 8px;
    background: var(--field);
    color: var(--sec);
  }
  .search input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: none;
    background: transparent;
    color: var(--label);
    padding: 0;
    -webkit-user-select: text;
    user-select: text;
  }
  .search input:focus {
    outline: none;
  }
  .search:focus-within {
    box-shadow: 0 0 0 3px var(--focus);
  }
  ul {
    flex: 1;
    min-height: 0;
    overflow: auto;
    list-style: none;
    margin: 0;
    padding: 4px;
    border-radius: 10px;
    background: var(--card);
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 8px;
    border-radius: 7px;
  }
  li.active {
    background: var(--accent);
    color: #fff;
  }
  li.active .tag {
    color: rgba(255, 255, 255, 0.85);
  }
  li.excluded {
    color: var(--sec);
  }
  li.excluded :global(img) {
    opacity: 0.55;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    color: var(--sec);
    font-size: 11px;
  }
  .status {
    margin: 0;
    min-height: 15px;
    color: var(--sec);
    font-size: 11.5px;
  }
  .note {
    margin-left: 8px;
    color: var(--label);
  }
  .by-id {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .by-id label {
    color: var(--sec);
    font-size: 12px;
    white-space: nowrap;
  }
  .by-id input {
    flex: 1;
    min-width: 0;
    height: 26px;
    border: none;
    border-radius: 7px;
    background: var(--field);
    padding: 0 8px;
    font-family: var(--mono);
    font-size: 12px;
    -webkit-user-select: text;
    user-select: text;
  }
  .by-id input[aria-invalid="true"] {
    box-shadow: inset 0 0 0 1px var(--danger);
  }
  .error {
    margin: -4px 0 0;
    color: var(--danger);
    font-size: 11.5px;
  }
  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .spacer {
    flex: 1;
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
  .link {
    background: transparent;
    color: var(--accent);
    padding: 0 6px;
  }
  .link:hover:not(:disabled) {
    background: var(--field);
  }
</style>
