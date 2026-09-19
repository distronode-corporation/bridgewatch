<script lang="ts">
  /** Step 6: the exact config.toml that Finish writes. */
  import * as Alert from "$lib/components/ui/alert/index.js";
  import type { DiagnosticView } from "../../lib/types";
  import type { WizardPreview } from "./api";

  interface Props {
    preview: WizardPreview | null;
    loading: boolean;
    error: string | null;
    /** Problems the save itself reported (validation, conflict). */
    saveDiagnostics: readonly DiagnosticView[];
    saveError: string | null;
    /** Where the token comes from, in words: the preview never holds it. */
    tokenNote: string | null;
    /** The `primary` answer: primary beside a primary already in the file. */
    primary?: boolean;
    /** Change the `primary` answer; the preview is rebuilt from it. */
    onprimary?: (primary: boolean) => void;
  }

  let {
    preview,
    loading,
    error,
    saveDiagnostics,
    saveError,
    tokenNote,
    primary = false,
    onprimary,
  }: Props = $props();

  const where = (d: DiagnosticView) => (d.line !== null ? `line ${d.line}: ` : d.path ? `${d.path}: ` : "");
</script>

<div class="flex flex-col gap-3">
  {#if loading}
    <p class="text-muted-foreground text-sm" aria-live="polite">Building config.toml…</p>
  {:else if error}
    <Alert.Root variant="destructive" data-slot="preview-error">
      <Alert.Title>Could not build the config</Alert.Title>
      <Alert.Description>{error}</Alert.Description>
    </Alert.Root>
  {:else if preview}
    <p class="text-muted-foreground text-sm">
      {preview.edited_existing
        ? "Your existing config.toml, with these changes. Everything else in it is kept."
        : "This is the config.toml Finish will write. You can edit it later in Settings or any editor."}
    </p>
    <!-- Focusable so a keyboard user can scroll a long preview. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <pre
      class="bg-muted max-h-72 overflow-auto rounded-lg p-3 font-mono text-xs"
      tabindex="0"
      aria-label="config.toml preview"
      data-slot="toml-preview">{preview.toml}</pre>
    {#if preview.secondary_because}
      <p class="text-muted-foreground text-xs" data-slot="secondary-note">
        Added as a secondary watch, because "{preview.secondary_because}" is already primary and the tray icon
        follows the primary watches.
      </p>
    {/if}
    {#if onprimary && (preview.secondary_because || primary)}
      <label class="flex items-center gap-2 text-xs" data-slot="primary-choice">
        <input
          type="checkbox"
          checked={primary}
          onchange={(event) => onprimary((event.currentTarget as HTMLInputElement).checked)}
        />
        Make this watch primary too
      </label>
    {/if}
    {#if tokenNote}
      <p class="text-muted-foreground text-xs" data-slot="token-note">{tokenNote}</p>
    {/if}
    {#each preview.warnings as warning, i (i)}
      <p class="text-tone-amber text-xs" data-slot="preview-warning">{where(warning)}{warning.message}</p>
    {/each}
  {/if}

  {#if saveError || saveDiagnostics.length > 0}
    <Alert.Root variant="destructive" data-slot="save-error">
      <Alert.Title>Not saved</Alert.Title>
      <Alert.Description>
        {#if saveError}<span class="block">{saveError}</span>{/if}
        {#each saveDiagnostics as d, i (i)}
          <span class="block">{where(d)}{d.message}</span>
        {/each}
      </Alert.Description>
    </Alert.Root>
  {/if}
</div>
