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
  import { isMac, modLabel } from "$lib/platform";
  import { KIND_SCOPES } from "$lib/query";
  import { Shelf } from "$lib/shelf.svelte";
  import { ThumbnailCache } from "$lib/thumbs";
  import type { ClipId, PinboardId } from "$lib/types";

  const shelf = new Shelf({ api });
  const thumbs = new ThumbnailCache({
    fetch: (id) => api.getClipThumbnail(id),
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
  const verb = $derived(shelf.pasteOnSelect ? "Paste" : "Copy");

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
      .catch((e) => (shelf.flash = e instanceof Error ? e.message : String(e)));
    const vis = () => shelf.setVisible(!document.hidden);
    document.addEventListener("visibilitychange", vis);
    searchEl?.focus();
    return () => {
      gone = true;
      dispose?.();
      document.removeEventListener("visibilitychange", vis);
      thumbs.clear();
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
      { label: n > 1 ? `${verb} ${n} Clips` : verb, hint: "↩", action: () => void shelf.paste(false) },
      { label: `${verb} as Plain Text`, hint: "⇧↩", action: () => void shelf.paste(true) },
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
</script>

<svelte:head><title>Satchel</title></svelte:head>
<svelte:window {onkeydown} />

<main class="panel" aria-label="Clipboard shelf" aria-busy={!ready}>
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

    <div class="spacer"></div>
    <div class="count" aria-live="polite">{shelf.resultLabel}</div>
    <button type="button" class="round" aria-label="More options" aria-haspopup="menu" onclick={(e) => openMoreMenu(e.currentTarget)}>
      <Icon name="more" size={18} />
    </button>
  </header>

  <div class="list">
    <CardList
      {shelf}
      {thumbs}
      {renaming}
      onpin={openPinMenu}
      onmenu={(x, y) => (menu = { items: clipItems(), x, y, label: "Clip" })}
      onrename={commitRename}
    />
    {#if shelf.previewOpen && focusClip}
      <PreviewPanel clip={focusClip} preview={shelf.preview} {thumbs} now={shelf.now} onclose={() => shelf.closePreview()} />
    {/if}
  </div>

  <footer class="bottom">
    <p class="status" role="status">{shelf.status}</p>
    <div class="spacer"></div>
    <button type="button" class="pill" disabled={!hasSelection} onclick={() => void shelf.paste(true)}>{verb} as Plain Text</button>
    <button type="button" class="pill primary" disabled={!hasSelection} onclick={() => void shelf.paste(false)}>{verb}</button>
  </footer>

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
    border-radius: var(--panel-radius);
    background: var(--panel);
    border: 1px solid var(--panel-bd);
    box-shadow: 0 8px 30px rgba(0, 0, 0, 0.3);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 20px 12px;
    min-width: 0;
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
    min-width: 140px;
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
    min-width: 0;
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
  .spacer {
    flex: 1;
  }
  .count {
    font-size: 12px;
    color: var(--sec);
    white-space: nowrap;
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
  }
  .bottom {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 20px;
    height: 56px;
    border-top: 1px solid var(--hair);
    flex: none;
  }
  .status {
    margin: 0;
    font-size: 12px;
    color: var(--sec);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 620px;
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
  .pill:disabled {
    opacity: 0.45;
  }
</style>
