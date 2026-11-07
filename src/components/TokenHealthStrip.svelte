<script lang="ts">
  import type { TokenHealth } from "../lib/types";

  interface Props {
    /** `Snapshot.tokens`: absent or empty hides the strip. */
    tokens?: TokenHealth[];
    /** Open Settings, where a token is replaced or a sign-in redone. */
    onsettings?: () => void;
  }

  let { tokens = [], onsettings }: Props = $props();

  // Expiring is a warning (amber, a status); expired, revoked and invalid
  // mean nothing is polled for that account (red, an alert).
  const broken = $derived(tokens.some((t) => t.state !== "expiring"));
</script>

<!--
  Token health (F3). Shown for as long as it is true; the notification for
  each warning is sent once by the core. The sentence names the account and
  what to do, and wraps rather than truncating (text may run 30% longer).
-->
{#if tokens.length > 0}
  <div
    class={[
      "token-health border-border flex flex-col gap-0.5 border-b px-3 py-1.5 text-xs",
      broken ? "bg-tone-red-bg text-tone-red" : "bg-tone-amber-bg text-tone-amber",
    ]}
    role={broken ? "alert" : "status"}
    data-slot="token-health"
  >
    {#each tokens as t (t.account)}
      <div class="line break-words" data-line data-account={t.account} data-state={t.state} data-warn={t.warn_hours ?? undefined}>
        {t.message}
      </div>
    {/each}
    {#if onsettings}
      <button
        class="settings mt-0.5 self-start text-[11px] font-medium underline underline-offset-2 outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        onclick={onsettings}
        data-action="token-settings">Open Settings…</button
      >
    {/if}
  </div>
{/if}
