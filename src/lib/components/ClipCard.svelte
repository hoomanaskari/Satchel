<script lang="ts">
  // One clip card. Clip content only ever reaches text nodes, `alt=""`
  // images from our own blob URLs, and validated hex colors.
  import Icon from "./Icon.svelte";
  import { appInitial, appTileColor, cardLabel, cardPreview, kindLabel, relativeTime } from "$lib/format";
  import type { ThumbnailCache } from "$lib/thumbs";
  import type { ClipSummary } from "$lib/types";
  import type { ClickMode } from "$lib/shelf.svelte";
  import { isMac } from "$lib/platform";

  interface Props {
    clip: ClipSummary;
    selected: boolean;
    /** The selection's focus: the one card in the tab order. */
    current: boolean;
    pinned: boolean;
    shortcut: string;
    now: number;
    renaming: boolean;
    thumbs: ThumbnailCache;
    onselect: (mode: ClickMode) => void;
    onpaste: () => void;
    onpin: (anchor: HTMLElement) => void;
    onmenu: (x: number, y: number) => void;
    onrename: (title: string | null) => void;
  }

  let { clip, selected, current, pinned, shortcut, now, renaming, thumbs, onselect, onpaste, onpin, onmenu, onrename }: Props =
    $props();

  const p = $derived(cardPreview(clip));
  const label = $derived(cardLabel(clip, now));
  const meta = $derived(`${kindLabel(clip.kind)} · ${relativeTime(clip.lastUsedAt, now)}`);
  const appName = $derived(clip.sourceApp?.name ?? "Unknown App");

  let thumbUrl = $state<string | null>(null);

  // Keyed on the id alone, so a refreshed clip object does not refetch the thumbnail.
  const thumbId = $derived(clip.hasThumbnail ? clip.id : null);
  $effect(() => {
    const id = thumbId;
    if (id === null) return;
    let alive = true;
    thumbs.acquire(id).then((u) => {
      if (alive) thumbUrl = u;
    });
    return () => {
      alive = false;
      thumbUrl = null;
      thumbs.release(id);
    };
  });

  function click(e: MouseEvent) {
    const toggle = isMac ? e.metaKey : e.ctrlKey;
    onselect(e.shiftKey ? "range" : toggle ? "toggle" : "single");
  }

  function contextmenu(e: MouseEvent) {
    e.preventDefault();
    if (!selected) onselect("single");
    onmenu(e.clientX, e.clientY);
  }

  let input = $state<HTMLInputElement | null>(null);
  $effect(() => {
    if (renaming && input) {
      input.focus();
      input.select();
    }
  });

  let done = false;
  function renameKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      done = true;
      onrename(input?.value ?? "");
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      done = true;
      onrename(null);
    }
  }
  function renameBlur() {
    if (!done) onrename(input?.value ?? "");
    done = false;
  }
</script>

<article class="card" class:selected>
  <div class="head">
    <span class="tile" aria-hidden="true" style:background-color={appTileColor(clip.sourceApp)}>{appInitial(clip.sourceApp)}</span>
    <span class="app">{appName}</span>
    <button
      type="button"
      class="pin"
      class:on={pinned}
      aria-label={pinned ? "Pinned. Change pinboards" : "Pin"}
      aria-pressed={pinned}
      onclick={(e) => onpin(e.currentTarget)}
    >
      <Icon name="pin" filled={pinned} />
    </button>
  </div>

  {#if renaming}
    <div class="rename">
      <label class="visually-hidden" for="rename-{clip.id}">Clip title</label>
      <input
        id="rename-{clip.id}"
        bind:this={input}
        type="text"
        value={clip.title ?? ""}
        placeholder="Title"
        maxlength="200"
        onkeydown={renameKey}
        onblur={renameBlur}
      />
    </div>
  {/if}

  <button
    type="button"
    class="body"
    data-card={clip.id}
    aria-pressed={selected}
    aria-label={label}
    tabindex={current ? 0 : -1}
    onclick={click}
    ondblclick={onpaste}
    oncontextmenu={contextmenu}
  >
    {#if clip.kind === "color" && p.swatch}
      <span class="swatch" style:background-color={p.swatch}></span>
    {:else if clip.kind === "image"}
      <span class="thumb">
        {#if thumbUrl}
          <img src={thumbUrl} alt="" draggable="false" />
        {:else}
          <Icon name="image" size={26} />
        {/if}
      </span>
    {/if}
    {#if p.head && !renaming}
      <span class="title">{p.head}</span>
    {/if}
    <span class="text" class:mono={p.mono} class:dim={!!p.head}>{p.body}</span>
    <span class="meta"><span>{meta}</span><span>{shortcut}</span></span>
  </button>
</article>

<style>
  .card {
    width: 226px;
    height: 236px;
    background: var(--card);
    border-radius: 18px;
    box-shadow:
      0 1px 3px rgba(0, 0, 0, 0.1),
      0 6px 16px rgba(0, 0, 0, 0.06);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
  }
  .card.selected {
    box-shadow:
      0 0 0 3px var(--accent),
      0 1px 3px rgba(0, 0, 0, 0.1),
      0 6px 16px rgba(0, 0, 0, 0.06);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 6px 0 12px;
    height: 32px;
    flex: none;
  }
  .tile {
    width: 20px;
    height: 20px;
    border-radius: 6px;
    color: #fff;
    font-size: 11px;
    font-weight: 700;
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }
  .app {
    font-size: 12px;
    font-weight: 600;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pin {
    width: 32px;
    height: 32px;
    border: none;
    background: transparent;
    border-radius: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--sec);
    flex: none;
  }
  .pin:hover {
    background: var(--field);
  }
  .pin.on {
    color: var(--accent);
  }
  .rename {
    padding: 6px 12px 0;
  }
  .rename input {
    width: 100%;
    height: 26px;
    border: none;
    border-radius: 7px;
    background: var(--field);
    padding: 0 8px;
    font-size: 13px;
    font-weight: 600;
    -webkit-user-select: text;
    user-select: text;
  }
  .body {
    flex: 1;
    min-height: 0;
    background: transparent;
    border: none;
    padding: 8px 12px 10px;
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow: hidden;
    border-radius: 0 0 18px 18px;
  }
  .body:focus-visible {
    outline-offset: -3px;
  }
  .swatch,
  .thumb {
    width: 100%;
    height: 92px;
    border-radius: 10px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .swatch {
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08);
  }
  .thumb {
    background: var(--sub);
    color: var(--sec);
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .title {
    font-size: 13px;
    font-weight: 600;
    line-height: 1.3;
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow-wrap: anywhere;
    flex: none;
  }
  .text {
    font-size: 13px;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow: hidden;
    overflow-wrap: anywhere;
    flex: 1;
    min-height: 0;
    width: 100%;
  }
  .text.dim {
    color: var(--sec);
  }
  .text.mono {
    font-family: var(--mono);
    font-size: 12px;
  }
  .meta {
    display: flex;
    justify-content: space-between;
    width: 100%;
    font-size: 11px;
    color: var(--sec);
    flex: none;
  }
</style>
