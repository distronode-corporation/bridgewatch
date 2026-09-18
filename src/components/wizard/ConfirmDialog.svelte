<script lang="ts">
  /**
   * "Are you sure?" for running a token command, in the shell's words when it
   * sent some (Lo13 ConfirmRequest) and in ours before the first run.
   *
   * The command is shown as argv, one argument per line, so what will run is
   * exactly what the user reads: nothing is interpreted by a shell.
   */
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";

  interface Props {
    open: boolean;
    title: string;
    /** One sentence per change (the shell's `ConfirmRequest.changes`). */
    lines: readonly string[];
    command?: readonly string[] | null;
    confirmLabel?: string;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let { open, title, lines, command = null, confirmLabel = "Run it", onConfirm, onCancel }: Props = $props();
</script>

<Dialog.Root
  {open}
  onOpenChange={(next) => {
    if (!next) onCancel();
  }}
>
  <Dialog.Content class="sm:max-w-md" data-slot="confirm-dialog" showCloseButton={false}>
    <Dialog.Header>
      <Dialog.Title>{title}</Dialog.Title>
      <Dialog.Description>
        {#each lines as line (line)}
          <span class="block">{line}</span>
        {/each}
      </Dialog.Description>
    </Dialog.Header>
    {#if command && command.length > 0}
      <ol class="bg-muted flex flex-col gap-0.5 rounded-lg p-2 font-mono text-xs" aria-label="command and arguments">
        {#each command as arg, i (i)}
          <li class="break-all"><span class="text-muted-foreground select-none">{i === 0 ? "run " : "arg "}</span>{arg}</li>
        {/each}
      </ol>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={onCancel} data-action="cancel">Cancel</Button>
      <Button onclick={onConfirm} data-action="confirm">{confirmLabel}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
