<script lang="ts">
  /** Step 3: branch, pipeline sources (GitHub: events and a workflow), optional secondary watches. */
  import { untrack } from "svelte";

  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { ExpectSuggestions } from "./api";
  import FieldMessage from "./FieldMessage.svelte";
  import { sourceChoices, suggestWatchId, type Draft, type StepErrors } from "./model";

  interface Props {
    draft: Draft;
    errors: StepErrors;
    /** GitHub, one row per push: the workflow files the newest pushes ran. */
    expectSuggestions?: ExpectSuggestions | null;
    expectLoading?: boolean;
    expectError?: string | null;
    /** Ask for the suggestions (when "one row per push" is turned on). */
    onexpect?: () => void;
  }

  let {
    draft = $bindable(),
    errors,
    expectSuggestions = null,
    expectLoading = false,
    expectError = null,
    onexpect,
  }: Props = $props();

  const github = $derived(draft.provider === "github");
  const choices = $derived(sourceChoices(draft.provider));

  function refChanged() {
    if (!draft.watchIdEdited && draft.project) draft.watchId = suggestWatchId(draft.project.path, draft.refName);
  }

  function toggleSource(value: string, on: boolean) {
    draft.sources = on ? [...new Set([...draft.sources, value])] : draft.sources.filter((s) => s !== value);
    if (github && draft.group === "commit") onexpect?.();
  }

  function setGrouped(on: boolean) {
    draft.group = on ? "commit" : "run";
  }

  // The suggestions follow what the watch will follow: asked when the step
  // shows a commit group (turned on here, or a re-run that lands with one), and
  // again when the branch is changed (on its change, not every keystroke) or an
  // event is ticked. The loader skips a branch and event set it has answered.
  //
  // ⛔ `untrack`: the loader reads the branch and the events before its first
  // await, and an effect tracks every read in the call. Tracked, each keystroke
  // in the branch field was a request (Seer, #84). The group and the provider
  // are the effect's only triggers; the branch's change event and the event
  // checkboxes ask on their own.
  $effect(() => {
    if (github && draft.group === "commit") untrack(() => onexpect?.());
  });

  function toggleExpect(file: string, on: boolean) {
    draft.expect = on ? [...new Set([...draft.expect, file])] : draft.expect.filter((f) => f !== file);
  }
</script>

<div class="flex flex-col gap-4">
  <div class="flex flex-col gap-1">
    <Label for="wizard-ref">Branch</Label>
    <Input
      id="wizard-ref"
      name="ref_name"
      class="w-full max-w-56 font-mono"
      spellcheck={false}
      bind:value={draft.refName}
      oninput={refChanged}
      onchange={() => {
        if (github && draft.group === "commit") onexpect?.();
      }}
      aria-invalid={errors.ref_name ? "true" : undefined}
      aria-describedby="wizard-ref-msg"
    />
    <FieldMessage id="wizard-ref-msg" message={errors.ref_name} hint="Pre-filled with the project's default branch." />
  </div>

  <fieldset class="flex flex-col gap-1.5" aria-describedby="wizard-sources-msg">
    <legend class="mb-1 font-medium">{github ? "Workflow runs triggered by" : "Pipelines started by"}</legend>
    {#each choices as choice (choice.value)}
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

  {#if github}
    <div class="flex flex-col gap-1">
      <div class="flex items-center gap-2">
        <Switch
          id="wizard-group-commit"
          checked={draft.group === "commit"}
          onCheckedChange={(on) => setGrouped(on === true)}
        />
        <Label for="wizard-group-commit" class="font-normal">One row per push, every workflow as a part of it</Label>
      </div>
      <FieldMessage
        id="wizard-group-msg"
        hint="A push that deployed while another workflow failed then reads as deployed with a failure, as a GitLab pipeline with a failed child does."
      />
    </div>
  {/if}

  {#if github && draft.group === "commit"}
    <fieldset class="flex flex-col gap-1.5" data-slot="expect" aria-describedby="wizard-expect-msg">
      <legend class="mb-1 font-medium">Workflows every push must start</legend>
      {#if expectLoading}
        <p class="text-muted-foreground m-0">Reading the newest pushes…</p>
      {:else if expectError}
        <p class="text-destructive m-0" role="alert">{expectError}</p>
      {:else if expectSuggestions && expectSuggestions.workflows.length > 0}
        {#each expectSuggestions.workflows as w (w.file)}
          <div class="flex items-center gap-2">
            <Checkbox
              id={`wizard-expect-${w.file}`}
              checked={draft.expect.includes(w.file)}
              onCheckedChange={(on) => toggleExpect(w.file, on === true)}
              aria-label={w.file}
            />
            <Label for={`wizard-expect-${w.file}`} class="font-normal"
              ><span class="font-mono">{w.file}</span>
              <span class="text-muted-foreground">in {w.seen_in} of the last {expectSuggestions.commits}</span></Label
            >
          </div>
        {/each}
      {:else if expectSuggestions}
        <p class="text-muted-foreground m-0" data-slot="expect-none">
          No runs on this branch yet. Add them later in Settings.
        </p>
      {/if}
      <FieldMessage
        id="wizard-expect-msg"
        hint="A ticked workflow that does not start for a push makes that push read as failed. Leave out any whose paths or branches filters can skip a push."
      />
    </fieldset>
  {:else if github}
    <div class="flex flex-col gap-1">
      <Label for="wizard-workflow">Workflow (optional)</Label>
      <Input
        id="wizard-workflow"
        name="workflow"
        class="w-full max-w-56 font-mono"
        placeholder="ci.yml"
        spellcheck={false}
        bind:value={draft.workflow}
        aria-invalid={errors.workflow ? "true" : undefined}
        aria-describedby="wizard-workflow-msg"
      />
      <FieldMessage
        id="wizard-workflow-msg"
        message={errors.workflow}
        hint="The workflow file name, e.g. ci.yml (or its numeric id); leave it empty for every workflow."
      />
    </div>
  {/if}

  <div class="flex flex-col gap-1">
    <Label for="wizard-watch-id">Watch id</Label>
    <Input
      id="wizard-watch-id"
      name="watch_id"
      class="w-full max-w-56 font-mono"
      spellcheck={false}
      bind:value={draft.watchId}
      oninput={() => (draft.watchIdEdited = true)}
      aria-invalid={errors.watch_id ? "true" : undefined}
      aria-describedby="wizard-watch-id-msg"
    />
    <FieldMessage id="wizard-watch-id-msg" message={errors.watch_id} />
  </div>

  <fieldset class="flex flex-col gap-2">
    <legend class="mb-1 font-medium">Also watch, quietly</legend>
    <div class="flex items-center gap-2">
      <Switch id="wizard-schedule" bind:checked={draft.scheduleWatch} />
      <Label for="wizard-schedule" class="font-normal"
        >{github ? "Scheduled runs on this branch" : "Scheduled pipelines on this branch"}</Label
      >
    </div>
    <!-- A pf/* preflight watch is a GitLab idiom; the core refuses one for a GitHub account. -->
    {#if !github}
      <div class="flex items-center gap-2">
        <Switch id="wizard-preflight" bind:checked={draft.preflightEnabled} />
        <Label for="wizard-preflight" class="font-normal">Preflight branches</Label>
      </div>
    {/if}
    {#if draft.preflightEnabled && !github}
      <div class="flex flex-col gap-1 pl-10">
        <Label for="wizard-preflight-ref" class="sr-only">Preflight branch pattern</Label>
        <Input
          id="wizard-preflight-ref"
          name="preflight_ref"
          class="w-full max-w-56 font-mono"
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
