<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import type { Validation } from "../../lib/types";
  import { TEXTAREA } from "./styles";

  interface Props {
    text: string;
    validation: Validation | null;
    /** Re-read the file: the new base when the user chooses to overwrite. */
    diskText: () => Promise<string>;
    onvalidate: (text: string) => void;
    /**
     * Resolves with what the shell said. `base` is the file text this edit is
     * relative to; the shell refuses with `conflict` when the file moved.
     */
    onsave: (text: string, base: string) => Promise<Validation>;
    onrevert: () => void;
  }

  let { text, validation, diskText, onvalidate, onsave, onrevert }: Props = $props();

  let draft = $state("");
  let dirty = $state(false);
  let saving = $state(false);
  let conflict = $state(false);
  /** The disk text this draft was seeded from: what the edit is RELATIVE TO. */
  let base = $state("");

  // Re-seed from disk whenever the saved text changes, unless the user has
  // started typing: clobbering an in-progress edit because the file watcher
  // fired would be worse than showing a stale buffer.
  $effect(() => {
    const fromDisk = text;
    if (!dirty) {
      draft = fromDisk;
      base = fromDisk;
    }
  });

  /**
   * Save, and keep the draft unless the file really took it.
   *
   * ⛔ `dirty = false` used to run the instant Save was clicked, before the
   * core had answered. The effect above then re-seeded `draft` from the
   * on-disk text — so a save REFUSED for a typo threw away the whole edit and
   * put the old file back in the box, with a red message about a mistake the
   * user could no longer see or fix.
   *
   * ⛔ H15: the write is a compare-and-swap IN THE SHELL against `base`, the
   * text this draft was seeded from. A buffer seeded an hour ago used to
   * silently revert an edit made in `$EDITOR` since; now the shell answers
   * `conflict` and writes nothing. Overwriting is an explicit second act,
   * which re-reads the file and uses THAT as the base.
   */
  async function save(overwrite = false) {
    if (saving) return;
    saving = true;
    try {
      const against = overwrite ? await diskText().catch(() => base) : base;
      const result = await onsave(draft, against);
      if (result.conflict) {
        conflict = true;
        return;
      }
      if (!result.ok) return;
      // Only now: the file holds this text, so re-seeding from disk is a no-op
      // rather than a loss.
      dirty = false;
      conflict = false;
    } finally {
      saving = false;
    }
  }

  const errors = $derived(validation?.diagnostics.filter((d) => d.severity === "error") ?? []);
  const warnings = $derived(validation?.diagnostics.filter((d) => d.severity === "warning") ?? []);
</script>

<!--
  A <textarea>, not a code editor. A syntax-highlighting editor is a megabyte of
  dependency and a build-time integration for a file most people will open twice;
  what actually matters is that the diagnostics carry line and column, which the
  core supplies and which is printed under the box.
-->
<textarea
  class={["editor h-[340px]", TEXTAREA]}
  spellcheck="false"
  value={draft}
  oninput={(e) => {
    draft = (e.currentTarget as HTMLTextAreaElement).value;
    dirty = true;
  }}
></textarea>

<div class="actions my-2 flex items-center gap-2">
  <Button variant="outline" size="sm" onclick={() => onvalidate(draft)}>Validate</Button>
  <Button size="sm" class="save" onclick={() => void save()} disabled={saving}>Save</Button>
  <Button
    variant="ghost"
    size="sm"
    onclick={() => {
      dirty = false;
      conflict = false;
      onrevert();
    }}>Revert</Button
  >
  {#if validation}
    <span class={["verdict text-xs", validation.ok ? "ok text-tone-green" : "bad text-tone-red"]}>
      {validation.ok ? "valid" : `${errors.length} error(s)`}
    </span>
  {/if}
</div>

{#if conflict}
  <div class="conflict border-tone-amber bg-tone-amber-bg mb-2 rounded-xl border px-3 py-2 text-xs" role="alert">
    <strong>The file changed on disk</strong> since this tab read it — another editor, another
    window, or a form tab in this one. Saving now would throw that change away.
    <div class="actions mt-2 flex items-center gap-2">
      <Button variant="destructive" size="sm" class="overwrite" onclick={() => void save(true)} disabled={saving}>
        Overwrite the file with what is in the box
      </Button>
      <Button
        variant="outline"
        size="sm"
        onclick={() => {
          dirty = false;
          conflict = false;
          onrevert();
        }}>Discard mine and reload</Button
      >
    </div>
  </div>
{/if}

{#if errors.length > 0 || warnings.length > 0}
  <ul class="diagnostics m-0 list-none p-0 text-xs">
    {#each [...errors, ...warnings] as d, index (index)}
      <li class={["border-border border-t py-1", d.severity, d.severity === "error" ? "text-tone-red" : "text-tone-amber"]}>
        <span class="where text-muted-foreground mr-1.5 font-mono">
          {d.line ? `${d.line}:${d.col}` : ""}{d.path ? ` ${d.path}` : ""}
        </span>
        {d.message}
      </li>
    {/each}
  </ul>
{/if}
