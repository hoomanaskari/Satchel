<script lang="ts">
  // The settings window (Tauri window "settings", opened by `open_settings`).
  // A regular app window: it fills the window and closes with Escape or
  // Cmd/Ctrl+W, or the native close button. Saving emits `settings-changed`
  // app-wide, which is how the shelf picks the change up.
  import { onMount } from "svelte";
  import SettingsPanel from "$lib/components/SettingsPanel.svelte";
  import { api } from "$lib/ipc";
  import { closesSettingsWindow } from "$lib/keyboard";
  import { isMac } from "$lib/platform";
  import type { Settings } from "$lib/types";

  let settings = $state.raw<Settings | null>(null);
  let error = $state("");

  const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

  onMount(() => {
    let stop: (() => void) | null = null;
    let gone = false;
    api
      .onSettingsChanged((s) => (settings = s))
      .then((u) => (gone ? u() : (stop = u)))
      .catch((e) => (error = message(e)));
    api
      .getSettings()
      .then((s) => (settings ??= s))
      .catch((e) => (error = message(e)));
    return () => {
      gone = true;
      stop?.();
    };
  });

  function onkeydown(e: KeyboardEvent) {
    // The shortcut recorder and the inline confirmation claim their keys first.
    if (e.defaultPrevented || !closesSettingsWindow(e, isMac)) return;
    e.preventDefault();
    api.closeSettings().catch((err) => (error = message(err)));
  }
</script>

<svelte:head><title>Satchel Settings</title></svelte:head>
<svelte:window {onkeydown} />

<main class="window">
  <h1 class="visually-hidden">Satchel Settings</h1>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <SettingsPanel
    {settings}
    onsave={(s) => api.saveSettings(s)}
    onpause={async (p) => {
      await api.setCapturePaused(p);
      if (settings) settings = { ...settings, capturePaused: p };
    }}
    onclear={async () => {
      await api.clearHistory();
    }}
  />
</main>

<style>
  .window {
    position: fixed;
    inset: 0;
    /* An opaque window: no native glass behind it, unlike the shelf. */
    background: var(--panel-solid);
    display: flex;
    flex-direction: column;
    padding-top: 14px;
  }
  .error {
    margin: 0 20px 8px;
    color: var(--danger);
    font-size: 12px;
  }
</style>
