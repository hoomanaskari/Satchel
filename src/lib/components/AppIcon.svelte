<script lang="ts">
  // An app's icon, fetched from Rust when it scrolls into view. Without an
  // icon (not installed, no `id`, or no icon cache) a tile with the name's
  // initial stands in; while the icon is loading the tile is blank, so a
  // fast load does not flash a letter first. An icon already in the cache
  // is in the first frame: a re-rendered or re-shown list does not flicker.
  // The box is always `size` square, so nothing shifts when the icon lands.
  // Decorative: the name is always shown next to it.
  import { iconSlot, type AppIconCache } from "$lib/app-icons";
  import { appInitial } from "$lib/format";

  interface Props {
    icons: AppIconCache | null;
    /** App id to fetch the icon for; null shows the tile only. */
    id: string | null;
    /** Name (or identifier) the tile's initial comes from. */
    label: string;
    size?: number;
    /** The tile's colour (its initial is then white); a neutral tile without it. */
    tint?: string | null;
  }
  let { icons, id, label, size = 20, tint = null }: Props = $props();

  let el = $state<HTMLElement | null>(null);
  /** Result of a load this component started, for the id it was started for. */
  let loaded = $state<{ id: string; url: string | null } | null>(null);
  const initial = $derived(appInitial(label));
  // URL, null (no icon: show the initial) or undefined (loading: blank tile).
  const url = $derived(iconSlot(icons, id, loaded));

  $effect(() => {
    const target = el;
    const appId = id;
    const cache = icons;
    if (!target || !appId || !cache || iconSlot(cache, appId, null) !== undefined) return;
    let live = true;
    const load = () =>
      cache.get(appId).then((u) => {
        if (live) loaded = { id: appId, url: u };
      });
    if (typeof IntersectionObserver === "undefined") {
      load();
      return () => (live = false);
    }
    // Long lists only fetch what is on screen, plus a margin.
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          io.disconnect();
          load();
        }
      },
      { rootMargin: "160px" },
    );
    io.observe(target);
    return () => {
      live = false;
      io.disconnect();
    };
  });
</script>

<span class="app-icon" bind:this={el} aria-hidden="true" style:width="{size}px" style:height="{size}px">
  {#if url}
    <img src={url} alt="" width={size} height={size} draggable="false" />
  {:else}
    <span
      class="tile"
      class:pending={url === undefined}
      class:tinted={!!tint && url === null}
      style:background-color={tint && url === null ? tint : null}
      style:font-size="{Math.round(size * 0.5)}px">{url === null ? initial : ""}</span
    >
  {/if}
</span>

<style>
  .app-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }
  img {
    display: block;
  }
  .tile {
    /* App icons have a ~10% transparent margin; match it so rows line up. */
    width: 82%;
    height: 82%;
    border-radius: 24%;
    background: var(--field);
    color: var(--sec);
    font-weight: 600;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .tile.tinted {
    color: #fff;
    font-weight: 700;
  }
  .tile.pending {
    opacity: 0.6;
  }
</style>
