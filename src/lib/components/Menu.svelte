<script lang="ts" module>
  export type MenuItem =
    | {
        label: string;
        action: () => void;
        checked?: boolean;
        danger?: boolean;
        disabled?: boolean;
        hint?: string;
        /** Color dot (validated hex) shown before the label. */
        dot?: string | null;
      }
    | { separator: true }
    | { heading: string };
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "./Icon.svelte";

  let { items, x, y, label, onclose }: { items: MenuItem[]; x: number; y: number; label: string; onclose: () => void } =
    $props();

  let el = $state<HTMLDivElement | null>(null);
  let left = $state(0);
  let top = $state(0);

  onMount(() => {
    left = x;
    top = y;
    if (!el) return;
    // One measurement to keep the menu on screen.
    const r = el.getBoundingClientRect();
    left = Math.max(8, Math.min(x, innerWidth - r.width - 8));
    top = Math.max(8, Math.min(y, innerHeight - r.height - 8));
    buttons()[0]?.focus();

    const outside = (e: PointerEvent) => {
      if (el && !el.contains(e.target as Node)) onclose();
    };
    // Defer so the click that opened the menu does not close it.
    const t = setTimeout(() => addEventListener("pointerdown", outside, true), 0);
    return () => {
      clearTimeout(t);
      removeEventListener("pointerdown", outside, true);
    };
  });

  function buttons(): HTMLButtonElement[] {
    return el ? Array.from(el.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")) : [];
  }

  function keydown(e: KeyboardEvent) {
    e.stopPropagation();
    const list = buttons();
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const d = e.key === "ArrowDown" ? 1 : -1;
      list[(i + d + list.length) % list.length]?.focus();
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      list[e.key === "Home" ? 0 : list.length - 1]?.focus();
    } else if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      onclose();
    }
  }

  function pick(item: Extract<MenuItem, { action: () => void }>) {
    onclose();
    item.action();
  }
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="menu" role="menu" aria-label={label} bind:this={el} style:left="{left}px" style:top="{top}px" onkeydown={keydown}>
  {#each items as item, i (i)}
    {#if "separator" in item}
      <div class="sep" role="separator"></div>
    {:else if "heading" in item}
      <div class="heading" role="presentation">{item.heading}</div>
    {:else}
      <button
        type="button"
        role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
        aria-checked={item.checked === undefined ? undefined : item.checked}
        class:danger={item.danger}
        disabled={item.disabled}
        onclick={() => pick(item)}
      >
        <span class="check">
          {#if item.checked}<Icon name="check" size={13} />{/if}
        </span>
        {#if item.dot}<span class="dot" style:background-color={item.dot}></span>{/if}
        <span class="label">{item.label}</span>
        {#if item.hint}<span class="hint">{item.hint}</span>{/if}
      </button>
    {/if}
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 50;
    min-width: 220px;
    max-width: 320px;
    max-height: calc(100vh - 16px);
    overflow-y: auto;
    padding: 5px;
    background: var(--overlay);
    border: 1px solid var(--hair);
    border-radius: 10px;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.22);
    display: flex;
    flex-direction: column;
  }
  button {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px 0 4px;
    border: none;
    border-radius: 6px;
    background: transparent;
    text-align: left;
    font-size: 13px;
  }
  button:hover:not(:disabled),
  button:focus-visible {
    background: var(--accent);
    color: #fff;
    outline: none;
  }
  button:disabled {
    opacity: 0.4;
  }
  .danger {
    color: var(--danger);
  }
  .check {
    width: 16px;
    display: flex;
    justify-content: center;
    flex: none;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex: none;
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    color: var(--sec);
    font-size: 12px;
  }
  button:hover .hint,
  button:focus-visible .hint {
    color: inherit;
  }
  .sep {
    height: 1px;
    background: var(--hair);
    margin: 4px 8px;
  }
  .heading {
    font-size: 11px;
    font-weight: 600;
    color: var(--sec);
    padding: 6px 10px 2px 26px;
  }
</style>
