<script lang="ts">
  interface Props {
    /** `Status.needsApproval`: absent or empty hides the strip. */
    accounts?: string[];
    /** Ask once more: the shell's Refresh, which forgets every refused prompt. */
    onretry?: () => void;
  }

  let { accounts = [], onretry }: Props = $props();

  const sentence = $derived(
    `${accounts.length === 1 ? "Account" : "Accounts"} ${accounts.join(", ")}: the credential store's prompt was cancelled, so bridgewatch does not ask again on its own.`,
  );
</script>

<!--
  A credential prompt that was cancelled or denied. ⛔ The shell never asks
  again on its own (on macOS every retry was a new Keychain dialog), so the
  only way back is a person pressing Retry, and the strip stays until then.
-->
{#if accounts.length > 0}
  <div
    class="needs-approval bg-tone-amber-bg text-tone-amber border-border flex flex-col gap-0.5 border-b px-3 py-1.5 text-xs"
    role="status"
    data-slot="needs-approval"
  >
    <div class="line break-words" data-line>{sentence}</div>
    {#if onretry}
      <button
        class="retry mt-0.5 self-start text-[11px] font-medium underline underline-offset-2 outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        onclick={onretry}
        data-action="approval-retry">Retry</button
      >
    {/if}
  </div>
{/if}
