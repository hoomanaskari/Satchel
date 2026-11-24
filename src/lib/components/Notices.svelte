<script lang="ts">
  // "Acknowledgements…" dialog of the settings window: the third-party
  // licence notices bundled with the app (THIRD-PARTY-NOTICES.txt), shown as
  // plain text. Escape or Done closes this dialog only; the window closes on
  // the next Escape.
  import { onMount } from "svelte";

  interface Props {
    /** The notices text (`get_third_party_notices`). */
    load: () => Promise<string>;
    onclose: () => void;
  }
  let { load, onclose }: Props = $props();

  let text = $state<string | null>(null);
  let loadError = $state("");
  let done = $state<HTMLButtonElement | null>(null);

  onMount(() => {
    done?.focus();
    load()
      .then((t) => (text = t))
      .catch((e) => (loadError = e instanceof Error ? e.message : String(e)));
  });

  function dialogKeys(e: KeyboardEvent) {
    if (e.key === "Escape" && !e.isComposing) {
      // Claimed here so the window's Escape handler leaves the window open.
      e.preventDefault();
      e.stopPropagation();
      onclose();
    } else if (e.key === "Tab") {
      // The text and Done are the only stops; keep Tab inside the dialog.
      e.preventDefault();
      const body = document.getElementById("notices-text");
      (document.activeElement === done ? body : done)?.focus();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div class="notices" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="notices-title" onkeydown={dialogKeys}>
    <h2 id="notices-title">Acknowledgements</h2>
    <p class="muted">Satchel includes the open source software below, used under these licences.</p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (scrollable text, reachable by keyboard) -->
    <pre id="notices-text" tabindex="0" aria-label="Third-party notices" aria-busy={text === null && !loadError}>{#if text !== null}{text}{:else if loadError}Could not load the notices: {loadError}{:else}Loading…{/if}</pre>
    <div class="footer">
      <span class="spacer"></span>
      <button type="button" bind:this={done} onclick={onclose}>Done</button>
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
  .notices {
    width: min(640px, calc(100vw - 40px));
    height: calc(100vh - 40px);
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
  .notices:focus {
    outline: none;
  }
  h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .muted {
    margin: 0;
    color: var(--sec);
    font-size: 11.5px;
  }
  pre {
    flex: 1;
    min-height: 0;
    overflow: auto;
    overscroll-behavior: contain;
    margin: 0;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--card);
    font-family: var(--mono);
    font-size: 11px;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    -webkit-user-select: text;
    user-select: text;
  }
  pre:focus-visible {
    outline: none;
    box-shadow: 0 0 0 3px var(--focus);
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
  button:hover {
    background: var(--field-hover);
  }
</style>
