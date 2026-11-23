<script lang="ts">
  // In-panel confirmation (window.confirm is not available in every webview).
  import { onMount } from "svelte";

  let {
    title,
    detail,
    confirmLabel,
    onconfirm,
    oncancel,
  }: { title: string; detail: string; confirmLabel: string; onconfirm: () => void; oncancel: () => void } = $props();

  let cancel = $state<HTMLButtonElement | null>(null);
  onMount(() => cancel?.focus());
</script>

<div class="scrim">
  <div
    class="box"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="confirm-title"
    aria-describedby="confirm-detail"
    tabindex="-1"
    onkeydown={(e) => {
      e.stopPropagation();
      if (e.key === "Escape") oncancel();
    }}
  >
    <h2 id="confirm-title">{title}</h2>
    <p id="confirm-detail">{detail}</p>
    <div class="buttons">
      <button type="button" bind:this={cancel} onclick={oncancel}>Cancel</button>
      <button type="button" class="danger" onclick={onconfirm}>{confirmLabel}</button>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: absolute;
    inset: 0;
    z-index: 40;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.12);
  }
  .box {
    width: 320px;
    background: var(--overlay);
    border-radius: 14px;
    border: 1px solid var(--hair);
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.25);
    padding: 16px 18px 14px;
  }
  h2 {
    margin: 0 0 6px;
    font-size: 14px;
  }
  p {
    margin: 0 0 14px;
    color: var(--sec);
    overflow-wrap: anywhere;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  button {
    height: 28px;
    padding: 0 14px;
    border: none;
    border-radius: 14px;
    background: var(--field);
    font-weight: 500;
  }
  .danger {
    background: var(--danger);
    color: #fff;
  }
</style>
