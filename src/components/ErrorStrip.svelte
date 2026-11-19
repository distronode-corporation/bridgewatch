<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import { configErrorLines, errorLines, isFirstRun } from "../lib/format";
  import type { Status } from "../lib/types";

  interface Props {
    errors: string[];
    /**
     * The shell's status, when it has arrived. While the file does not load its
     * diagnostics are shown whatever the snapshot says (H9).
     */
    status?: Status | null;
    /** Open Settings where the broken file can be fixed. */
    onfix?: () => void;
    /** Open the setup wizard (a first launch with no file). */
    onsetup?: () => void;
  }

  let { errors, status = null, onfix, onsetup }: Props = $props();

  const lines = $derived(errorLines(errors, status));
  const configBroken = $derived(configErrorLines(status).length > 0);
  // A first launch is not a fault: the file does not exist YET. Same strip
  // (it must not vanish either), but it points at the wizard, not at a fix.
  const firstRun = $derived(isFirstRun(status));
</script>

<!--
  `Snapshot.errors` carries both per-watch fetch failures and the diagnostics
  from a configuration that failed to hot-reload. Both belong at the top and
  neither may be collapsed behind a disclosure: the whole point of surfacing a
  failed reload is that the user does not discover it hours later.

  ⛔ H9: the strip is ALSO fed from `Status`, which the popover asks for every
  second it is on screen. A snapshot published from the last good configuration
  knows nothing about the file, and a strip fed only from it vanished on the
  next tick while the file on disk still did not load.
-->
{#if lines.length > 0}
  <div
    class={[
      "strip border-border flex flex-col gap-0.5 border-b px-3 py-1.5 text-xs",
      firstRun ? "bg-tone-blue-bg text-tone-blue" : "bg-tone-red-bg text-tone-red",
    ]}
    role="alert"
    data-first-run={firstRun || undefined}
  >
    {#each lines as line (line)}
      <div class="line break-words" data-line>{line}</div>
    {/each}
    {#if firstRun && onsetup}
      <Button
        size="xs"
        variant="outline"
        class="setup mt-1 self-start"
        onclick={onsetup}>Set up bridgewatch…</Button
      >
    {:else if configBroken && onfix}
      <Button
        size="xs"
        variant="outline"
        class="fix mt-1 self-start"
        onclick={onfix}>Fix in Settings</Button
      >
    {/if}
  </div>
{/if}
