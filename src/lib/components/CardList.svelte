<script lang="ts">
  // Horizontally scrolling, windowed card list. Only cards that intersect
  // the viewport (plus a small overscan) are in the DOM; the track has the
  // full width so the scrollbar and keyboard reveal stay exact.
  import { untrack } from "svelte";
  import ClipCard from "./ClipCard.svelte";
  import { emptyState } from "$lib/format";
  import { modLabel, prefersReducedMotion } from "$lib/platform";
  import type { AppIconCache } from "$lib/app-icons";
  import type { Shelf } from "$lib/shelf.svelte";
  import type { ThumbnailCache } from "$lib/thumbs";
  import type { ClipId } from "$lib/types";
  import { computeWindow, contentWidth, firstVisible, offsetOf, revealScroll, type Layout } from "$lib/windowing";

  interface Props {
    shelf: Shelf;
    thumbs: ThumbnailCache;
    icons: AppIconCache | null;
    renaming: ClipId | null;
    onpin: (id: ClipId, anchor: HTMLElement) => void;
    onmenu: (x: number, y: number) => void;
    onrename: (id: ClipId, title: string | null) => void;
  }
  let { shelf, thumbs, icons, renaming, onpin, onmenu, onrename }: Props = $props();

  const L: Layout = { item: 226, gap: 14, pad: 20 };

  let scroller = $state<HTMLDivElement | null>(null);
  let scrollLeft = $state(0);
  let viewport = $state(0);

  const count = $derived(shelf.clips.length);
  const win = $derived(computeWindow(scrollLeft, viewport, count, L, 3));
  const visible = $derived(shelf.clips.slice(win.start, win.end));
  const first = $derived(firstVisible(scrollLeft, viewport, count, L));
  const selected = $derived(new Set(shelf.selection.ids));
  const empty = $derived(emptyState(shelf.filters, shelf.activePinboard?.name ?? null));

  $effect(() => shelf.setFirstVisible(first));

  $effect(() => {
    void shelf.loadMoreIfNeeded(win.end);
  });

  // New filters: back to the start.
  $effect(() => {
    void shelf.resetTick;
    untrack(() => {
      if (scroller) scroller.scrollLeft = 0;
      scrollLeft = 0;
    });
  });

  // Keep the focused card in view when the selection moves.
  $effect(() => {
    const focus = shelf.selection.focus;
    if (focus == null) return;
    untrack(() => {
      if (!scroller) return;
      const i = shelf.order.indexOf(focus);
      if (i < 0) return;
      const target = revealScroll(i, scrollLeft, viewport || scroller.clientWidth, L);
      if (target !== scrollLeft) scroller.scrollTo({ left: target, behavior: prefersReducedMotion() ? "auto" : "smooth" });
    });
  });

  // A vertical wheel scrolls the shelf sideways.
  $effect(() => {
    const el = scroller;
    if (!el) return;
    const wheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
      e.preventDefault();
      el.scrollLeft += e.deltaY;
    };
    el.addEventListener("wheel", wheel, { passive: false });
    return () => el.removeEventListener("wheel", wheel);
  });

  function shortcutFor(index: number): string {
    const n = index - first + 1;
    return n >= 1 && n <= 9 ? `${modLabel}${n}` : "";
  }
</script>

<div
  class="scroller"
  bind:this={scroller}
  bind:clientWidth={viewport}
  onscroll={() => scroller && (scrollLeft = scroller.scrollLeft)}
>
  {#if shelf.isEmpty}
    <div class="empty" role="status">
      <div class="empty-title">{empty.title}</div>
      <div>{empty.detail}</div>
    </div>
  {:else}
    <!-- svelte-ignore a11y_no_redundant_roles (WebKit drops list semantics with list-style: none) -->
    <ul role="list" aria-label="Clips" class="track" style:width="{contentWidth(count, L)}px">
      {#each visible as clip, j (clip.id)}
        {@const i = win.start + j}
        <li class="slot" style:transform="translateX({offsetOf(i, L)}px)" aria-posinset={i + 1} aria-setsize={shelf.hasMore ? -1 : count}>
          <ClipCard
            {clip}
            {thumbs}
            {icons}
            selected={selected.has(clip.id)}
            current={shelf.selection.focus === clip.id}
            pinned={shelf.pinboardId !== null || clip.pinboardIds.length > 0}
            shortcut={shortcutFor(i)}
            now={shelf.now}
            renaming={renaming === clip.id}
            onselect={(mode) => shelf.select(clip.id, mode)}
            onpaste={() => {
              shelf.select(clip.id, "single");
              void shelf.paste(false);
            }}
            onpin={(anchor) => onpin(clip.id, anchor)}
            {onmenu}
            onrename={(title) => onrename(clip.id, title)}
          />
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .scroller {
    position: relative;
    height: 258px;
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: none;
    overscroll-behavior-x: contain;
  }
  .scroller::-webkit-scrollbar {
    display: none;
  }
  .track {
    position: relative;
    height: 100%;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .slot {
    position: absolute;
    top: 6px;
    left: 0;
    contain: layout style;
  }
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--sec);
  }
  .empty-title {
    color: var(--label);
    font-size: 17px;
    font-weight: 600;
  }
</style>
