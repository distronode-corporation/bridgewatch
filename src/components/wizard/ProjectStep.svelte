<script lang="ts">
  /** Step 2: pick from the token's projects, or type an id, path or URL. */
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import FieldMessage from "./FieldMessage.svelte";
  import type { ProjectListing, ProjectSummary } from "./api";
  import { projectRefFor, type Draft, type StepErrors } from "./model";

  interface Props {
    draft: Draft;
    errors: StepErrors;
    listing: ProjectListing | null;
    loading: boolean;
    loadError: string | null;
    resolving: boolean;
    /** Pick a listed project. */
    onPick: (project: ProjectSummary) => void;
    /** Look up `draft.projectInput`. */
    onResolve: () => void;
    /** Search the listing again. */
    onSearch: (term: string) => void;
  }

  let { draft = $bindable(), errors, listing, loading, loadError, resolving, onPick, onResolve, onSearch }: Props =
    $props();

  let search = $state("");
  const projects = $derived(listing?.mode === "projects" ? listing.projects : []);
  const github = $derived(draft.provider === "github");
  /** GitHub says repository; GitLab says project. */
  const noun = $derived(github ? "repositories" : "projects");
</script>

<div class="flex flex-col gap-4">
  {#if loading}
    <p class="text-muted-foreground text-sm" aria-live="polite">Loading {noun}…</p>
  {:else if loadError}
    <Alert.Root variant="destructive">
      <Alert.Title>Could not list {noun}</Alert.Title>
      <Alert.Description>{loadError} Type the {github ? "repository" : "project"} below instead.</Alert.Description>
    </Alert.Root>
  {:else if listing?.mode === "type_id_or_path"}
    <p class="text-muted-foreground text-sm" data-slot="type-reason">{listing.reason}</p>
  {:else if listing?.mode === "projects"}
    <div class="flex flex-col gap-2">
      <div class="flex gap-2">
        <Label for="wizard-project-search" class="sr-only">Search {noun}</Label>
        <Input
          id="wizard-project-search"
          type="search"
          placeholder="Search your {noun}"
          bind:value={search}
          onkeydown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              onSearch(search.trim());
            }
          }}
        />
        <Button variant="outline" size="sm" onclick={() => onSearch(search.trim())}>Search</Button>
      </div>
      {#if projects.length === 0}
        <p class="text-muted-foreground text-sm">No {noun} match.</p>
      {:else}
        <fieldset class="flex max-h-56 flex-col gap-0.5 overflow-y-auto rounded-lg border p-1" aria-describedby="wizard-project-msg">
          <legend class="sr-only">{github ? "Repositories" : "Projects"}</legend>
          {#each projects as project (project.id)}
            <label class="hover:bg-muted flex items-center gap-2 rounded-md px-2 py-1 text-sm">
              <input
                type="radio"
                name="project"
                value={project.id}
                class="accent-primary"
                checked={draft.project?.ref === projectRefFor(draft.provider, project)}
                onchange={() => onPick(project)}
              />
              <span class="min-w-0 truncate font-mono">{project.path}</span>
              {#if project.default_branch}
                <span class="text-muted-foreground ml-auto shrink-0 text-xs">{project.default_branch}</span>
              {/if}
            </label>
          {/each}
        </fieldset>
        {#if listing.truncated}
          <p class="text-muted-foreground text-xs">More {noun} than shown; search to narrow the list.</p>
        {/if}
      {/if}
    </div>
  {/if}

  <div class="flex flex-col gap-1">
    <Label for="wizard-project-input">{github ? "Repository (owner/repo or URL)" : "Project id, path or URL"}</Label>
    <div class="flex gap-2">
      <Input
        id="wizard-project-input"
        name="project"
        placeholder={github ? "owner/repo" : "group/project or 12345"}
        spellcheck={false}
        class="font-mono"
        bind:value={draft.projectInput}
        aria-invalid={errors.project ? "true" : undefined}
        aria-describedby="wizard-project-msg"
        onkeydown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            onResolve();
          }
        }}
      />
      <Button
        variant="outline"
        size="sm"
        onclick={onResolve}
        disabled={resolving || !draft.projectInput.trim()}
        data-action="resolve-project">{resolving ? "Looking…" : "Look up"}</Button
      >
    </div>
    <FieldMessage id="wizard-project-msg" message={errors.project} />
  </div>

  {#if draft.project}
    <p class="text-sm" data-slot="chosen-project">
      Watching <span class="font-mono">{draft.project.path}</span>
      {#if draft.project.default_branch}<span class="text-muted-foreground">(default branch {draft.project.default_branch})</span>{/if}
    </p>
  {/if}
</div>
