<script lang="ts">
  /** An inline error (or hint) under a field, referenced by aria-describedby. */
  interface Props {
    id: string;
    message?: string | null;
    hint?: string | null;
  }

  let { id, message = null, hint = null }: Props = $props();
</script>

{#if message}
  <p {id} class="text-destructive text-xs" role="alert" data-slot="field-error">{message}</p>
{:else if hint}
  <p {id} class="text-muted-foreground text-xs" data-slot="field-hint">{hint}</p>
{/if}
