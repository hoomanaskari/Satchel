<script lang="ts">
  // Inline SVG icons; decorative, so hidden from assistive tech.
  type Name = "search" | "pin" | "more" | "plus" | "close" | "image" | "file" | "check" | "link" | "pause" | "play";
  let { name, size = 15, filled = false }: { name: Name; size?: number; filled?: boolean } = $props();
</script>

<svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" class="icon">
  {#if name === "search"}
    <g fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
      <circle cx="11" cy="11" r="7"></circle><path d="M20 20l-3.5-3.5"></path>
    </g>
  {:else if name === "pin"}
    <g fill={filled ? "currentColor" : "none"} stroke="currentColor" stroke-width="2" stroke-linejoin="round" stroke-linecap="round">
      <path d="M9 3h6l-1 6 3 3v2H7v-2l3-3z"></path><path d="M12 14v7"></path>
    </g>
  {:else if name === "more"}
    <g fill="currentColor"><circle cx="5" cy="12" r="2"></circle><circle cx="12" cy="12" r="2"></circle><circle cx="19" cy="12" r="2"></circle></g>
  {:else if name === "plus"}
    <path d="M12 5v14M5 12h14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"></path>
  {:else if name === "close"}
    <path d="M6 6l12 12M18 6L6 18" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"></path>
  {:else if name === "image"}
    <g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round">
      <rect x="3" y="4" width="18" height="16" rx="3"></rect><path d="M3 16l5-5 4 4 3-3 6 6"></path><circle cx="15.5" cy="8.5" r="1.5"></circle>
    </g>
  {:else if name === "file"}
    <g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round">
      <path d="M6 3h8l4 4v14H6z"></path><path d="M14 3v4h4"></path>
    </g>
  {:else if name === "link"}
    <g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
      <path d="M10 14a4.5 4.5 0 0 0 6.4 0l3-3a4.5 4.5 0 0 0-6.4-6.4l-1 1"></path>
      <path d="M14 10a4.5 4.5 0 0 0-6.4 0l-3 3a4.5 4.5 0 0 0 6.4 6.4l1-1"></path>
    </g>
  {:else if name === "check"}
    <path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"></path>
  {:else if name === "pause"}
    <g fill="currentColor"><rect x="6.5" y="5" width="3.6" height="14" rx="1.2"></rect><rect x="13.9" y="5" width="3.6" height="14" rx="1.2"></rect></g>
  {:else if name === "play"}
    <path d="M8 5.6v12.8a1 1 0 0 0 1.5.86l10.4-6.4a1 1 0 0 0 0-1.72L9.5 4.74A1 1 0 0 0 8 5.6z" fill="currentColor"></path>
  {/if}
</svg>

<style>
  .icon {
    flex: none;
    display: block;
  }
</style>
