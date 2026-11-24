<script lang="ts">
  // The shelf (Tauri window "shelf"). Thin view over `Shelf`: it wires DOM
  // events to actions and renders state; decisions live in $lib.
  import { onMount, tick, untrack } from "svelte";
  import CardList from "$lib/components/CardList.svelte";
  import Confirm from "$lib/components/Confirm.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Menu, { type MenuItem } from "$lib/components/Menu.svelte";
  import PinboardTabs from "$lib/components/PinboardTabs.svelte";
  import PreviewPanel from "$lib/components/PreviewPanel.svelte";
  import { safeHexColor } from "$lib/format";
  import { api } from "$lib/ipc";
  import { mapKey, type KeyContext } from "$lib/keyboard";
  import { isMac, modLabel, shelfBackground } from "$lib/platform";
  import { KIND_SCOPES } from "$lib/query";
  import { AppIconCache } from "$lib/app-icons";
  import { Shelf } from "$lib/shelf.svelte";
  import { ThumbnailCache } from "$lib/thumbs";
  import type { ClipId, PinboardId } from "$lib/types";

  const shelf = new Shelf({ api });
  // Fixed for the life of the page: Rust announces it before the page loads.
  const glass = shelfBackground() === "glass";
  const thumbs = new ThumbnailCache({
    fetch: (id) => api.getClipThumbnail(id),
    createUrl: (b) => URL.createObjectURL(b),
    revoke: (u) => URL.revokeObjectURL(u),
  });
  // Link preview images: fetched and cached by Rust, handed over as bytes.
  const linkImages = new ThumbnailCache({
    fetch: (id) => api.getLinkImage(id),
    createUrl: (b) => URL.createObjectURL(b),
    revoke: (u) => URL.revokeObjectURL(u),
  });
  // Source-app icons on the cards: one entry per app, kept for the life of
  // the page (the shelf is shown and hidden, never reloaded), so reopening
  // the shelf or re-rendering cards never refetches or flickers.
  const icons = new AppIconCache({
    fetch: (id) => api.getAppIcon(id),
    createUrl: (b) => URL.createObjectURL(b),
    revoke: (u) => URL.revokeObjectURL(u),
  });

  let searchEl = $state<HTMLInputElement | null>(null);
  let menu = $state<{ items: MenuItem[]; x: number; y: number; label: string } | null>(null);
  let confirm = $state<{ title: string; detail: string; confirmLabel: string; run: () => void } | null>(null);
  let renaming = $state<ClipId | null>(null);
  let editingBoard = $state<PinboardId | "new" | null>(null);
  let ready = $state(false);

  const overlayOpen = $derived(!!menu || !!confirm);
  const focusClip = $derived(shelf.focusClip);
  const hasSelection = $derived(shelf.selection.ids.length > 0);
  const labels = $derived(shelf.pasteLabels);
  const notice = $derived(shelf.notice);
  const update = $derived(shelf.update);

  onMount(() => {
    let dispose: (() => void) | null = null;
    let gone = false;
    shelf
      .init()
      .then((d) => {
        if (gone) d();
        else dispose = d;
        ready = true;
      })
      .catch((e) => shelf.fail(e));
    const vis = () => shelf.setVisible(!document.hidden);
    document.addEventListener("visibilitychange", vis);
    searchEl?.focus();
    return () => {
      gone = true;
      dispose?.();
      document.removeEventListener("visibilitychange", vis);
      thumbs.clear();
      linkImages.clear();
      icons.clear();
    };
  });

  // Shown again (or Cmd+F): drop transient UI and focus the search field.
  $effect(() => {
    void shelf.focusSearchTick;
    untrack(() => {
      menu = null;
      confirm = null;
      renaming = null;
      editingBoard = null;
      searchEl?.focus();
    });
  });

  function keyContext(): KeyContext {
    const a = document.activeElement;
    const inField = a instanceof HTMLElement && a.matches("input:not([type=checkbox]):not([type=radio]), textarea, select, [contenteditable]");
    const fieldSel = a instanceof HTMLInputElement && a.selectionStart !== a.selectionEnd;
    const pageSel = (getSelection()?.toString() ?? "") !== "";
    return {
      mac: isMac,
      focus: a === searchEl ? "search" : inField ? "text" : "other",
      searchEmpty: shelf.text === "",
      hasTextSelection: fieldSel || pageSel,
      previewOpen: shelf.previewOpen,
      onControl: !inField && a instanceof HTMLElement && !a.dataset.card && a.matches("button, a, input, select, summary"),
    };
  }

  async function onkeydown(e: KeyboardEvent) {
    if (e.defaultPrevented) return;
    if (overlayOpen) return;
    const action = mapKey(e, keyContext());
    if (!action) return;
    if (action.type === "typeToSearch") {
      // Focus moves before the key's default action, so the character lands in the field.
      searchEl?.focus();
      return;
    }
    e.preventDefault();
    if (action.type === "delete" && shelf.selection.ids.length > 1) {
      askDeleteClips();
      return;
    }
    const cardFocused = document.activeElement instanceof HTMLElement && !!document.activeElement.dataset.card;
    await shelf.run(action);
    if (cardFocused && shelf.selection.focus != null) {
      await tick();
      document.querySelector<HTMLElement>(`[data-card="${shelf.selection.focus}"]`)?.focus();
    }
  }

  function askDeleteClips() {
    const n = shelf.selection.ids.length;
    confirm = {
      title: `Delete ${n} clips?`,
      detail: "They are removed from history and every pinboard. This cannot be undone.",
      confirmLabel: "Delete",
      run: () => void shelf.deleteSelected(),
    };
  }

  function askDeletePinboard(id: PinboardId) {
    const p = shelf.pinboards.find((x) => x.id === id);
    if (!p) return;
    confirm = {
      title: `Delete “${p.name}”?`,
      detail: "The pinboard is removed. Its clips stay in History until they expire.",
      confirmLabel: "Delete Pinboard",
      run: () => void shelf.deletePinboard(id),
    };
  }

  function pinboardItems(clipId: ClipId | null, withNew = true): MenuItem[] {
    const clip = clipId == null ? null : shelf.clips.find((c) => c.id === clipId);
    if (!clip) return [];
    const boards = [...shelf.pinboards].sort((a, b) => a.position - b.position);
    return [
      { heading: "Pin to" },
      ...boards.map((b) => ({
        label: b.name,
        dot: safeHexColor(b.color),
        checked: clip.pinboardIds.includes(b.id),
        action: () => void shelf.togglePinboard(clip.id, b.id),
      })),
      ...(withNew ? [{ label: "New Pinboard…", action: () => (editingBoard = "new") }] : []),
    ];
  }

  function clipItems(): MenuItem[] {
    const single = shelf.selection.ids.length === 1;
    const n = shelf.selection.ids.length;
    if (n === 0) return [];
    return [
      { label: labels.paste, hint: "↩", action: () => void shelf.paste(false) },
      { label: labels.alternate, hint: "⇧↩", action: () => void shelf.paste(true) },
      { label: "Copy", hint: `${modLabel}C`, action: () => void shelf.copy(false) },
      { label: "Preview", hint: "Space", disabled: !single, action: () => void shelf.togglePreview() },
      { label: "Rename…", disabled: !single, action: () => (renaming = shelf.selection.focus) },
      { separator: true },
      ...(single ? pinboardItems(shelf.selection.focus, false) : [{ label: "Pin", hint: `${modLabel}P`, action: () => void shelf.togglePin() }]),
      { separator: true },
      {
        label: n > 1 ? `Delete ${n} Clips` : "Delete",
        hint: isMac ? "⌘⌫" : "Ctrl+Del",
        danger: true,
        action: () => (n > 1 ? askDeleteClips() : void shelf.deleteSelected()),
      },
    ];
  }

  function openMoreMenu(anchor: HTMLElement) {
    const r = anchor.getBoundingClientRect();
    const board = shelf.activePinboard;
    const items: MenuItem[] = [...clipItems()];
    if (items.length) items.push({ separator: true });
    items.push({ label: "New Pinboard…", action: () => (editingBoard = "new") });
    if (board) {
      items.push(
        { label: `Rename “${board.name}”…`, action: () => (editingBoard = board.id) },
        { label: `Delete “${board.name}”…`, danger: true, action: () => askDeletePinboard(board.id) },
      );
    }
    items.push(
      { separator: true },
      {
        label: "Pause Capturing",
        checked: shelf.settings?.capturePaused ?? false,
        action: () => void shelf.setPaused(!(shelf.settings?.capturePaused ?? false)),
      },
      { label: "Settings…", hint: `${modLabel},`, action: () => void shelf.openSettings() },
    );
    menu = { items, x: r.right - 240, y: r.bottom + 6, label: "More options" };
  }

  function openPinMenu(clipId: ClipId, anchor: HTMLElement) {
    if (shelf.pinboards.length <= 1) {
      void shelf.togglePin([clipId]);
      return;
    }
    const r = anchor.getBoundingClientRect();
    menu = { items: pinboardItems(clipId), x: r.left, y: r.bottom + 4, label: "Pinboards" };
  }

  function commitBoard(target: PinboardId | "new", name: string) {
    editingBoard = null;
    if (target === "new") void shelf.createPinboard(name);
    else void shelf.renamePinboard(target, name);
    searchEl?.focus();
  }

  function commitRename(id: ClipId, title: string | null) {
    renaming = null;
    if (title !== null) void shelf.rename(id, title);
    searchEl?.focus();
  }

  function closeMenu() {
    menu = null;
    searchEl?.focus();
  }

  // The window is larger than the panel: its transparent margins hold the
  // panel's shadow. A click there is a click outside the shelf, so it
  // dismisses it like one (macOS lets clicks on fully clear pixels through
  // to the window below, but not on the faint shadow).
  function onpointerdown(e: PointerEvent) {
    if (e.target === document.documentElement || e.target === document.body) void shelf.close();
  }
</script>

<svelte:head><title>Satchel</title></svelte:head>
<svelte:window {onkeydown} {onpointerdown} />

<main class="panel" class:glass aria-label="Clipboard shelf" aria-busy={!ready}>
  <header class="top">
    <div class="search">
      <Icon name="search" />
      <label for="q" class="visually-hidden">Search clipboard history</label>
      <input
        id="q"
        bind:this={searchEl}
        type="text"
        value={shelf.text}
        oninput={(e) => shelf.setText(e.currentTarget.value)}
        placeholder="Search"
        autocomplete="off"
        spellcheck="false"
        maxlength="256"
      />
    </div>

    <PinboardTabs
      pinboards={shelf.pinboards}
      active={shelf.pinboardId}
      editing={editingBoard}
      onselect={(id) => void shelf.setTab(id)}
      onedit={(t) => (editingBoard = t)}
      oncommit={commitBoard}
      onmenu={(id, x, y) => {
        const b = shelf.pinboards.find((p) => p.id === id);
        if (!b) return;
        menu = {
          label: "Pinboard",
          x,
          y,
          items: [
            { label: "Rename…", action: () => (editingBoard = id) },
            { label: "Delete…", danger: true, action: () => askDeletePinboard(id) },
          ],
        };
      }}
    />

    <div class="divider" aria-hidden="true"></div>

    <div class="kinds" role="group" aria-label="Kind">
      {#each KIND_SCOPES as k (k.id)}
        <button type="button" class:on={shelf.kind === k.id} aria-pressed={shelf.kind === k.id} onclick={() => void shelf.setKind(k.id)}>
          {k.label}
        </button>
      {/each}
    </div>

    <!-- The notice (errors, warnings, the missing permission) takes only the
         room left between the filters and the actions. Its live region is
         always in the DOM so screen readers announce what lands in it.
         Confirmations ("Pinned", "Copied") are announced, never shown. -->
    <p class="visually-hidden" role="status">{shelf.announcement}</p>
    <div class="notice-slot" class:active={!!notice} class:permission={notice?.tone === "permission"}>
      <div class="notice" class:error={notice?.tone === "error"} class:permission={notice?.tone === "permission"} title={notice?.text}>
        <p class="notice-text" role="alert">{notice?.text ?? ""}</p>
      </div>
      {#if notice?.tone === "permission"}
        <button type="button" class="pill small" aria-label="Open Accessibility Settings…" onclick={() => void shelf.openAccessibilitySettings()}>
          <span class="long">Open Accessibility Settings…</span><span class="short" aria-hidden="true">Open Settings…</span>
        </button>
      {/if}
    </div>
    <div class="count" class:hidden={!!notice} aria-live="polite">{shelf.resultLabel}</div>
    <!-- A state, not a notice: shown for as long as capture is paused, however
         it was paused (this menu, the tray, Settings). Clicking resumes. -->
    {#if shelf.paused}
      <button
        type="button"
        class="paused"
        aria-label="Capture paused. Resume capturing"
        title="Satchel is not saving copies. Click to resume."
        onclick={() => void shelf.setPaused(false)}
      >
        <span class="glyph idle"><Icon name="pause" size={11} /></span>
        <span class="glyph hover"><Icon name="play" size={11} /></span>
        <span>Paused</span>
      </button>
    {/if}
    <!-- Also a state: there while an update is on offer (found by Rust's own
         checks; nothing pops up). Clicking downloads, verifies and installs it,
         then Satchel reopens; meanwhile it shows the progress. -->
    {#if update}
      {#if update.actionable}
        <button
          type="button"
          class="update {update.tone}"
          aria-label={update.aria}
          title={update.title}
          onclick={() => void shelf.installUpdate()}
        >
          <span class="glyph"><Icon name={update.tone === "failed" ? "alert" : "update"} size={12} /></span>
          <span>{update.label}</span>
        </button>
      {:else}
        <div
          class="update busy"
          role="status"
          aria-label={update.aria}
          title={update.title}
          style:--progress={update.progress === null ? null : `${update.progress}%`}
        >
          <span class="glyph"><Icon name="update" size={12} /></span>
          <span>{update.label}</span>
        </div>
      {/if}
    {/if}
    <div class="actions">
      <button type="button" class="pill" disabled={!hasSelection} aria-label={labels.alternate} onclick={() => void shelf.paste(true)}>
        <span class="long">{labels.alternate}</span><span class="short" aria-hidden="true">{labels.alternateShort}</span>
      </button>
      <button type="button" class="pill primary" disabled={!hasSelection} onclick={() => void shelf.paste(false)}>{labels.paste}</button>
    </div>
    <button type="button" class="round" aria-label="More options" aria-haspopup="menu" onclick={(e) => openMoreMenu(e.currentTarget)}>
      <Icon name="more" size={18} />
    </button>
  </header>

  <div class="list">
    <CardList
      {shelf}
      {thumbs}
      {linkImages}
      {icons}
      {renaming}
      onpin={openPinMenu}
      onmenu={(x, y) => (menu = { items: clipItems(), x, y, label: "Clip" })}
      onrename={commitRename}
    />
    {#if shelf.previewOpen && focusClip}
      <PreviewPanel clip={focusClip} preview={shelf.preview} {thumbs} {linkImages} {icons} now={shelf.now} onclose={() => shelf.closePreview()} />
    {/if}
  </div>

  {#if confirm}
    <Confirm
      title={confirm.title}
      detail={confirm.detail}
      confirmLabel={confirm.confirmLabel}
      oncancel={() => {
        confirm = null;
        searchEl?.focus();
      }}
      onconfirm={() => {
        const run = confirm?.run;
        confirm = null;
        run?.();
        searchEl?.focus();
      }}
    />
  {/if}
</main>

{#if menu}
  <Menu items={menu.items} x={menu.x} y={menu.y} label={menu.label} onclose={closeMenu} />
{/if}

<style>
  .panel {
    position: fixed;
    left: var(--panel-inset);
    right: var(--panel-inset);
    bottom: var(--panel-inset);
    /* Exactly the window minus its top and bottom margins, like the native
       glass / blur view behind it (`--shelf-height` mirrors shelf.rs HEIGHT;
       a short screen clamps the window, hence the 100vh bound). */
    height: min(
      calc(var(--shelf-height) - var(--panel-top) - var(--panel-inset)),
      calc(100vh - var(--panel-top) - var(--panel-inset))
    );
    border-radius: var(--panel-radius);
    background: var(--panel);
    border: 1px solid var(--panel-bd);
    /* Both layers fade out inside the window's transparent margins (see
       shelf.rs PANEL_INSET / PANEL_TOP), so the window edge never cuts them:
       a soft rim reaching 12px (= --panel-inset) on every side, and a lift
       shadow shrunk by its spread so it reaches 22px up (< --panel-top) but
       only 12px to the sides and 2px down. */
    box-shadow:
      0 0 12px rgba(0, 0, 0, 0.22),
      0 -10px 20px -8px rgba(0, 0, 0, 0.28);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  /* Over Liquid Glass (macOS 26+) the panel tint is nearly clear so the
     glass shows; the glass itself keeps text legible on any wallpaper. Cards
     stay opaque. Fields get a touch more contrast than over the blur. The
     native glass draws its own rim, so the border is fainter. */
  .panel.glass {
    --panel: rgba(255, 255, 255, 0.12);
    --panel-bd: rgba(255, 255, 255, 0.35);
    --field: rgba(0, 0, 0, 0.07);
    --field-hover: rgba(0, 0, 0, 0.12);
    --hair: rgba(0, 0, 0, 0.12);
  }
  @media (prefers-color-scheme: dark) {
    .panel.glass {
      --panel: rgba(0, 0, 0, 0.16);
      --panel-bd: rgba(255, 255, 255, 0.12);
      --field: rgba(255, 255, 255, 0.12);
      --field-hover: rgba(255, 255, 255, 0.18);
      --hair: rgba(255, 255, 255, 0.14);
    }
  }
  /* One row, never wrapping. As the screen narrows: the search field, the
     pinboard tabs and the kind filters shrink (tabs and filters scroll
     sideways) down to their minimums; the item count goes (≤1180 px), then
     "… as Plain Text" shortens to "Plain Text" (≤1000 px), and on the
     narrowest panels (≤860 px) that button leaves the bar (the clip menu and
     Shift+Return still do it). The notice only takes the room left over, and
     180 px when it has something to say (the count makes way for it), so it
     never pushes the actions out. */
  .top {
    container: top / inline-size;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 20px 12px;
    min-width: 0;
    flex: none;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--field);
    border-radius: 10px;
    padding: 0 10px;
    height: 36px;
    width: 280px;
    flex: 0 1 280px;
    min-width: 120px;
    color: var(--sec);
  }
  .search:focus-within {
    box-shadow: 0 0 0 3px var(--focus);
  }
  .search input {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: none;
    outline: none;
    color: var(--label);
    font-size: 14px;
    height: 32px;
    -webkit-user-select: text;
    user-select: text;
  }
  .divider {
    width: 1px;
    height: 20px;
    background: var(--hair);
    flex: none;
  }
  .kinds {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 120px;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .kinds::-webkit-scrollbar {
    display: none;
  }
  .kinds button {
    height: 30px;
    padding: 0 12px;
    border-radius: 15px;
    border: none;
    background: transparent;
    color: var(--sec);
    font-size: 13px;
    font-weight: 500;
    flex: none;
  }
  .kinds button:hover {
    color: var(--label);
  }
  .kinds button.on {
    background: var(--field);
    color: var(--label);
  }
  .notice-slot {
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }
  .notice-slot.active {
    min-width: 180px;
  }
  /* Room for the start of the explanation as well as its button. */
  .notice-slot.permission {
    min-width: 400px;
  }
  .notice {
    display: none;
    min-width: 0;
    align-items: center;
    height: 28px;
    padding: 0 12px;
    border-radius: 14px;
    background: var(--field);
    color: var(--label);
    font-size: 12px;
  }
  .notice-slot.active .notice {
    display: flex;
  }
  .notice.error {
    color: var(--danger);
  }
  /* The explanation is long: two small lines fit the bar's 36 px row and
     show its first sentence; all of it is in the tooltip and announced. */
  .notice.permission {
    background: rgba(255, 159, 10, 0.2);
    height: 36px;
    border-radius: 10px;
    font-size: 11px;
    line-height: 1.2;
  }
  .notice.permission .notice-text {
    white-space: normal;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .notice-text {
    margin: 0;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    font-size: 12px;
    color: var(--sec);
    white-space: nowrap;
    flex: none;
  }
  .count.hidden {
    display: none;
  }
  /* Amber like the permission notice, so it reads as "needs attention" in
     light and dark, over glass or blur. The pause glyph turns into play on
     hover and focus: the click resumes. Never shrinks or leaves the bar. */
  .paused {
    --amber: #c25e00;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px 0 10px;
    border: none;
    border-radius: 14px;
    background: rgba(255, 159, 10, 0.2);
    box-shadow: inset 0 0 0 1px rgba(255, 159, 10, 0.35);
    color: var(--label);
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    flex: none;
  }
  .paused:hover {
    background: rgba(255, 159, 10, 0.3);
  }
  .paused .glyph {
    display: flex;
    color: var(--amber);
  }
  .paused .hover,
  .paused:hover .idle,
  .paused:focus-visible .idle {
    display: none;
  }
  .paused:hover .hover,
  .paused:focus-visible .hover {
    display: flex;
  }
  @media (prefers-color-scheme: dark) {
    .paused {
      --amber: #ffb340;
    }
  }
  /* The update control: blue, Satchel's accent, so it reads as an offer and
     not a warning; red only when the last attempt failed. While it downloads
     the pill fills from the left. Like "Paused", it never shrinks or leaves
     the bar. */
  .update {
    --tint: 0, 113, 227;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px 0 10px;
    border: none;
    border-radius: 14px;
    background: rgba(var(--tint), 0.16);
    box-shadow: inset 0 0 0 1px rgba(var(--tint), 0.32);
    color: var(--label);
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    flex: none;
    font-variant-numeric: tabular-nums;
  }
  button.update:hover {
    background: rgba(var(--tint), 0.26);
  }
  .update .glyph {
    display: flex;
    color: rgb(var(--tint));
  }
  .update.busy {
    background: linear-gradient(to right, rgba(var(--tint), 0.28) var(--progress, 0%), rgba(var(--tint), 0.12) var(--progress, 0%));
  }
  .update.failed {
    --tint: 215, 0, 21;
  }
  @media (prefers-color-scheme: dark) {
    .update {
      --tint: 10, 132, 255;
    }
    .update.failed {
      --tint: 255, 69, 58;
    }
  }
  .actions {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .pill .short {
    display: none;
  }
  @container top (max-width: 1180px) {
    .count {
      display: none;
    }
  }
  @container top (max-width: 860px) {
    .actions .pill:not(.primary) {
      display: none;
    }
  }
  @container top (max-width: 1000px) {
    .notice-slot.permission {
      min-width: 260px;
    }
    .pill .long {
      display: none;
    }
    .pill .short {
      display: inline;
    }
  }
  .round {
    width: 36px;
    height: 36px;
    border: none;
    border-radius: 18px;
    background: var(--field);
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }
  .round:hover,
  .pill:hover:not(:disabled):not(.primary) {
    background: var(--field-hover);
  }
  .list {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .pill {
    height: 32px;
    padding: 0 14px;
    border: none;
    border-radius: 16px;
    background: var(--field);
    font-size: 13px;
    font-weight: 500;
    flex: none;
  }
  .pill.primary {
    padding: 0 18px;
    background: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .pill.small {
    height: 28px;
    padding: 0 12px;
    border-radius: 14px;
    font-size: 12px;
  }
  .pill:disabled {
    opacity: 0.45;
  }
</style>
