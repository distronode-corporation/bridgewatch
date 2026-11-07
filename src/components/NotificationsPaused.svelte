<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import { pausedLine } from "../lib/format";
  import type { NotificationsPaused } from "../lib/types";

  interface Props {
    /** The pause in force, from `Status`; `null` hides the strip. */
    paused: NotificationsPaused | null;
    /** End the pause. */
    onresume?: () => void;
  }

  let { paused, onresume }: Props = $props();
</script>

<!--
  Set from the tray menu's "Pause notifications". Shown for as long as the
  pause lasts, because a silenced monitor that does not say so reads as one
  with nothing to say. Not an alert: nothing is wrong. The sentence wraps
  rather than truncating, and the button keeps its width.
-->
{#if paused}
  <div
    class="paused border-border bg-muted/40 text-muted-foreground flex items-center gap-2 border-b px-3 py-1.5 text-xs"
    role="status"
    data-slot="notifications-paused"
    data-until={paused.until ?? "resumed"}
  >
    <span class="min-w-0 flex-1 break-words" data-line>{pausedLine(paused.until)}</span>
    {#if onresume}
      <Button size="xs" variant="outline" class="shrink-0" onclick={onresume} data-action="resume">Resume</Button>
    {/if}
  </div>
{/if}
