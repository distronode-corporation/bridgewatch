<script lang="ts">
  /**
   * "updated 4s ago", ticking once a second.
   *
   * `at` is `Snapshot.last_poll` (RFC 3339) or ms since the epoch. With `now`
   * given it renders against that clock and runs no timer.
   */
  import { updatedAgo } from "./jobs";

  interface Props {
    at: string | number | null | undefined;
    now?: number;
    /** "4s ago" without the leading "updated". */
    short?: boolean;
    class?: string;
  }

  let { at, now: externalNow, short = false, class: className = "" }: Props = $props();

  let ownNow = $state(Date.now());
  const now = $derived(externalNow ?? ownNow);

  $effect(() => {
    if (externalNow !== undefined) return;
    ownNow = Date.now();
    const timer = setInterval(() => {
      ownNow = Date.now();
    }, 1000);
    return () => clearInterval(timer);
  });

  const title = $derived(typeof at === "number" ? new Date(at).toISOString() : (at ?? ""));
</script>

<span
  class={["text-muted-foreground text-ui-meta tabular-nums", className]}
  data-slot="updated-ago"
  title={title}
  aria-live="off">{updatedAgo(at, now, short)}</span
>
