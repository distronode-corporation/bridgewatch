<script lang="ts">
  /** Step 4: which job marks a deploy, suggested from the ref's latest pipeline, or none. */
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import FieldMessage from "./FieldMessage.svelte";
  import type { MarkerSuggestion, MarkerSuggestions } from "./api";
  import type { Draft, StepErrors } from "./model";

  interface Props {
    draft: Draft;
    errors: StepErrors;
    suggestions: MarkerSuggestions | null;
    loading: boolean;
    loadError: string | null;
  }

  let { draft = $bindable(), errors, suggestions, loading, loadError }: Props = $props();

  let custom = $state("");
  /** GitHub's unit is a workflow run; GitLab's a pipeline. */
  const run = $derived(draft.provider === "github" ? "workflow run" : "pipeline");

  const byName = $derived(new Map((suggestions?.suggestions ?? []).map((s) => [s.name, s] as const)));
  /** Suggested names first (in rank order), then any the draft already holds. */
  const names = $derived([
    ...(suggestions?.suggestions ?? []).map((s) => s.name),
    ...draft.markers.filter((m) => m.trim() && !byName.has(m)),
  ]);

  function toggle(name: string, on: boolean) {
    draft.markers = on ? [...new Set([...draft.markers, name])] : draft.markers.filter((m) => m !== name);
    if (on) draft.noMarker = false;
  }

  function addCustom() {
    const name = custom.trim();
    if (!name) return;
    toggle(name, true);
    custom = "";
  }

  const where = (s: MarkerSuggestion) => (s.pipeline === "parent" ? null : `in ${s.pipeline}'s child`);
</script>

<div class="flex flex-col gap-4">
  <p class="text-muted-foreground text-sm" data-slot="marker-explainer">
    A deploy marker is the job whose success means "this commit is live"; bridgewatch shows the {run} as deployed once
    it passes.
  </p>

  <fieldset class="flex flex-col gap-2" aria-describedby="wizard-markers-msg">
    <legend class="mb-1 text-sm font-medium">Deploy detection</legend>
    <label class="flex items-center gap-2 text-sm">
      <input
        type="radio"
        name="marker-mode"
        value="markers"
        class="accent-primary"
        checked={!draft.noMarker}
        onchange={() => (draft.noMarker = false)}
      />
      Mark deploys with these jobs
    </label>

    <div class="flex flex-col gap-2 pl-6">
      {#if loading}
        <p class="text-muted-foreground text-sm" aria-live="polite">Reading the latest {run}…</p>
      {:else if loadError}
        <Alert.Root variant="destructive">
          <Alert.Title>Could not read the latest {run}</Alert.Title>
          <Alert.Description>{loadError} You can still type a job name.</Alert.Description>
        </Alert.Root>
      {:else if suggestions && suggestions.suggestions.length === 0}
        <p class="text-muted-foreground text-sm" data-slot="no-suggestions">
          {suggestions.pipeline_id === null
            ? `No ${run} on this branch yet, so nothing to suggest.`
            : `No job in the latest ${run} looks like a deploy.`}
        </p>
      {/if}

      {#each names as name (name)}
        {@const s = byName.get(name)}
        <div class="flex items-start gap-2" data-marker={name}>
          <Checkbox
            id={`wizard-marker-${name}`}
            class="mt-0.5"
            checked={!draft.noMarker && draft.markers.includes(name)}
            disabled={draft.noMarker}
            onCheckedChange={(on) => toggle(name, on === true)}
            aria-label={name}
          />
          <div class="flex min-w-0 flex-col">
            <Label for={`wizard-marker-${name}`} class="font-mono font-normal">
              {name}
              {#if s?.stage}<Badge variant="secondary">{s.stage}</Badge>{/if}
            </Label>
            {#if s}
              <span class="text-muted-foreground text-xs" data-slot="marker-reason">
                {[where(s), ...s.reasons].filter(Boolean).join("; ")}
              </span>
            {/if}
          </div>
        </div>
      {/each}

      {#if suggestions && suggestions.unread_children.length > 0}
        <p class="text-muted-foreground text-xs">
          Could not read the child pipelines of {suggestions.unread_children.join(", ")}.
        </p>
      {/if}

      <div class="flex gap-2">
        <Label for="wizard-marker-custom" class="sr-only">Another job name</Label>
        <Input
          id="wizard-marker-custom"
          class="w-56 font-mono"
          placeholder="another job name"
          spellcheck={false}
          disabled={draft.noMarker}
          bind:value={custom}
          onkeydown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              addCustom();
            }
          }}
        />
        <Button variant="outline" size="sm" onclick={addCustom} disabled={draft.noMarker || !custom.trim()}>Add</Button>
      </div>
    </div>

    <label class="flex items-center gap-2 text-sm">
      <input
        type="radio"
        name="marker-mode"
        value="none"
        class="accent-primary"
        checked={draft.noMarker}
        onchange={() => (draft.noMarker = true)}
      />
      No deploy marker (a green {run} is the end of the story)
    </label>
    <FieldMessage id="wizard-markers-msg" message={errors.deploy_markers} />
  </fieldset>
</div>
