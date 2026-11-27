<!--
SPDX-FileCopyrightText: Distronode Corporation

SPDX-License-Identifier: Apache-2.0
-->

<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
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
      <Button
        size="xs"
        variant="outline"
        class="retry mt-1 self-start"
        onclick={onretry}
        data-action="approval-retry">Retry</Button
      >
    {/if}
  </div>
{/if}
