<script lang="ts">
  // History / pinboard segmented control with inline create and rename.
  import Icon from "./Icon.svelte";
  import { safeHexColor } from "$lib/format";
  import type { Pinboard, PinboardId } from "$lib/types";

  interface Props {
    pinboards: Pinboard[];
    active: PinboardId | null;
    /** Pinboard being renamed, or "new" while creating one. */
    editing: PinboardId | "new" | null;
    onselect: (id: PinboardId | null) => void;
    onedit: (target: PinboardId | "new" | null) => void;
    oncommit: (target: PinboardId | "new", name: string) => void;
    onmenu: (id: PinboardId, x: number, y: number) => void;
  }
  let { pinboards, active, editing, onselect, onedit, oncommit, onmenu }: Props = $props();

  const sorted = $derived([...pinboards].sort((a, b) => a.position - b.position));

  let input = $state<HTMLInputElement | null>(null);
  $effect(() => {
    if (editing !== null && input) {
      input.focus();
      input.select();
    }
  });

  let finished = false;
  function key(e: KeyboardEvent, target: PinboardId | "new") {
    if (e.key === "Enter") {
      e.preventDefault();
      finished = true;
      oncommit(target, input?.value ?? "");
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      finished = true;
      onedit(null);
    }
  }
  function blur(target: PinboardId | "new") {
    if (!finished) oncommit(target, input?.value ?? "");
    finished = false;
  }
</script>

{#snippet nameInput(target: PinboardId | "new", value: string)}
  <label class="visually-hidden" for="pinboard-name">{target === "new" ? "New pinboard name" : "Pinboard name"}</label>
  <input
    id="pinboard-name"
    bind:this={input}
    class="name"
    type="text"
    {value}
    maxlength="60"
    placeholder="Pinboard"
    onkeydown={(e) => key(e, target)}
    onblur={() => blur(target)}
  />
{/snippet}

<div class="seg" role="group" aria-label="Lists">
  <button type="button" aria-pressed={active === null} class:on={active === null} onclick={() => onselect(null)}>History</button>
  {#each sorted as p (p.id)}
    {#if editing === p.id}
      {@render nameInput(p.id, p.name)}
    {:else}
      <button
        type="button"
        aria-pressed={active === p.id}
        class:on={active === p.id}
        title="Double-click to rename"
        onclick={() => onselect(p.id)}
        ondblclick={() => onedit(p.id)}
        oncontextmenu={(e) => {
          e.preventDefault();
          onmenu(p.id, e.clientX, e.clientY);
        }}
      >
        {#if safeHexColor(p.color)}<span class="dot" style:background-color={p.color}></span>{/if}
        <span class="label">{p.name}</span>
      </button>
    {/if}
  {/each}
  {#if editing === "new"}
    {@render nameInput("new", "")}
  {/if}
  <button type="button" class="add" aria-label="New pinboard" title="New pinboard" onclick={() => onedit("new")}>
    <Icon name="plus" size={13} />
  </button>
</div>

<style>
  .seg {
    display: flex;
    background: var(--field);
    border-radius: 10px;
    padding: 2px;
    height: 36px;
    min-width: 0;
    max-width: 46vw;
    overflow-x: auto;
    scrollbar-width: none;
    flex: none;
  }
  .seg::-webkit-scrollbar {
    display: none;
  }
  button {
    min-width: 84px;
    max-width: 180px;
    padding: 0 14px;
    border: none;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 500;
    background: transparent;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    flex: none;
  }
  button.on {
    background: var(--seg-on);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.18);
  }
  .add {
    min-width: 32px;
    padding: 0 8px;
    color: var(--sec);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name {
    width: 130px;
    border: none;
    border-radius: 8px;
    background: var(--card);
    padding: 0 10px;
    font-size: 13px;
    -webkit-user-select: text;
    user-select: text;
    flex: none;
  }
</style>
