<script lang="ts">
  /** Step 3: branch, pipeline sources, optional secondary watches. */
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import FieldMessage from "./FieldMessage.svelte";
  import { SOURCE_CHOICES, suggestWatchId, type Draft, type StepErrors } from "./model";

  interface Props {
    draft: Draft;
    errors: StepErrors;
  }

  let { draft = $bindable(), errors }: Props = $props();

  function refChanged() {
    if (!draft.watchIdEdited && draft.project) draft.watchId = suggestWatchId(draft.project.path, draft.refName);
  }

  function toggleSource(value: string, on: boolean) {
    draft.sources = on ? [...new Set([...draft.sources, value])] : draft.sources.filter((s) => s !== value);
  }
</script>

<div class="flex flex-col gap-4">
  <div class="flex flex-col gap-1">
    <Label for="wizard-ref">Branch</Label>
    <Input
      id="wizard-ref"
      name="ref_name"
      class="w-56 font-mono"
      spellcheck={false}
      bind:value={draft.refName}
      oninput={refChanged}
      aria-invalid={errors.ref_name ? "true" : undefined}
      aria-describedby="wizard-ref-msg"
    />
    <FieldMessage id="wizard-ref-msg" message={errors.ref_name} hint="Pre-filled with the project's default branch." />
  </div>

  <fieldset class="flex flex-col gap-1.5" aria-describedby="wizard-sources-msg">
    <legend class="mb-1 text-sm font-medium">Pipelines started by</legend>
    {#each SOURCE_CHOICES as choice (choice.value)}
      <div class="flex items-center gap-2">
        <Checkbox
          id={`wizard-source-${choice.value}`}
          checked={draft.sources.includes(choice.value)}
          onCheckedChange={(on) => toggleSource(choice.value, on === true)}
          aria-label={choice.label}
        />
        <Label for={`wizard-source-${choice.value}`} class="font-normal">{choice.label}</Label>
      </div>
    {/each}
    <FieldMessage id="wizard-sources-msg" message={errors.sources} />
  </fieldset>

  <div class="flex flex-col gap-1">
    <Label for="wizard-watch-id">Watch id</Label>
    <Input
      id="wizard-watch-id"
      name="watch_id"
      class="w-56 font-mono"
      spellcheck={false}
      bind:value={draft.watchId}
      oninput={() => (draft.watchIdEdited = true)}
      aria-invalid={errors.watch_id ? "true" : undefined}
      aria-describedby="wizard-watch-id-msg"
    />
    <FieldMessage id="wizard-watch-id-msg" message={errors.watch_id} />
  </div>

  <fieldset class="flex flex-col gap-2">
    <legend class="mb-1 text-sm font-medium">Also watch, quietly</legend>
    <div class="flex items-center gap-2">
      <Switch id="wizard-schedule" bind:checked={draft.scheduleWatch} />
      <Label for="wizard-schedule" class="font-normal">Scheduled pipelines on this branch</Label>
    </div>
    <div class="flex items-center gap-2">
      <Switch id="wizard-preflight" bind:checked={draft.preflightEnabled} />
      <Label for="wizard-preflight" class="font-normal">Preflight branches</Label>
    </div>
    {#if draft.preflightEnabled}
      <div class="flex flex-col gap-1 pl-10">
        <Label for="wizard-preflight-ref" class="sr-only">Preflight branch pattern</Label>
        <Input
          id="wizard-preflight-ref"
          name="preflight_ref"
          class="w-56 font-mono"
          placeholder="pf/*"
          spellcheck={false}
          bind:value={draft.preflightRef}
          aria-invalid={errors.preflight_ref ? "true" : undefined}
          aria-describedby="wizard-preflight-msg"
        />
        <FieldMessage id="wizard-preflight-msg" message={errors.preflight_ref} hint="A glob, e.g. pf/*." />
      </div>
    {/if}
  </fieldset>
</div>
