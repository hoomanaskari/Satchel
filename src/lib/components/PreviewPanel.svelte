<script lang="ts">
  // Larger preview of the focused clip (Space). Text only; selectable.
  import AppIcon from "./AppIcon.svelte";
  import Icon from "./Icon.svelte";
  import type { AppIconCache } from "$lib/app-icons";
  import { appTileColor, cardPreview, formatBytes, kindLabel, previewText, relativeTime } from "$lib/format";
  import type { PreviewState } from "$lib/shelf.svelte";
  import type { ThumbnailCache } from "$lib/thumbs";
  import type { ClipSummary } from "$lib/types";

  let {
    clip,
    preview,
    thumbs,
    icons,
    now,
    onclose,
  }: {
    clip: ClipSummary;
    preview: PreviewState | null;
    thumbs: ThumbnailCache;
    icons: AppIconCache | null;
    now: number;
    onclose: () => void;
  } = $props();

  const card = $derived(cardPreview(clip));
  const body = $derived(previewText(preview?.text ?? clip.preview));
  const files = $derived(clip.kind === "file" ? (preview?.text ?? clip.preview).split(/\r?\n/).filter(Boolean) : []);

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
</script>

<section class="preview" aria-label="Preview" aria-live="polite">
  <div class="content">
    {#if clip.kind === "image"}
      <div class="image">
        {#if thumbUrl}<img src={thumbUrl} alt="" draggable="false" />{:else}<Icon name="image" size={40} />{/if}
      </div>
    {:else if clip.kind === "color"}
      <div class="color">
        {#if card.swatch}<span class="swatch" style:background-color={card.swatch}></span>{/if}
        <span class="hex">{clip.preview}</span>
      </div>
    {:else if clip.kind === "file"}
      <ul class="files">
        {#each files as f, i (i)}<li><Icon name="file" size={14} /><span>{f}</span></li>{/each}
      </ul>
    {:else if preview?.loading}
      <p class="muted">Loading…</p>
    {:else}
      <pre class:mono={clip.kind === "code"}>{body.text}</pre>
      {#if body.truncated}<p class="muted">Preview shortened. Paste to use the full text.</p>{/if}
    {/if}
  </div>

  <aside class="info">
    <div class="app">
      <AppIcon {icons} id={clip.sourceApp?.id ?? null} label={clip.sourceApp?.name ?? "?"} tint={appTileColor(clip.sourceApp)} size={22} />
      <span>{clip.sourceApp?.name ?? "Unknown App"}</span>
    </div>
    {#if clip.title}<p class="title">{clip.title}</p>{/if}
    <dl>
      <dt>Kind</dt>
      <dd>{kindLabel(clip.kind)}</dd>
      <dt>Copied</dt>
      <dd>{relativeTime(clip.createdAt, now)}</dd>
      <dt>Last used</dt>
      <dd>{relativeTime(clip.lastUsedAt, now)}</dd>
      {#if clip.kind === "image" && clip.preview.trim()}
        <dt>Dimensions</dt>
        <dd>{clip.preview}</dd>
      {/if}
      <dt>Size</dt>
      <dd>{formatBytes(clip.byteSize)}</dd>
      {#if preview?.text && clip.kind !== "image"}
        <dt>Characters</dt>
        <dd>{preview.text.length.toLocaleString()}</dd>
      {/if}
    </dl>
    <button type="button" class="close" onclick={onclose}>Close Preview</button>
  </aside>
</section>

<style>
  .preview {
    position: absolute;
    inset: 6px 20px 16px;
    z-index: 10;
    display: flex;
    background: var(--card);
    border-radius: 18px;
    box-shadow:
      0 0 0 3px var(--accent),
      0 10px 30px rgba(0, 0, 0, 0.2);
    overflow: hidden;
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 16px 18px;
    -webkit-user-select: text;
    user-select: text;
    cursor: text;
  }
  pre {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font: inherit;
    font-size: 14px;
    line-height: 1.5;
  }
  pre.mono {
    font-family: var(--mono);
    font-size: 12.5px;
  }
  .muted {
    color: var(--sec);
    font-size: 12px;
    margin: 10px 0 0;
  }
  .image {
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--sec);
  }
  .image img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: 8px;
  }
  .color {
    display: flex;
    align-items: center;
    gap: 18px;
    height: 100%;
  }
  .swatch {
    width: 160px;
    height: 160px;
    border-radius: 16px;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08);
  }
  .hex {
    font-family: var(--mono);
    font-size: 20px;
  }
  .files {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .files li {
    display: flex;
    gap: 8px;
    align-items: center;
    overflow-wrap: anywhere;
  }
  .info {
    width: 220px;
    flex: none;
    border-left: 1px solid var(--hair);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    font-size: 12px;
  }
  .app {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
  }
  .title {
    margin: 0;
    font-weight: 600;
    font-size: 13px;
    overflow-wrap: anywhere;
  }
  dl {
    margin: 0;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 10px;
  }
  dt {
    color: var(--sec);
  }
  dd {
    margin: 0;
  }
  .close {
    margin-top: auto;
    height: 28px;
    border: none;
    border-radius: 14px;
    background: var(--field);
    font-size: 12px;
    font-weight: 500;
  }
</style>
